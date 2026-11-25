//! The control plane on the wire: token gating, page and assets, load and
//! unload replies, the jobs and status documents, per-listener filtering,
//! rescan, SSE, the typed request envelope, and the guard matrix (Host,
//! Origin, content type, request ids, token scheme, ring hygiene).

use super::*;
use std::sync::{Mutex, OnceLock};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

/// Process-global line capture for the one pin that must watch the operator's
/// channel (`os_failure_text_reaches_the_ring_but_never_the_client_body`).
/// Same shape as `tests/serve_id_retry.rs`: a `tracing_subscriber` registry is
/// installed once via `try_init`, so the ring stays untouched and the pin reads
/// what `tracing::error!` produced (FINDINGS-05 [6]).
#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<String>>>);

struct LineVisitor<'a> {
    parts: &'a mut Vec<String>,
}

impl tracing::field::Visit for LineVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.parts.push(format!("{}={:?}", field.name(), value));
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut parts = Vec::new();
        event.record(&mut LineVisitor { parts: &mut parts });
        self.0.lock().expect("capture lock").push(format!(
            "{} {} {}",
            event.metadata().level(),
            event.metadata().target(),
            parts.join(" ")
        ));
    }
}

fn capture_lines() -> Arc<Mutex<Vec<String>>> {
    static STORE: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let store = Arc::new(Mutex::new(Vec::new()));
            let _ = tracing_subscriber::registry()
                .with(Capture(store.clone()))
                .try_init();
            store
        })
        .clone()
}

