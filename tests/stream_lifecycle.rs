//! Mid-stream lifecycle wire tests for the proxy (external review item 9): the
//! two directions the pass-through matrix left open — an engine that dies after
//! the response head, and a client that drops a live stream.
//!
//! The engine here is a raw-TCP wire mock, not the echo server of
//! `tests/mock_runtime.rs`: this suite needs exact HTTP bytes (an
//! under-delivered `Content-Length`, an endless chunked body) and needs to
//! observe the engine's write side, so it answers the harness on a real
//! loopback socket with hand-written responses. The harness side is the real
//! in-process router (the pattern of `tests/api_surface.rs`), so every
//! assertion is about served bytes, never internal actor state: the fake
//! engine's own request/spawn counters stand in for "no retry happened", and
//! anything the outside cannot see stays out of the assertions.
//!
//! Both tests are cardless: ephemeral ports, no real engine, no model load,
//! bounded timeouts on every await.

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use serde_json::Value;
use simpleharn::api::{category_router, control_router, ApiState, BootTimings};
use simpleharn::budget::LedgerOnlyWitness;
use simpleharn::config::{AppConfig, CategoryId};
use simpleharn::engine::mock::MockAdapter;
use simpleharn::engine::{
    EngineAdapter, EngineError, EngineRuntime, LaunchSpec, Probe, ReadinessSpec, SpawnError,
    Spawned,
};
use simpleharn::listeners::ListenerRegistry;
use simpleharn::model::ModelEntry;
use simpleharn::ownership::SpawnTicket;
use simpleharn::scheduler::Scheduler;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_stream::StreamExt;
use tower::ServiceExt;

/// Every harness call, body frame, and engine-side watch is bounded by an
/// explicit deadline; no await in this file waits unbounded.
const HARNESS_CALL: Duration = Duration::from_secs(15);
const STREAM_READ: Duration = Duration::from_secs(10);
const ABORT_WATCH: Duration = Duration::from_secs(5);
/// Gap between the slow engine's SSE ticks.
const TICK_GAP: Duration = Duration::from_millis(40);
/// Hard self-stop of the endless stream: the fake never outlives its purpose.
const STREAM_CAP: Duration = Duration::from_secs(30);

/// `tower::ServiceExt::oneshot` carries no `Host`; the rebinding guard wants a
/// loopback one (the same stand-in `tests/api_surface.rs` uses).
const HOST: &str = "127.0.0.1:8708";

/// The exact bytes the dying engine sends before it closes the connection.
const PARTIAL_BODY: &[u8] = b"data: {\"chunk\":1}\n\n";
/// The `Content-Length` the dying engine claims — a lie the client must see as
/// a terminated stream, never a silent completion.
const CLAIMED_LEN: usize = 8192;

/// One tick of the endless stream.
const TICK: &[u8] = b"data: {\"tick\":1}\n\n";
/// The forward re-serializes the JSON compactly, so this substring is the
/// stream flag on the wire (the same detection `mock_runtime`'s echo makes).
const STREAM_FLAG: &[u8] = br#""stream":true"#;

/// What the wire mock does with an inference request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireMode {
    /// The first inference request gets a response head and partial bytes,
    /// then the connection closes — an engine death after the head.
    MidStreamDeath,
    /// Any streamed request is answered with an endless chunked body; the
    /// moment the client's side of the connection ends, the engine notes it.
    SlowStream,
}

/// The fake engine's observable bookkeeping: what it saw on the wire.
#[derive(Debug)]
struct WireEngine {
    mode: WireMode,
    /// Inference requests accepted (health probes and discovery are not
    /// counted), so "exactly one request" pins the absence of a retry.
    requests: AtomicUsize,
    /// Set when the engine's side of a live stream ends while it is still
    /// streaming: the peer closed (EOF) or the write side broke.
    abort_observed: AtomicBool,
}

#[derive(Debug)]
struct RunningEngine {
    engine: Arc<WireEngine>,
    /// `None` once a kill has taken it: the accept loop is owned by at most
    /// one releaser (the same take-before-abort discipline `mock_runtime` uses).
    accept: Option<tokio::task::JoinHandle<()>>,
}

/// The test `EngineRuntime`: every engine is a real loopback HTTP server on an
/// ephemeral port with hand-written responses, selected by the `WIRE_MODE` the
/// adapter plants in the launch spec.
#[derive(Debug)]
struct WireRuntime {
    engines: Arc<Mutex<HashMap<u16, RunningEngine>>>,
    spawns: AtomicUsize,
    client: reqwest::Client,
}

