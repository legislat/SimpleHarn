use super::*;
use crate::budget::{AttributionQuality, MemorySample, VramWitness};
use crate::scheduler::{SchedulerError, SchedulerStatus, SlotStatus};
use axum::http::Request;
use std::path::PathBuf;
use tower::ServiceExt;

mod cost_payload;
mod engine_death;
mod errors;
mod guard;
mod proxy;
mod readiness;
mod status_render;
mod tokens;
mod writeback;

fn unit_state() -> ApiState {
    // The selection and classification logic never issues a scheduler command,
    // so a dummy handle (no actor) is the honest seam here.
    ApiState::new(
        AppConfig::default(),
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    )
}

/// A state with a LIVE actor: `status_document` asks the scheduler for its
/// snapshot, so [`unit_state`]'s dummy handle cannot serve a `/har/status` test.
/// The empty catalog dir keeps the actor's scan trivial and engine-free.
fn live_state(boot: BootTimings) -> ApiState {
    let config = AppConfig::default();
    let mut adapters: BTreeMap<String, Arc<dyn crate::engine::EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(crate::engine::mock::MockAdapter) as Arc<dyn crate::engine::EngineAdapter>,
    );
    let scheduler = crate::scheduler::Scheduler::start(
        config.clone(),
        crate::testutil::temp_dir("api-boot"),
        adapters,
        Arc::new(crate::process::ProcessRuntime::new()) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(crate::budget::LedgerOnlyWitness),
    );
    ApiState::new(
        config,
        scheduler,
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        boot,
    )
}

/// How many times a warm-up emission is retried before the pin fails loudly.
const CAPTURE_WARM_UP_ATTEMPTS: usize = 8;

/// Emit inside a throwaway capture until every fragment in `wanted` has landed
/// (the flake that read as "the flood's one line
/// is missing", 1 run in 5).
///
/// A `tracing` callsite's `Interest` is cached process-wide on its first touch,
/// and the guard's reporting lines come from callsites another test may touch
/// first with no subscriber installed. The sibling that does exactly that —
/// reading bookkeeping rather than a capture — is
/// `a_refused_token_is_counted_once_and_never_carries_the_token`, and
/// `set_default` (thread-local, which is what `Session::start` uses) never
/// re-registers an already-touched callsite. The symptom is what the D9 capture
/// showed: every `tower_http` line of the refused requests present and the
/// guard's own line absent, because `TraceLayer` is applied outside the guard
/// layers and reports independently of it.
///
/// So the precondition is enforced rather than hoped for: with the throwaway
/// capture as this thread's default, `rebuild_interest_cache` re-evaluates every
/// callsite against it (the step `set_default` alone never performs), the emit
/// runs, and the lines are *asserted* to have arrived — retried until they do, and
/// failing with the capture in hand rather than letting the pin measure against a
/// line that can never appear. No test lock is needed: the captures are
/// thread-local and every warm-up traffic goes through its own `ApiState`, so a
/// sibling cannot spend the `(class, path)` budget the caller asserts about; the
/// interest cache is the only shared state, and this function repairs it.
///
/// Guard-reporting pins no longer call this by hand: `guard_capture_session`
/// performs the warm-up as part of installing the capture, so subscribing
/// without warming is not a shape a guard pin can write.
async fn warm_capture_lines<F, Fut>(wanted: &[&'static str], mut emit: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for attempt in 0..CAPTURE_WARM_UP_ATTEMPTS {
        let session = crate::tray::testlog::Session::start();
        tracing::callsite::rebuild_interest_cache();
        emit().await;
        let lines = session.stop();
        if wanted
            .iter()
            .all(|want| lines.iter().any(|line| line.contains(want)))
        {
            return;
        }
        assert!(
            attempt + 1 < CAPTURE_WARM_UP_ATTEMPTS,
            "the reporting callsites never delivered a line to a capture session: \
                 wanted {wanted:?} after {CAPTURE_WARM_UP_ATTEMPTS} attempts, last \
                 capture {lines:?}"
        );
    }
}

/// A state for warm-up traffic only: never the state a pin measures. The interest
/// cache is keyed by callsite, so any state exercises it, and a throwaway keeps
/// the measured window's dedup budget and fold tallies untouched.
fn warm_state(token: Option<&str>) -> Arc<ApiState> {
    Arc::new(ApiState::new(
        AppConfig {
            control_token: token.map(str::to_string),
            ..AppConfig::default()
        },
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ))
}

fn occupied_slot(model: &str, port: u16, loaded_at: Instant) -> SlotStatus {
    SlotStatus {
        category: CategoryId::Aux,
        index: 0,
        model: Some(model.to_string()),
        port: Some(port),
        pid: Some(1),
        need_gib: Some(4.0),
        idle_s: Some(0),
        serve_id: Some("mock-model".to_string()),
        loading_job: None,
        state: "serving".to_string(),
        launched_at: None,
        stale_config: None,
        container_reported_gb: None,
        booked_ram_gb: None,
        ram_cost_basis: None,
        ram_source: None,
        loaded_at,
    }
}

