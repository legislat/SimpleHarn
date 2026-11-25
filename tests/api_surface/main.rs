//! API-surface integration tests (Steps 11-12): control routes, per-category
//! listeners, wrong-category 404-before-ensure, no-model fallback, proxy
//! pass-through — all against the in-process mock engine.
//!
//! Crate root of the `api_surface` target: the shared builders and helpers
//! live here, the scenario pins live in the sibling modules below. The mock
//! runtime is shared with other integration-test files, so it stays at
//! `tests/mock_runtime.rs` and is declared by path.

#[path = "../mock_runtime.rs"]
mod mock_runtime;

mod catalog_retag;
mod config_writeback;
mod control_plane;
mod proxy_admission;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use mock_runtime::MockRuntime;
use serde_json::Value;
use simpleharn::api::{category_router, control_router, ApiState};
use simpleharn::config::{AppConfig, CategoryId};
use simpleharn::engine::mock::MockAdapter;
use simpleharn::engine::{EngineAdapter, EngineRuntime};
use simpleharn::scheduler::Scheduler;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

type Spec = (&'static str, &'static str, CategoryId, Option<f64>);

/// The per-test stand-in for the boot config file (FINDINGS-18 [2]): `app()` points
/// every `AppConfig` at `<modelconf dir>/simpleharn.json` so the persisting
/// endpoints (`/har/config/default`, the retag default-clear) write INSIDE the test's
/// own temp dir. Before this they fell through to `default_config_path()` — the exe
/// dir — so `cargo test` wrote a file the product reads at boot, outside any sandbox,
/// and no test could read back what the endpoint had written.
const TEST_CONFIG_NAME: &str = "simpleharn.json";

fn write_modelconf(specs: &[Spec]) -> std::path::PathBuf {
    let dir = simpleharn::testutil::temp_dir("api-modelconf");
    for (name, engine, category, vram_gb) in specs {
        let mut yaml = format!(
            "name: {name}
engine: {engine}
category: {category}
"
        );
        if let Some(vram) = vram_gb {
            yaml.push_str(&format!(
                "vram_gb: {vram}
"
            ));
        }
        std::fs::write(dir.join(format!("{name}.yaml")), yaml).expect("model yaml");
    }
    dir
}

/// [`app`] with extra engine adapters (ids `mock-a`/`mock-b` carrying distinct
/// `/v1/models` ids, `mock-nodiscovery`, …).
#[allow(clippy::type_complexity)]
fn build_app(
    mut config: AppConfig,
    specs: &[Spec],
    extra_adapters: &[(&str, std::sync::Arc<dyn EngineAdapter>)],
) -> (
    Router,
    Router,
    Router,
    Router,
    MockRuntime,
    std::path::PathBuf,
) {
    let runtime = MockRuntime::new();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert("mock".to_string(), Arc::new(MockAdapter));
    adapters.insert("mock-error".to_string(), Arc::new(MockAdapter));
    adapters.insert("mock-never".to_string(), Arc::new(MockAdapter));
    adapters.insert("mock-slow".to_string(), Arc::new(MockAdapter));
    adapters.insert("mock-dies-once".to_string(), Arc::new(MockAdapter));
    adapters.insert("mock-dies".to_string(), Arc::new(MockAdapter));
    adapters.insert("mock-nodiscovery".to_string(), Arc::new(MockAdapter));
    for (id, adapter) in extra_adapters {
        adapters.insert((*id).to_string(), Arc::clone(adapter));
    }
    let dir = write_modelconf(specs);
    // A hand-stamped `source_path` (the custom-path persist test) is respected;
    // everything else is sandboxed to this test's temp dir (FINDINGS-18 [2]).
    if config.source_path.is_none() {
        config.source_path = Some(dir.join(TEST_CONFIG_NAME));
    }
    let scheduler = Scheduler::start(
        config.clone(),
        dir.clone(),
        adapters,
        Arc::new(runtime.clone()) as Arc<dyn EngineRuntime>,
        Arc::new(simpleharn::budget::LedgerOnlyWitness),
    );
    let state = Arc::new(ApiState::new(
        config,
        scheduler,
        simpleharn::listeners::ListenerRegistry::new(),
        Arc::new(simpleharn::budget::LedgerOnlyWitness),
        simpleharn::api::BootTimings::default(),
    ));
    let control = control_router(Arc::clone(&state));
    let work = category_router(Arc::clone(&state), CategoryId::Work);
    let rp = category_router(Arc::clone(&state), CategoryId::Rp);
    let aux = category_router(state, CategoryId::Aux);
    (control, work, rp, aux, runtime, dir)
}

fn app(
    config: AppConfig,
    specs: &[Spec],
) -> (
    Router,
    Router,
    Router,
    Router,
    MockRuntime,
    std::path::PathBuf,
) {
    build_app(config, specs, &[])
}

/// Every builder here carries a loopback `Host`: the guard's DNS-rebinding gate now
/// runs on every request (FINDINGS-05 [5]), and a `tower::ServiceExt::oneshot`
/// request carries no `Host` header on its own.
async fn post_json(app: &Router, path: &str, body: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn get_json(app: &Router, path: &str) -> (StatusCode, Value) {
    let response = app
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// An `AppConfig` whose AUX category holds `slots` occupants (WORK/RP/EMB stay at 1).
fn aux_slots(slots: usize) -> AppConfig {
    let mut config = AppConfig::default();
    config
        .category_mut(CategoryId::Aux)
        .expect("default config has AUX")
        .max_models = slots;
    config
}

/// Polls until `runtime` reports exactly `want` live engines, and returns the count it
/// ended on. An unload answers `Done` when its job is STARTED, not when the kill lands
/// (`unload_slots` sends the reply before the spawned release completes), so the live
/// count settles shortly after the handler returns.
async fn await_server_count(runtime: &MockRuntime, want: usize) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while runtime.server_count() != want && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    runtime.server_count()
}