impl WireRuntime {
    fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(4_000))
            .build()
            .expect("invariant: a client with a timeout and no TLS needs no configuration");
        WireRuntime {
            engines: Arc::new(Mutex::new(HashMap::new())),
            spawns: AtomicUsize::new(0),
            client,
        }
    }

    /// How many engines this runtime has ever started: a mid-stream death or
    /// a client abort that reaped and reloaded would grow this.
    fn spawns(&self) -> usize {
        self.spawns.load(Ordering::SeqCst)
    }

    /// Engines still registered with the runtime.
    fn registered(&self) -> usize {
        self.engines.lock().expect("wire runtime lock").len()
    }

    /// The single engine of a single-model test: the same instance the
    /// follow-up request must reach.
    fn only_engine(&self) -> Arc<WireEngine> {
        let table = self.engines.lock().expect("wire runtime lock");
        assert_eq!(
            table.len(),
            1,
            "this test drives exactly one model, so exactly one engine is registered"
        );
        Arc::clone(
            &table
                .values()
                .next()
                .expect("invariant: len was just checked")
                .engine,
        )
    }
}

fn spawn_failed(error: std::io::Error) -> SpawnError {
    SpawnError::SpawnFailed {
        reason: error.to_string(),
    }
}

#[async_trait]
impl EngineRuntime for WireRuntime {
    async fn spawn(&self, spec: &LaunchSpec, _ticket: SpawnTicket) -> Result<Spawned, SpawnError> {
        let mode = match spec.env.get("WIRE_MODE").map(String::as_str) {
            Some("wire-mid-death") => WireMode::MidStreamDeath,
            Some("wire-slow-stream") => WireMode::SlowStream,
            other => {
                return Err(SpawnError::SpawnFailed {
                    reason: format!("unknown wire mode {other:?}"),
                })
            }
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(spawn_failed)?;
        let port = listener.local_addr().map_err(spawn_failed)?.port();
        let engine = Arc::new(WireEngine {
            mode,
            requests: AtomicUsize::new(0),
            abort_observed: AtomicBool::new(false),
        });
        let accept = tokio::spawn({
            let engine = Arc::clone(&engine);
            async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let engine = Arc::clone(&engine);
                    tokio::spawn(handle_conn(socket, engine));
                }
            }
        });
        self.engines.lock().expect("wire runtime lock").insert(
            port,
            RunningEngine {
                engine,
                accept: Some(accept),
            },
        );
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(Spawned { port, pid: 0 })
    }

    async fn kill(&self, ticket: &SpawnTicket) -> Result<(), SpawnError> {
        let handle = {
            let mut table = self.engines.lock().expect("wire runtime lock");
            table
                .get_mut(&ticket.port)
                .and_then(|entry| entry.accept.take())
        };
        if let Some(handle) = handle {
            handle.abort();
            let _gone = handle.await;
        }
        self.engines
            .lock()
            .expect("wire runtime lock")
            .remove(&ticket.port);
        Ok(())
    }

    async fn release_confirmed(&self, ticket: &SpawnTicket) -> Result<bool, SpawnError> {
        let live = self
            .engines
            .lock()
            .expect("wire runtime lock")
            .contains_key(&ticket.port);
        Ok(!live)
    }

    async fn probe(&self, port: u16, readiness: &ReadinessSpec) -> Probe {
        simpleharn::engine::http_probe(&self.client, port, readiness).await
    }

    async fn discover_model_id(&self, port: u16) -> Option<String> {
        let url = format!("http://127.0.0.1:{port}/v1/models");
        let response = self.client.get(&url).send().await.ok()?;
        let body: Value = response.json().await.ok()?;
        body["data"][0]["id"].as_str().map(str::to_string)
    }
}

/// A `MockAdapter` under a wire-engine id; the id doubles as the `WIRE_MODE`
/// the runtime selects the scripted behavior from.
#[derive(Debug)]
struct WireAdapter {
    engine: String,
}

impl EngineAdapter for WireAdapter {
    fn engine_id(&self) -> &str {
        &self.engine
    }