#[tokio::test]
async fn control_token_gates_api_routes_but_never_the_page_itself() {
    // The optional shared secret (`control_token` / `SIMPLEHARN_TOKEN`) is what makes
    // publishing `internal_host` to a segment defensible: without it any LAN host can
    // load, unload, reconfigure and read logs (deploy review, 2026-10-26).
    let config = AppConfig {
        control_token: Some("s3cret".to_string()),
        ..AppConfig::default()
    };
    let (control, work, _rp, _, _runtime, _modelconf) =
        app(config, &[("qwen3", "mock", CategoryId::Work, Some(4.0))]);

    let (status, body) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {body}");
    assert_eq!(body["code"], "unauthorized");

    let wrong = control
        .clone()
        .oneshot(
            Request::get("/har/models")
                .header("host", "127.0.0.1:8708")
                .header("x-har-token", "nope")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let right = control
        .clone()
        .oneshot(
            Request::get("/har/status")
                .header("host", "127.0.0.1:8708")
                .header("x-har-token", "s3cret")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(right.status(), StatusCode::OK);

    let bearer = control
        .clone()
        .oneshot(
            Request::get("/har/models")
                .header("host", "127.0.0.1:8708")
                .header("authorization", "Bearer s3cret")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(bearer.status(), StatusCode::OK);

    // A mutating proxy route is gated as well...
    let (status, _body) = post_json(&work, "/v1/chat/completions", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // ...while the static page and every asset it links are not: the page has to load
    // before it can ask for a token. `/pico.min.css` was missing from the public list,
    // which meant a token-protected harness served an unstyled page (README review).
    // Iterating the guard's own list, not a copy, so adding an asset to the page without
    // exempting it can never pass here again.
    for path in simpleharn::api::PUBLIC_PATHS {
        let page = control
            .clone()
            .oneshot(
                Request::get(path)
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("static request"),
            )
            .await
            .expect("in-process request");
        assert_eq!(page.status(), StatusCode::OK, "public asset {path}");
    }
}

/// Every local asset reference the page itself makes (`href="/..."` / `src="/..."`).
///
/// Absolute URLs (the Google-Fonts link) and `#` anchors are excluded by the leading-slash
/// rule, so the result is the browser's own fetch set — the client's truth, not the guard's
/// copy of it.
///
/// FINDINGS-FIXROUND2 [6]: the scanner matches only the exact double-quoted lowercase
/// forms, so single-quoted, uppercase, spaced, or unquoted attribute forms evade it; the
/// trio floor below keeps today's scan non-vacuous, and hardening is deferred to the card
/// window.
fn linked_assets(html: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for attr in ["href=\"", "src=\""] {
        let mut rest = html;
        while let Some(at) = rest.find(attr) {
            let after = &rest[at + attr.len()..];
            let Some(end) = after.find('"') else {
                break;
            };
            let value = &after[..end];
            if value.starts_with('/') && !value.starts_with("//") {
                found.push(value.to_string());
            }
            rest = after;
        }
    }
    found.sort();
    found.dedup();
    found
}

#[tokio::test]
async fn every_page_linked_asset_is_public() {
    // FINDINGS-16 [1] / FINDINGS-18 [1]: the served-asset set is hand-maintained in three
    // copies (the webui router, PUBLIC_PATHS, index.html's tags) and the loop in the test
    // above asks only for what the guard already exempts — delete an entry there and the
    // request disappears instead of failing. `/pico.min.css` shipped exactly that way: a
    // token-protected deploy served an unstyled page through green gates. Iterating
    // index.html (what the client actually fetches) covers the direction the guard's own
    // list cannot see, so a linked-but-not-exempt asset fails here.
    let config = AppConfig {
        control_token: Some("s3cret".to_string()),
        ..AppConfig::default()
    };
    let (control, _, _, _, _runtime, _modelconf) = app(config, &[]);

    let (status, page) = get_json_html(&control, "/").await;
    assert_eq!(status, StatusCode::OK, "the page must answer tokenless");

    let assets = linked_assets(&page);
    assert!(
        assets.len() >= 3,
        "scanner found only {assets:?} in index.html; a short or empty scan must fail loudly"
    );
    let joined = assets.join(" ");
    assert!(
        joined.contains(".css") && joined.contains(".js"),
        "scanned set {assets:?} holds neither a stylesheet nor a script: the scanner is broken"
    );
    for known in ["/style.css", "/app.js", "/pico.min.css"] {
        assert!(
            assets.iter().any(|path| path == known),
            "index.html links {known}; the scanner missed it (found {assets:?})"
        );
    }

    for path in &assets {
        let (status, body) = get_json_html(&control, path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "linked asset {path} refused tokenless — is it missing from PUBLIC_PATHS?"
        );
        assert!(!body.is_empty(), "linked asset {path} served an empty body");
    }
}

#[tokio::test]
async fn load_flow_populates_jobs_and_status() {
    let (control, _work, _rp, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );

    let (status, body) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], "loaded");
    assert_eq!(body["receipt"]["model"], "qwen3");
    assert_eq!(runtime.server_count(), 1);

    let (status, body) = get_json(&control, "/har/jobs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["jobs"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["jobs"][0]["state"], "Serving");

    let (status, body) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slots"][0]["model"], "qwen3");

    // R2-18-01: `unwrap_or_default()` read a renamed or dropped `id` as 0, and 0 is
    // exactly the id of the one job this test mints — the detail leg could never
    // fail. Demand the key: a ledger row that stops carrying its id goes red here.
    let job_id = body["jobs"][0]["id"]
        .as_u64()
        .expect("the /har/status ledger row carries its id for /har/jobs/{id}");
    let (status, _) = get_json(&control, &format!("/har/jobs/{job_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get_json(&control, "/har/jobs/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn load_unknown_model_is_typed_not_found() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"ghost"}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn unload_bodies_are_strict() {
    let (control, _, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(runtime.server_count(), 1, "one engine is loaded");

    // No category/model but a NON-EMPTY body: 400 (the object is unrecognized).
    let (status, _) = post_json(&control, "/har/unload", r#"{"unknown":1}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Garbage: 400.
    let (status, _) = post_json(&control, "/har/unload", "not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Model unload of a not-loaded model: accepted (T12.2 idempotence), and the
    // reply names the no-op instead of claiming an unload (FINDINGS-07 [5]).
    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"never-loaded"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "nothing_unloaded", "{body}");
    assert_eq!(
        runtime.server_count(),
        1,
        "an idempotent model unload of a never-loaded model frees nothing"
    );
    // Explicit empty object: unload-all (T12.4) — accepted AND it really releases
    // every engine (FINDINGS-18 [5]: the response used to be bound and discarded;
    // a handler that answered 200 while unloading nothing passed).
    let (status, _) = post_json(&control, "/har/unload", "{}").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        await_server_count(&runtime, 0).await,
        0,
        "the empty-object scope actually unloads"
    );

    // Reload, then the scoped form: `{category: WORK}` must free ITS engine, not
    // merely answer 200. `await_server_count` polls because the reply arrives at job
    // START, not when the kill lands (see the helper's note).
    let (status, _) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        runtime.server_count(),
        1,
        "the engine is back for the scoped test"
    );
    // Valid: accepted and actually unloads.
    let (status, body) = post_json(&control, "/har/unload", r#"{"category":"WORK"}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["status"], "unloading",
        "a real category unload: {body}"
    );
    assert_eq!(body["unloading"], 1, "{body}");
    assert_eq!(
        await_server_count(&runtime, 0).await,
        0,
        "the category unload frees its engine"
    );
    let (_, status) = get_json(&control, "/har/status").await;
    let work_slot = status["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .find(|slot| slot["category"] == "WORK")
        .expect("WORK slot");
    assert!(
        work_slot["model"].is_null(),
        "and the slot reads empty afterwards: {status}"
    );
}

#[tokio::test]
async fn unload_reply_names_what_the_actor_actually_did() {
    // FINDINGS-07 [5]: the handler discarded the actor's `Reply` and always answered
    // `{"status":"unloading"}` — the no-op arms (nothing loaded at all, or a model
    // that never was) answered exactly like a scope that really moved. The reply
    // must now name the fact: `nothing_unloaded` when the command moved nothing,
    // `unloading` with the slot count when a real unload was accepted.
    // Falsifier: pre-fix the two `nothing_unloaded` asserts fail on "unloading".
    let (control, _, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );

    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"qwen3"}"#).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an idempotent no-op stays 200: {body}"
    );
    assert_eq!(body["status"], "nothing_unloaded", "{body}");
    assert_eq!(body["model"], "qwen3", "the scope echo survives");
    let (_, body) = post_json(&control, "/har/unload", "{}").await;
    assert_eq!(
        body["status"], "nothing_unloaded",
        "unload-all over an empty harness moved nothing: {body}"
    );
    assert_eq!(runtime.server_count(), 0, "a no-op frees nothing");

    let (status, _) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        runtime.server_count(),
        1,
        "the engine is installed for the real leg"
    );
    let (_, body) = post_json(&control, "/har/unload", r#"{"category":"WORK"}"#).await;
    assert_eq!(
        body["status"], "unloading",
        "a real unload still says unloading: {body}"
    );
    assert_eq!(body["unloading"], 1, "{body}");
    assert_eq!(
        await_server_count(&runtime, 0).await,
        0,
        "the accepted unload really frees the engine"
    );
}

#[tokio::test]
async fn unload_during_a_load_answers_cancelled_not_unloading() {
    // FINDINGS-07 [5], the cancel arm: unloading a still-loading slot cancels the
    // swap; no occupant was ever installed there, so the honest reply is
    // `cancelled` — not the unconditional `unloading` the handler used to print.
    // Falsifier: pre-fix the status assert reads "unloading".
    let (control, _, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("slowcat", "mock-slow", CategoryId::Work, Some(4.0))],
    );
    let loader = {
        let control = control.clone();
        tokio::spawn(
            async move { post_json(&control, "/har/load", r#"{"model":"slowcat"}"#).await },
        )
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, status) = get_json(&control, "/har/status").await;
        let loading = status["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .any(|slot| slot["category"] == "WORK" && !slot["loading_job"].is_null());
        if loading {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the load never became a visible job: {status}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"slowcat"}"#).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["status"], "cancelled",
        "a cancelled load with nothing installed says cancelled: {body}"
    );
    assert_eq!(body["unloading"], 0, "{body}");
    assert_eq!(body["jobs_cancelled"], 1, "{body}");

    let (load_status, load_body) = loader.await.expect("the load task survives");
    assert_eq!(
        load_status,
        StatusCode::CONFLICT,
        "the cancelled load must not report a loaded model: {load_body}"
    );
    assert_eq!(
        await_server_count(&runtime, 0).await,
        0,
        "the cancelled swap leaks no engine"
    );
}

#[tokio::test]
async fn unload_body_with_both_model_and_category_is_refused() {
    // FINDINGS-07 [6]: a body carrying BOTH scopes silently took the wider category
    // branch (tested first), so "unload this one model from WORK" killed every WORK
    // occupant and echoed only the category. README lists the three shapes as
    // alternatives, never combinable: both keys are a structural 400 in the
    // project's `{code, message}` envelope.
    // Falsifier: pre-fix this answers 200 and the WORK engine dies.
    let (control, _, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, _) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runtime.server_count(), 1, "one engine is loaded");

    let (status, body) = post_json(
        &control,
        "/har/unload",
        r#"{"model":"qwen3","category":"WORK"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], "bad_request", "flat envelope: {body}");
    assert_eq!(
        body["error"]["code"], "bad_request",
        "the OpenAI envelope carries the same code"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "a refused scope must kill nothing"
    );
}

#[tokio::test]
async fn models_are_filtered_per_listener() {
    let (_, work, rp, _, _, _modelconf) = app(
        AppConfig::default(),
        &[
            ("work-model", "mock", CategoryId::Work, Some(4.0)),
            ("embed-model", "mock", CategoryId::Emb, Some(2.0)),
        ],
    );

    let (status, body) = get_json(&work, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body["data"]
        .as_array()
        .expect("list")
        .iter()
        .map(|m| m["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["work-model"]);
    // TASK-NAMING-CLEANUP pin (c, wire half): `owned_by: "simpleharn"` is the
    // public wire contract. The old legacy-stable ruling is superseded by the
    // operator ruling of 2026-11-05: the wire value carries the one product
    // name. Falsifier: rename the wire literal and this pin goes red before
    // any client can discover the drift.
    for row in body["data"].as_array().expect("list") {
        assert_eq!(
            row["owned_by"].as_str(),
            Some("simpleharn"),
            "owned_by is the public wire contract of the current product name: {row}"
        );
    }

    let (status, body) = get_json(&rp, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn wrong_category_is_404_before_any_load() {
    // The RP listener must refuse a WORK model with 404 and create no job.
    let (control, _, rp, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("work-model", "mock", CategoryId::Work, Some(4.0))],
    );

    let (status, body) = post_json(&rp, "/v1/chat/completions", r#"{"model":"work-model"}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("WORK listener"));

    let (_, status) = get_json(&control, "/har/jobs").await;
    assert_eq!(
        status["jobs"].as_array().map(Vec::len),
        Some(0),
        "no ensure may run"
    );
    assert_eq!(runtime.server_count(), 0, "no engine may spawn");
}

#[tokio::test]
async fn the_job_ledger_names_the_admission_rung() {
    // B6 wire contract: an admitted load row carries `path` — the rung of
    // the making-room ladder the gate took — on `/har/jobs`, on
    // `/har/jobs/{id}`, and inside the `jobs` array of `/har/status`; a row
    // that never earned a rung omits the key, never sends null. The unit
    // pins in `src/scheduler/mod.rs` hold the per-door derivation and the
    // log field name; this pins the served bytes.
    // Falsifier: drop the `job.path` assignment in `finish_admit` and leg 1
    // finds no `path` on the load row; serialize `path` without
    // `skip_serializing_if` and leg 2 reads `"path": null` on the unload row.
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK, "load: {body}");

    let (_, jobs) = get_json(&control, "/har/jobs").await;
    let row = jobs["jobs"]
        .as_array()
        .expect("jobs array")
        .iter()
        .find(|job| job["model"] == "qwen3" && job["kind"] == "Load")
        .expect("the load row is on the ledger");
    assert_eq!(
        row["path"], "FitsNow",
        "the roomy ledger admits on the cheapest rung: {row}"
    );
    let job_id = row["id"].as_u64().expect("the row carries its id");
    let (_, one) = get_json(&control, &format!("/har/jobs/{job_id}")).await;
    assert_eq!(one["job"]["path"], "FitsNow", "the single-row door agrees");
    let (_, doc) = get_json(&control, "/har/status").await;
    let embedded = doc["jobs"]
        .as_array()
        .expect("jobs embedded in /har/status")
        .iter()
        .find(|job| job["id"].as_u64() == Some(job_id))
        .expect("the same row inside /har/status");
    assert_eq!(embedded["path"], "FitsNow", "{embedded}");

    // An unload row never stands before the gate: the field is absent on
    // the wire, not a null the client must special-case.
    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK, "unload accepted: {body}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let ledger = loop {
        let (_, jobs) = get_json(&control, "/har/jobs").await;
        let rows = jobs["jobs"].as_array().expect("jobs array");
        if rows.iter().any(|job| job["kind"] == "Unload") {
            break rows.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the unload never reached the ledger: {jobs}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let unload = ledger
        .iter()
        .find(|job| job["kind"] == "Unload")
        .expect("the unload row");
    assert!(
        unload.get("path").is_none(),
        "a row with no rung omits the field: {unload}"
    );
    let load = ledger
        .iter()
        .find(|job| job["id"].as_u64() == Some(job_id))
        .expect("the load row is still listed");
    assert_eq!(
        load["path"], "FitsNow",
        "the rung describes the decision the load made, whatever came after: {load}"
    );
}

#[tokio::test]
async fn rescan_picks_up_new_model_files() {
    let (control, work, _, _, _, modelconf) = app(
        AppConfig::default(),
        &[("first", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, body) = get_json(&work, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(Vec::len), Some(1));

    std::fs::write(
        modelconf.join("second.yaml"),
        "name: second
engine: mock
category: WORK
vram_gb: 4
",
    )
    .expect("write second model");
    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["models"], 2);

    let (_, body) = get_json(&work, "/v1/models").await;
    assert_eq!(body["data"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn control_page_serves_assets() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let (status, _) = get_json_html(&control, "/").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get_json_html(&control, "/style.css").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get_json_html(&control, "/app.js").await;
    assert_eq!(status, StatusCode::OK);
}

async fn get_json_html(app_router: &Router, path: &str) -> (StatusCode, String) {
    let response = app_router
        .clone()
        .oneshot(
            Request::get(path)
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn har_models_lists_the_full_catalog() {
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[
            ("work-model", "mock", CategoryId::Work, Some(4.0)),
            ("embed-model", "mock", CategoryId::Emb, Some(2.0)),
        ],
    );
    let (status, body) = get_json(&control, "/har/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn extended_logging_toggle_round_trips_and_validates() {
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, body) = post_json(&control, "/har/config/logging", r#"{"enabled":"yes"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let (status, _) = post_json(&control, "/har/config/logging", r#"{"enabled":true}"#).await;
    assert_eq!(status, StatusCode::OK);
    let (_, doc) = get_json(&control, "/har/status").await;
    assert_eq!(doc["extended_logging"], true);
    let (status, _) = post_json(&control, "/har/config/logging", r#"{"enabled":false}"#).await;
    assert_eq!(status, StatusCode::OK);
    let (_, doc) = get_json(&control, "/har/status").await;
    assert_eq!(doc["extended_logging"], false);
}

#[tokio::test]
async fn events_stream_serves_sse_with_an_immediate_state() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let response = control
        .clone()
        .oneshot(
            Request::get("/har/events")
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "{content_type}"
    );

    // The first frame must arrive without waiting for a mutation.
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        use tokio_stream::StreamExt;
        stream.next().await
    })
    .await
    .expect("first frame within 5s")
    .expect("stream alive")
    .expect("frame is bytes");
    let text = String::from_utf8_lossy(&first);
    assert!(text.contains("har_state"), "frame: {text}");
    // The tokenless page renders ONLY this frame, so its document must be the
    // whole page state: every `/har/status` key plus the model table. Dropping
    // either leg silently empties the table for SSE-only clients.
    let payload = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("frame carries a data line");
    let frame: serde_json::Value = serde_json::from_str(payload).expect("frame data is JSON");
    let (_, status) = get_json(&control, "/har/status").await;
    let mut want: Vec<&str> = status
        .as_object()
        .expect("status is an object")
        .keys()
        .map(String::as_str)
        .collect();
    want.push("models");
    want.sort_unstable();
    want.dedup();
    let got: Vec<&str> = frame
        .as_object()
        .expect("frame is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(got, want, "SSE frame drifted from the /har/status document");
}

#[tokio::test]
async fn guards_refuse_rebinding_hosts_and_foreign_origins() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);

    // DNS-rebinding guard: a hostname in Host is refused 400 before any handler.
    let response = control
        .clone()
        .oneshot(
            Request::get("/har/status")
                .header("host", "evil.example.com")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Loopback shapes pass.
    for host in ["127.0.0.1:8708", "localhost:8708", "[::1]:8708"] {
        let response = control
            .clone()
            .oneshot(
                Request::get("/har/status")
                    .header("host", host)
                    .body(Body::empty())
                    .expect("static request"),
            )
            .await
            .expect("in-process request");
        assert_eq!(response.status(), StatusCode::OK, "host {host}");
    }

    // CSRF guard: a cross-site Origin on POST is refused 403. This assertion used to
    // read `StatusCode::CONFLICT`, which agreed with the implementation and quietly
    // overruled both the guard's documented contract and this comment; 409 tells a
    // client the state may change so retrying is reasonable, and a security refusal
    // must not say that (T13 is what caught it, 2026-10-27).
    let response = control
        .clone()
        .oneshot(
            Request::post("/har/unload")
                .header("host", "127.0.0.1:8708")
                .header("origin", "http://evil.example.com")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let body: Value = serde_json::from_slice(&bytes).expect("error body");
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    assert_eq!(body["code"], "forbidden", "flat code: {body}");
    assert_eq!(
        body["error"]["code"], "forbidden",
        "the OpenAI envelope carries the same code"
    );

    // FINDINGS-18 [3]: the same-authority arm. Browsers send `Origin` on SAME-origin
    // non-GET requests too, so a guard that refused every Origin-bearing POST — e.g.
    // comparing Origin against a portless Host — 403s every button on the control
    // page (unload, load, rescan, default picker) while every refusal-direction test
    // stays green. This is the only test anywhere that POSTs with a MATCHING Origin:
    // a valid unload-all (`{}`, nothing loaded) with Origin == Host must be ACCEPTED,
    // pinned to the exact code rather than `!= FORBIDDEN`.
    let same_origin = control
        .oneshot(
            Request::post("/har/unload")
                .header("host", "127.0.0.1:8708")
                .header("origin", "http://127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(
        same_origin.status(),
        StatusCode::OK,
        "a same-authority Origin must pass the CSRF guard, not 403 the operator's page"
    );
}

#[tokio::test]
async fn posts_without_json_content_type_are_415() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let response = control
        .oneshot(
            Request::post("/har/unload")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "text/plain")
                .body(Body::from("{}"))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn request_ids_are_echoed_or_generated() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);

    // Client-provided id is echoed.
    let response = control
        .clone()
        .oneshot(
            Request::get("/har/status")
                .header("host", "127.0.0.1:8708")
                .header("x-request-id", "my-trace-1")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(
        response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("my-trace-1")
    );

    // Absent id is generated (req-<epoch>-<counter> shape).
    let response = control
        .clone()
        .oneshot(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(id.starts_with("req-"), "generated id: {id}");

    // FINDINGS-05 [7]: the echo above is the useful half; the wall is that an
    // oversized id, or one carrying a space or a tab, is regenerated rather than
    // echoed — the ring and the response may not be an attacker's echo chamber.
    // Falsifier (pre-fix): every leg below echoes verbatim. The same values are
    // pinned into the `request complete` ring line in-crate by
    // `client_request_ids_are_capped_and_sanitized_before_echo_and_log`.
    for rejected in [
        "a".repeat(65),
        "has space".to_string(),
        "tab\there".to_string(),
    ] {
        let response = control
            .clone()
            .oneshot(
                Request::get("/har/status")
                    .header("host", "127.0.0.1:8708")
                    .header("x-request-id", &rejected)
                    .body(Body::empty())
                    .expect("static request"),
            )
            .await
            .expect("in-process request");
        let id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            id.starts_with("req-") && id != rejected,
            "a {rejected:?} id must regenerate, not echo: {id:?}"
        );
    }
}

/// FINDINGS-05 [3]: `har_config_logging` was the only POST handler taking a bare
/// `Json<Value>`, so a malformed body answered axum's default 400 `text/plain`
/// and broke the documented `{code,message}` + OpenAI-envelope contract on that
/// route — nothing could branch on a missing `code`. Falsifier: revert the
/// handler to `Json(body): Json<Value>` and the JSON parse dies on the
/// text/plain sentence.
#[tokio::test]
async fn malformed_logging_body_answers_the_project_envelope() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    for malformed in ["{", "{\"enabled\":", ""] {
        let response = control
            .clone()
            .oneshot(
                Request::post("/har/config/logging")
                    .header("host", "127.0.0.1:8708")
                    .header("content-type", "application/json")
                    .body(Body::from(malformed))
                    .expect("static request"),
            )
            .await
            .expect("in-process request");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bounded body");
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "malformed {malformed:?}: {text}"
        );
        assert!(
            content_type.starts_with("application/json"),
            "malformed {malformed:?} must answer the JSON envelope, not text/plain: {content_type} / {text}"
        );
        let body: Value = serde_json::from_slice(&bytes).expect("envelope JSON");
        assert_eq!(body["code"], "bad_request");
        assert_eq!(body["error"]["code"], "bad_request");
        assert_eq!(body["error"]["message"], body["message"]);
    }
}

/// FINDINGS-05 [4]: `loopback_host` accepted every IP literal and threw the port
/// away (`rsplit_once(':')`), so `Host: 203.0.113.9:8708`, `Host: 0.0.0.0:8708`
/// and any wrong port on any listener passed the DNS-rebinding gate — looser
/// than the guard's own doc. Falsifier (pre-batch): every BAD_REQUEST leg below
/// answers 200. Falsifier (FINDINGS-16 N[1]): the `0.0.0.0`-bind OK leg answers
/// 400 against a gate keyed on `Host == internal_host` instead of the
/// unspecified-bind rule, so the suite sees the real published posture.
#[tokio::test]
async fn the_host_gate_closes_on_address_and_port_not_just_syntax() {
    async fn probe(router: &Router, host: &str) -> StatusCode {
        router
            .clone()
            .oneshot(
                Request::get("/har/status")
                    .header("host", host)
                    .body(Body::empty())
                    .expect("static request"),
            )
            .await
            .expect("in-process request")
            .status()
    }
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    // A wrong port on the right address: the port the request was addressed to is
    // part of "this harness", not decoration the guard may drop.
    assert_eq!(
        probe(&control, "127.0.0.1:9999").await,
        StatusCode::BAD_REQUEST
    );
    // Routable and unspecified addresses, even on a real port.
    assert_eq!(
        probe(&control, "203.0.113.9:8708").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        probe(&control, "0.0.0.0:8708").await,
        StatusCode::BAD_REQUEST
    );
    // Non-regression: the whole loopback family — bare and bracketed v6, with or
    // without a port from the configured set.
    for host in [
        "127.0.0.1",
        "127.0.0.2:8711",
        "localhost:8709",
        "[::1]",
        "[0:0:0:0:0:0:0:1]:8710",
    ] {
        assert_eq!(
            probe(&control, host).await,
            StatusCode::OK,
            "host {host} is this harness"
        );
    }
    // The publish posture (FINDINGS-16 N[1]/N[2]): `0.0.0.0` is the only
    // non-loopback bind boot validation allows and the operator's live shape, so
    // the legs use the real boot-valid configs, never a hand-built unicast bind.
    // The same LAN Host must stay 400 on the loopback bind and answer 200 on the
    // publish bind; hostnames and foreign ports stay refused on both.
    assert_eq!(
        probe(&control, "192.168.1.23:8708").await,
        StatusCode::BAD_REQUEST,
        "a loopback bind never answers a LAN-addressed Host"
    );
    let published = AppConfig {
        internal_host: "0.0.0.0".parse().expect("publish bind"),
        ..AppConfig::default()
    };
    let (published_control, _, _, _, _, _) = app(published, &[]);
    assert_eq!(
        probe(&published_control, "192.168.1.23:8708").await,
        StatusCode::OK,
        "the published posture (guard is the floor) answers its LAN clients"
    );
    assert_eq!(
        probe(&published_control, "192.168.1.23:9999").await,
        StatusCode::BAD_REQUEST,
        "the publish widening does not excuse a foreign port"
    );
    assert_eq!(
        probe(&published_control, "evil.example:8708").await,
        StatusCode::BAD_REQUEST,
        "a hostname Host stays refused even on the publish bind"
    );
}

/// FINDINGS-05 [5]: the Host gate ran only `if let Some(host)`, so a request
/// carrying no `Host` at all skipped the DNS-rebinding gate instead of being
/// refused by it — a conditional floor is not a floor, and the published-host
/// posture is documented as relying on the guard, not on a stack property.
/// Falsifier (pre-fix): every leg below answers 200/401 instead of 400.
#[tokio::test]
async fn a_request_without_a_host_header_is_refused_not_skipped() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let response = control
        .clone()
        .oneshot(
            Request::get("/har/status")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let body: Value = serde_json::from_slice(&bytes).expect("refusal envelope");
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], "bad_request");
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Host"),
        "the refusal must name the header it needs: {body}"
    );

    // Structural before token (DECISIONS LOCKED): the right token does not excuse
    // a request the guard cannot inspect.
    let config = AppConfig {
        control_token: Some("s3cret".to_string()),
        ..AppConfig::default()
    };
    let (guarded, _, _, _, _, _) = app(config, &[]);
    let response = guarded
        .oneshot(
            Request::post("/har/config/logging")
                .header("content-type", "application/json")
                .header("x-har-token", "s3cret")
                .body(Body::from(r#"{"enabled":false}"#))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// FINDINGS-05 [6]: `error_body` passed the `Internal`/`Upstream` reasons through
/// verbatim, and those reasons embed absolute paths (model yaml, log file,
/// engine exe), so a published harness with no token told any host on the
/// segment its install tree. The full sentence now goes to `tracing::error!` —
/// the ring and the log file — and the client keeps only the stable class
/// sentence. Falsifier (pre-fix): no captured line carries the OS sentence at
/// all, and the wire body carries it (the 500 leg of
/// `config_model_reports_writeback_refusals_by_http_class` pins that half).
#[tokio::test]
async fn os_failure_text_reaches_the_ring_but_never_the_client_body() {
    let capture = capture_lines();
    // The reporting callsite must deliver into this subscriber (D9): warm it on a
    // throwaway app whose model name no assertion reads, rebuild cached interest,
    // THEN take the measurement.
    let (warm, _, _, _, _, warm_dir) = build_app(
        AppConfig::default(),
        &[("warmer", "mock", CategoryId::Work, Some(4.0))],
        &[],
    );
    let (_, published) = get_json(&warm, "/har/models").await;
    assert!(
        published["data"]
            .as_array()
            .expect("list")
            .iter()
            .any(|entry| entry["id"] == "warmer"),
        "the warm row must be published before its file moves: {published}"
    );
    std::fs::remove_file(warm_dir.join("warmer.yaml")).expect("warm: delete under published row");
    post_json(
        &warm,
        "/har/config/model",
        r#"{"model":"warmer","category":"AUX"}"#,
    )
    .await;
    tracing::callsite::rebuild_interest_cache();

    let (control, _, _, _, _, modelconf) = build_app(
        AppConfig::default(),
        &[("osleak", "mock", CategoryId::Work, Some(4.0))],
        &[],
    );
    let (_, published) = get_json(&control, "/har/models").await;
    assert!(
        published["data"]
            .as_array()
            .expect("list")
            .iter()
            .any(|entry| entry["id"] == "osleak"),
        "the row must be published before the file moves: {published}"
    );
    std::fs::remove_file(modelconf.join("osleak.yaml")).expect("delete under published row");
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"osleak","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {body}");
    assert_eq!(body["code"], "internal");
    assert_eq!(body["error"]["code"], "internal");
    assert_eq!(body["message"], body["error"]["message"]);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("internal error:"),
        "the stable sentence keeps the class: {body}"
    );
    for leaked in [
        "cannot read",
        "osleak.yaml",
        modelconf.to_str().unwrap_or("?"),
    ] {
        assert!(
            !body.to_string().contains(leaked),
            "the client body may not carry {leaked:?}: {body}"
        );
    }
    // The operator's channel holds the whole truth the socket no longer sees.
    let lines = capture.lock().expect("capture lock");
    assert!(
        lines.iter().any(|line| line.contains("cannot read")
            && line.contains("osleak.yaml")
            && line.contains("detail")),
        "the full OS reason must reach the harness log, not the client: {lines:?}"
    );
}

/// The generic `tower-http` failure line is diagnostic only; a named admission
/// refusal is already the harness's WARN and must not be duplicated at ERROR.
/// Falsifier: the default `TraceLayer` failure handler (ERROR) leaves an
/// `ERROR tower_http::trace::on_failure` line for this 503 and the second pin
/// below goes red.
#[tokio::test]
async fn a_named_503_refusal_is_not_an_error_on_the_http_trace() {
    let capture = capture_lines();
    tracing::callsite::rebuild_interest_cache();
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[("too-big", "mock", CategoryId::Work, Some(1000.0))],
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"too-big"}"#).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    let lines = capture.lock().expect("capture lock");
    assert!(
        lines.iter().any(|line| line.contains("admission refused")),
        "the named refusal must stay visible in its WARN: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("ERROR") && line.contains("tower_http::trace::on_failure")),
        "the generic trace failure must not duplicate a named 503 at ERROR: {lines:?}"
    );
}

/// FINDINGS-05 [8]: the `Bearer` scheme was stripped byte-exactly, so an
/// OpenAI-style client sending `authorization: bearer <token>` got a 401 that
/// read as a wrong token; RFC 7235 makes auth-schemes case-insensitive. The
/// constant-time half of the finding is pinned against the shipped source
/// in-crate (`the_token_wall_is_constant_time_and_scheme_case_insensitive`);
/// this is the wire matrix — any scheme case passes, a same-length wrong token
/// and a different-length one are both refused, and a bare token still passes.
#[tokio::test]
async fn bearer_scheme_is_accepted_in_any_case_and_only_the_right_token_passes() {
    let config = AppConfig {
        control_token: Some("s3cret-token".to_string()),
        ..AppConfig::default()
    };
    let (control, _, _, _, _, _) = app(config, &[]);
    for scheme in ["Bearer", "bearer", "BEARER", "BeArEr"] {
        let response = control
            .clone()
            .oneshot(
                Request::get("/har/models")
                    .header("host", "127.0.0.1:8708")
                    .header("authorization", format!("{scheme} s3cret-token"))
                    .body(Body::empty())
                    .expect("static request"),
            )
            .await
            .expect("in-process request");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "scheme {scheme} is RFC-valid and must be accepted"
        );
    }
    // Wrong stays wrong in both directions: one byte flipped at equal length (the
    // fold decides) and a different length (the length gate decides).
    for wrong in ["s3cret-tokeN", "s3cret", "s3cret-token-longer"] {
        let response = control
            .clone()
            .oneshot(
                Request::get("/har/models")
                    .header("host", "127.0.0.1:8708")
                    .header("x-har-token", wrong)
                    .body(Body::empty())
                    .expect("static request"),
            )
            .await
            .expect("in-process request");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "wrong token {wrong:?} must be refused"
        );
    }
    // A bare token with no scheme still passes, as the page sends it.
    let response = control
        .oneshot(
            Request::get("/har/models")
                .header("host", "127.0.0.1:8708")
                .header("x-har-token", "s3cret-token")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn har_log_serves_the_ring() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    // Push a marker entry straight into the ring (the test binary has no global
    // subscriber; the endpoint reads the same process-global storage).
    simpleharn::observability::ring_push(simpleharn::observability::LogEntry::now(
        "ERROR",
        "test::har_log",
        "last load failed: need 50 GiB",
    ));
    let (status, body) = get_json(&control, "/har/log?limit=10").await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("events list");
    assert!(events.iter().any(|entry| {
        entry["message"]
            .as_str()
            .unwrap_or_default()
            .contains("last load failed")
    }));
    assert!(body["capacity"].is_u64());
}

/// Sends one GET with the loopback `Host` and hands back the whole response, so
/// a leg can read headers the `get_json` helper drops.
async fn get_full(app: &Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(path)
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request")
}

/// Checks one refusal leg: status, JSON content type, the guard's request id,
/// and the dual envelope with `want_code` in both shapes. Returns the body.
async fn refusal_envelope(
    response: axum::response::Response,
    want_status: StatusCode,
    want_code: &str,
) -> Value {
    assert_eq!(response.status(), want_status);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("application/json"),
        "the refusal must answer the JSON envelope, not {content_type:?}"
    );
    assert!(
        response.headers().contains_key("x-request-id"),
        "the envelope rewrite must keep the guard's request id: {:?}",
        response.headers()
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let body: Value = serde_json::from_slice(&bytes).expect("envelope JSON");
    assert_eq!(body["code"], want_code, "flat envelope: {body}");
    assert_eq!(body["error"]["code"], want_code, "OpenAI envelope: {body}");
    assert_eq!(body["error"]["type"], "simpleharn_error", "{body}");
    assert_eq!(body["error"]["message"], body["message"], "{body}");
    body
}

/// R2-05-03: a route the harness does not know answers the typed envelope on
/// every listener. Pre-fix the control listener answered the webui fallback's
/// empty body and a category listener answered axum's `text/plain` 404 —
/// nothing could branch on a missing `code`. Falsifier: drop the
/// `envelope_router_errors` layer and every leg dies in `refusal_envelope`.
#[tokio::test]
async fn unknown_routes_answer_the_envelope_on_every_listener() {
    let (control, work, _, _, _, _) = app(AppConfig::default(), &[]);
    let body = refusal_envelope(
        get_full(&control, "/har/definitely-not-a-route").await,
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;
    assert_eq!(
        body["message"], "not_found: no route for GET /har/definitely-not-a-route",
        "{body}"
    );
    let body = refusal_envelope(
        get_full(&work, "/nope").await,
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;
    assert_eq!(
        body["message"], "not_found: no route for GET /nope",
        "{body}"
    );
}

/// R2-05-03/04: a known route with the wrong method answers `405
/// method_not_allowed` in the same envelope, and the router's `Allow` header
/// survives the rewrite. The guard's POST content-type gate runs before
/// routing, so the `POST /health` leg declares JSON exactly as the page would.
#[tokio::test]
async fn wrong_methods_answer_the_envelope_and_keep_the_allow_header() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let body = refusal_envelope(
        get_full(&control, "/har/load").await,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
    )
    .await;
    assert_eq!(
        body["message"], "method_not_allowed: GET /har/load is not accepted",
        "{body}"
    );
    let response = control
        .oneshot(
            Request::post("/health")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::empty())
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let allow = response
        .headers()
        .get("allow")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = refusal_envelope(
        response,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
    )
    .await;
    assert_eq!(
        body["message"], "method_not_allowed: POST /health is not accepted",
        "{body}"
    );
    assert_eq!(
        allow, "GET,HEAD",
        "the router's Allow list survives: {body}"
    );
}

/// R2-05-03: `har_log` extracts `Result<Query, QueryRejection>` and answers a
/// rejection with the typed `400 bad_request` envelope, never axum's plain-text
/// one. Honest scope: on axum 0.8.9 that branch is defensive. The form decoder
/// passes a bad escape through and replaces invalid UTF-8 (`decode_utf8_lossy`),
/// and a `HashMap<String, String>` map accepts every pair, so no wire input
/// makes the extractor reject — `%zz` stays a key and `%ff` becomes one
/// replacement character. These legs pin that such inputs keep answering the
/// ring document with the `limit` fallback. A decoder that rejects in the
/// future must answer the envelope (the `Result` arm); the legs then move to
/// the `400` shape, they must not silently return to `text/plain`.
/// FR-05 (2026-11-06): `limit=abc` left this lenient list — a malformed
/// `limit` is now a live handler refusal, `400 bad_request` in the project
/// envelope (pinned in `src/api/tests.rs`), no longer a swallowed default.
#[tokio::test]
async fn a_bad_query_on_har_log_stays_inside_the_typed_surface() {
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    for query in ["%zz", "%ff"] {
        let (status, body) = get_json(&control, &format!("/har/log?{query}")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "`{query}` decodes leniently: {body}"
        );
        assert!(body["events"].is_array(), "ring document answers: {body}");
        assert!(body["capacity"].is_u64(), "{body}");
    }
}