/// Readiness/crash live harness: the scripted engine answers readiness
/// immediately
/// (`probe_ready`), never invents an id, records every reap and confirms every
/// release — a dead engine's refused port is the harness's own release
/// evidence. Spawn and probe record only; any other runtime call reaching
/// here is a test bug.
#[derive(Default)]
struct ScriptedEngine {
    spawns: std::sync::atomic::AtomicUsize,
    killed: std::sync::Mutex<Vec<(u16, u64)>>,
    probe_ready: std::sync::atomic::AtomicBool,
}

impl ScriptedEngine {
    fn spawn_count(&self) -> usize {
        self.spawns.load(std::sync::atomic::Ordering::Acquire)
    }
    fn killed(&self) -> Vec<(u16, u64)> {
        self.killed.lock().expect("killed lock").clone()
    }
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for ScriptedEngine {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        let n = self
            .spawns
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        Ok(crate::engine::Spawned {
            port: ticket.port,
            pid: 7_000 + n as u32,
        })
    }
    async fn kill(
        &self,
        ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        self.killed
            .lock()
            .expect("killed lock")
            .push((ticket.port, ticket.generation));
        Ok(())
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        Ok(true)
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        if self.probe_ready.load(std::sync::atomic::Ordering::Acquire) {
            crate::engine::Probe::Ready
        } else {
            crate::engine::Probe::NotYet {
                reason: "no answer yet: error sending request for url \
                         (http://127.0.0.1:18111/health)"
                    .to_string(),
            }
        }
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        None
    }
}

/// AUX config on its own reserved ephemeral engine ports, never the operator's
/// 18109 plan and never a protected port. The sockets bind-then-drop: the
/// harness plans the ports, nothing binds them.
fn aux_config_on_reserved_ports(slots: usize, load_timeout_s: u64) -> AppConfig {
    let mut config = AppConfig {
        load_timeout_s,
        ..Default::default()
    };
    let mut probes = Vec::new();
    let mut ports = Vec::new();
    for _ in 0..slots {
        let sock = std::net::TcpListener::bind("127.0.0.1:0").expect("reserved port");
        ports.push(sock.local_addr().expect("addr").port());
        probes.push(sock);
    }
    drop(probes);
    let aux = config
        .category_mut(CategoryId::Aux)
        .expect("AUX is configured");
    aux.max_models = slots;
    aux.engine_ports = ports;
    config
}

/// A live actor with the scripted engine and a seeded catalog dir.
fn scripted_live_state(
    config: AppConfig,
    rows: &[(&str, &str)],
) -> (Arc<ApiState>, Arc<ScriptedEngine>) {
    let dir = crate::testutil::temp_dir("api-rc0102");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("catalog dir");
    for (name, body) in rows {
        std::fs::write(dir.join(format!("{name}.yaml")), *body).expect("seed row");
    }
    let runtime = Arc::new(ScriptedEngine {
        probe_ready: std::sync::atomic::AtomicBool::new(true),
        ..Default::default()
    });
    let mut adapters: BTreeMap<String, Arc<dyn crate::engine::EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(crate::engine::mock::MockAdapter) as Arc<dyn crate::engine::EngineAdapter>,
    );
    let scheduler = crate::scheduler::Scheduler::start(
        config.clone(),
        dir,
        adapters,
        Arc::clone(&runtime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(crate::budget::LedgerOnlyWitness),
    );
    let api = Arc::new(ApiState::new(
        config,
        scheduler,
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));
    (api, runtime)
}

async fn post_control(api: &Arc<ApiState>, path: &str, payload: Value) -> (StatusCode, Value) {
    let response = control_router(Arc::clone(api))
        .oneshot(
            Request::post(path)
                .header("host", "127.0.0.1:8708")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .expect("control request"),
        )
        .await
        .expect("in-process control request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("control body");
    (
        status,
        serde_json::from_slice(&bytes).expect("control json"),
    )
}

async fn get_control(api: &Arc<ApiState>, path: &str) -> (StatusCode, Value) {
    let response = control_router(Arc::clone(api))
        .oneshot(
            Request::get(path)
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("control request"),
        )
        .await
        .expect("in-process control request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("control body");
    (
        status,
        serde_json::from_slice(&bytes).expect("control json"),
    )
}

async fn post_aux_chat(api: &Arc<ApiState>, payload: Value) -> (StatusCode, Value) {
    let response = category_router(Arc::clone(api), CategoryId::Aux)
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("host", "127.0.0.1:8710")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .expect("chat request"),
        )
        .await
        .expect("in-process chat request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("chat body");
    (status, serde_json::from_slice(&bytes).expect("chat json"))
}

/// A chat body naming `model` with one user turn.
fn chat_body(model: &str) -> Value {
    json!({ "model": model, "messages": [{ "role": "user", "content": "hi" }] })
}