    fn spec(&self, model: &ModelEntry, port: u16) -> Result<LaunchSpec, EngineError> {
        let mut spec = MockAdapter.spec(model, port)?;
        spec.env
            .insert("WIRE_MODE".to_string(), self.engine.clone());
        Ok(spec)
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// One HTTP/1.1 request off the wire: method, target, and the exact body the
/// engine received. `None` on any framing failure or over-long head/body.
async fn read_request(socket: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 65_536 {
            return None;
        }
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut first_line = head.lines().next().unwrap_or_default().split_whitespace();
    let method = first_line.next()?.to_string();
    let path = first_line.next()?.to_string();
    let mut content_length = 0usize;
    for line in head.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    while buf.len() < head_end + content_length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 65_536 + content_length {
            return None;
        }
    }
    let body = buf[head_end..head_end + content_length].to_vec();
    Some((method, path, body))
}

/// A complete response, closed after: `connection: close` keeps every
/// follow-up request on a fresh connection the accept loop counts.
async fn respond(socket: &mut TcpStream, content_type: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.shutdown().await
}

/// The death after the head: an honest response head, the SSE bytes the engine
/// produced, then the connection ends under the `Content-Length` it claimed.
async fn die_mid_stream(socket: &mut TcpStream) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {CLAIMED_LEN}\r\n\r\n"
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(PARTIAL_BODY).await?;
    socket.shutdown().await
}

fn chunk_frame(data: &[u8]) -> Vec<u8> {
    let mut frame = format!("{:x}\r\n", data.len()).into_bytes();
    frame.extend_from_slice(data);
    frame.extend_from_slice(b"\r\n");
    frame
}

/// The endless stream: head plus a first tick on the wire, then more ticks.
/// A separate reader half watches the client's side, so a full write buffer
/// can never hide the close: whichever side reports the end first (peer EOF,
/// broken write) latches `abort_observed`.
async fn stream_forever(socket: TcpStream, engine: Arc<WireEngine>) {
    let (mut rd, mut wr) = socket.into_split();
    let head =
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
    let first = chunk_frame(TICK);
    if wr.write_all(head).await.is_err() || wr.write_all(&first).await.is_err() {
        engine.abort_observed.store(true, Ordering::SeqCst);
        return;
    }
    let writer = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + STREAM_CAP;
            loop {
                tokio::time::sleep(TICK_GAP).await;
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                if wr.write_all(&chunk_frame(TICK)).await.is_err() {
                    engine.abort_observed.store(true, Ordering::SeqCst);
                    break;
                }
            }
        })
    };
    let mut probe = [0u8; 16];
    let reader_deadline = tokio::time::Instant::now() + STREAM_CAP;
    loop {
        let Ok(read) = tokio::time::timeout_at(reader_deadline, rd.read(&mut probe)).await else {
            break;
        };
        match read {
            Ok(0) | Err(_) => {
                engine.abort_observed.store(true, Ordering::SeqCst);
                break;
            }
            Ok(_) => {}
        }
    }
    writer.abort();
}

async fn handle_conn(mut socket: TcpStream, engine: Arc<WireEngine>) {
    let Some((_method, path, body)) = read_request(&mut socket).await else {
        return;
    };
    if path.starts_with("/health") {
        let _ = respond(
            &mut socket,
            "application/json",
            br#"{"status":"ok","loaded":true}"#,
        )
        .await;
        return;
    }
    if path.starts_with("/v1/models") {
        let _ = respond(
            &mut socket,
            "application/json",
            br#"{"object":"list","data":[{"id":"wire-engine","object":"model"}]}"#,
        )
        .await;
        return;
    }
    let served = engine.requests.fetch_add(1, Ordering::SeqCst) + 1;
    if engine.mode == WireMode::MidStreamDeath && served == 1 {
        let _ = die_mid_stream(&mut socket).await;
        return;
    }
    if engine.mode == WireMode::SlowStream
        && body.windows(STREAM_FLAG.len()).any(|w| w == STREAM_FLAG)
    {
        stream_forever(socket, engine).await;
        return;
    }
    let echo = format!("{{\"served\":{served}}}");
    let _ = respond(&mut socket, "application/json", echo.as_bytes()).await;
}

/// Control and WORK routers over one wire-engine model: the `tests/api_surface.rs`
/// stand-in, but with the scripted wire runtime instead of the echo mock.
#[allow(clippy::type_complexity)]
fn wire_app(model: &str, engine: &str) -> (Router, Router, Arc<WireRuntime>, PathBuf) {
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        engine.to_string(),
        Arc::new(WireAdapter {
            engine: engine.to_string(),
        }) as Arc<dyn EngineAdapter>,
    );
    let dir = simpleharn::testutil::temp_dir("stream-lifecycle");
    std::fs::write(
        dir.join(format!("{model}.yaml")),
        format!("name: {model}\nengine: {engine}\ncategory: WORK\nvram_gb: 4.0\n"),
    )
    .expect("model yaml");
    let config = AppConfig {
        source_path: Some(dir.join("simpleharn.json")),
        ..AppConfig::default()
    };
    let runtime = Arc::new(WireRuntime::new());
    let scheduler = Scheduler::start(
        config.clone(),
        dir.clone(),
        adapters,
        Arc::clone(&runtime) as Arc<dyn EngineRuntime>,
        Arc::new(LedgerOnlyWitness),
    );
    let state = Arc::new(ApiState::new(
        config,
        scheduler,
        ListenerRegistry::new(),
        Arc::new(LedgerOnlyWitness),
        BootTimings::default(),
    ));
    (
        control_router(Arc::clone(&state)),
        category_router(state, CategoryId::Work),
        runtime,
        dir,
    )
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    tokio::time::timeout(HARNESS_CALL, app.clone().oneshot(request))
        .await
        .expect("the harness did not answer within its bound")
        .expect("in-process request")
}

async fn post_json(app: &Router, path: &str, body: &str) -> (StatusCode, Value) {
    let response = send(
        app,
        Request::post(path)
            .header("host", HOST)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("static request"),
    )
    .await;
    let status = response.status();
    let bytes = tokio::time::timeout(
        HARNESS_CALL,
        axum::body::to_bytes(response.into_body(), usize::MAX),
    )
    .await
    .expect("a response body stalled within its bound")
    .expect("bounded body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get_json(app: &Router, path: &str) -> (StatusCode, Value) {
    let response = send(
        app,
        Request::get(path)
            .header("host", HOST)
            .body(Body::empty())
            .expect("static request"),
    )
    .await;
    let status = response.status();
    let bytes = tokio::time::timeout(
        HARNESS_CALL,
        axum::body::to_bytes(response.into_body(), usize::MAX),
    )
    .await
    .expect("a response body stalled within its bound")
    .expect("bounded body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn content_type(response: &Response) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn work_chat_request(body: &str) -> Request<Body> {
    Request::post("/v1/chat/completions")
        .header("host", HOST)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("static request")
}

fn serving_slot(doc: &Value, model: &str) -> Value {
    doc["slots"]
        .as_array()
        .expect("slots array")
        .iter()
        .find(|slot| slot["model"] == model)
        .unwrap_or_else(|| panic!("the {model} slot is on /har/status: {doc}"))
        .clone()
}

#[tokio::test]
async fn an_engine_that_dies_mid_stream_truncates_the_stream_and_stays_serving() {
    // The wire test for the documented gap: a death after the response head
    // surfaces INSIDE the body stream, past the crash-retry loop. The client
    // must see the bytes the engine produced and then a stream that terminates
    // with an error — never a silent completion. The retry must not fire (the
    // engine would otherwise see a second request and a restart would grow the
    // spawn count), and the same listener must keep serving afterwards.
    let (control, work, runtime, _dir) = wire_app("trickster", "wire-mid-death");
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"trickster"}"#).await;
    assert_eq!(status, StatusCode::OK, "load: {body}");
    let engine = runtime.only_engine();

    let response = send(
        &work,
        work_chat_request(r#"{"model":"trickster","messages":[]}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "the head crossed");
    assert!(
        content_type(&response).starts_with("text/event-stream"),
        "the engine's content-type must cross untouched: {}",
        content_type(&response)
    );

    let mut stream = response.into_body().into_data_stream();
    let mut received = Vec::new();
    let mut clean_end = false;
    let mut termination: Option<String> = None;
    loop {
        let frame = tokio::time::timeout(STREAM_READ, stream.next())
            .await
            .expect("the body stalled: no chunk, no end, and no error within its bound");
        match frame {
            Some(Ok(bytes)) => received.extend_from_slice(&bytes),
            Some(Err(error)) => {
                termination = Some(error.to_string());
                break;
            }
            None => {
                clean_end = true;
                break;
            }
        }
    }
    assert!(
        !clean_end,
        "the engine died mid-body under a {}-byte Content-Length, but the client saw a silent completion after {} bytes",
        CLAIMED_LEN,
        received.len()
    );
    let termination = termination.expect("invariant: the loop broke on one of the arms");
    assert_eq!(
        received, PARTIAL_BODY,
        "the client must receive exactly the bytes the engine sent before the failure: {termination}"
    );

    // No mid-stream retry: the crash-retry lives before the head, so a death
    // past it may not reach the engine a second time, and the fake may not be
    // reaped and reloaded.
    assert_eq!(
        engine.requests.load(Ordering::SeqCst),
        1,
        "a death mid-body must not fire the crash-retry toward the engine: {termination}"
    );
    assert_eq!(
        runtime.spawns(),
        1,
        "a death mid-body must not restart the resident: {termination}"
    );

    // The harness is still healthy: the next request on the same listener is
    // served by the same engine instance (the counter continues to 2).
    let (status, echo) = post_json(&work, "/v1/chat/completions", r#"{"model":"trickster"}"#).await;
    assert_eq!(status, StatusCode::OK, "follow-up: {echo}");
    assert_eq!(
        echo["served"], 2,
        "the same engine instance must answer the next request: {echo}"
    );
    assert_eq!(engine.requests.load(Ordering::SeqCst), 2, "{echo}");
    assert_eq!(runtime.spawns(), 1, "still no restart across the failure");
    assert_eq!(runtime.registered(), 1, "the resident is still registered");

    // What the harness tells its own page: a mid-body death is not a witnessed
    // death, so the resident keeps serving and no unload row appears.
    let (status, doc) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(
        serving_slot(&doc, "trickster")["state"],
        "serving",
        "a mid-body death must not terminalize the resident: {doc}"
    );
    assert!(
        doc["jobs"]
            .as_array()
            .expect("jobs array")
            .iter()
            .all(|job| !(job["model"] == "trickster" && job["kind"] == "Unload")),
        "a mid-body death arms no unload row: {doc}"
    );
}

#[tokio::test]
async fn a_client_that_drops_the_stream_mid_body_is_observed_by_the_engine() {
    // The client-abort direction: while a live stream runs, the client drops
    // the response. The harness must tear down the engine-side connection —
    // the fake's write side observes the end within its bound — and neither
    // hang nor crash: the same listener serves a follow-up afterwards, and no
    // retry or restart happens anywhere.
    let (control, work, runtime, _dir) = wire_app("slowpoke", "wire-slow-stream");
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"slowpoke"}"#).await;
    assert_eq!(status, StatusCode::OK, "load: {body}");
    let engine = runtime.only_engine();

    let response = send(
        &work,
        work_chat_request(r#"{"model":"slowpoke","stream":true}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        content_type(&response).starts_with("text/event-stream"),
        "the engine's content-type must cross untouched: {}",
        content_type(&response)
    );

    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(STREAM_READ, stream.next())
        .await
        .expect("the stream never started within its bound")
        .expect("the engine stream ended before the client aborted it")
        .expect("the engine streams a first tick that is readable");
    assert!(!first.is_empty(), "the first tick must carry bytes");

    // The abort: the client drops the response body mid-stream, while the
    // engine is still ticking.
    drop(stream);

    let deadline = Instant::now() + ABORT_WATCH;
    while !engine.abort_observed.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the engine never observed the client abort within {} s",
            ABORT_WATCH.as_secs()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // The abort is the client's, not the engine's: nothing retries, nothing
    // restarts, the resident stays registered and serving.
    assert_eq!(
        engine.requests.load(Ordering::SeqCst),
        1,
        "a client abort must not fire any retry toward the engine"
    );
    assert_eq!(
        runtime.spawns(),
        1,
        "a client abort must not restart the engine"
    );
    assert_eq!(runtime.registered(), 1, "the resident is still registered");

    // The harness did not hang or wedge: the same listener answers the next
    // request, served by the same instance (the counter continues to 2).
    let (status, echo) = post_json(&work, "/v1/chat/completions", r#"{"model":"slowpoke"}"#).await;
    assert_eq!(status, StatusCode::OK, "follow-up: {echo}");
    assert_eq!(
        echo["served"], 2,
        "the same engine instance must answer the next request: {echo}"
    );
    assert_eq!(engine.requests.load(Ordering::SeqCst), 2, "{echo}");
    let (status, doc) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(
        serving_slot(&doc, "slowpoke")["state"],
        "serving",
        "a client abort must not touch the resident: {doc}"
    );
}
