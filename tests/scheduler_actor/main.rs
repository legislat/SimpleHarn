//! Scheduler-actor integration tests against the in-process mock engine.
//! Each test pins one defect class: client abort, double load, rapid A->B
//! supersede, cancel mid-load, control plane behind a load, plus
//! the admission and supersede contracts.

#[path = "../mock_runtime.rs"]
mod mock_runtime;
#[path = "../port_plan.rs"]
mod port_plan;

use mock_runtime::MockRuntime;
use port_plan::reserve_engine_plan;
use simpleharn::budget::{MemorySample, VramWitness};
use simpleharn::config::{AppConfig, CategoryId};
use simpleharn::engine::llama::LlamaAdapter;
use simpleharn::engine::mock::MockAdapter;
use simpleharn::engine::ninfer::NinferAdapter;
use simpleharn::engine::strata::StrataAdapter;
use simpleharn::engine::EngineAdapter;
use simpleharn::scheduler::{
    booking_gibs, JobId, JobKind, Reply, Scheduler, SchedulerError, SchedulerHandle,
    SchedulerStatus, SwapState,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn config(load_timeout_s: u64) -> AppConfig {
    let mut config = AppConfig {
        load_timeout_s,
        ..AppConfig::default()
    };
    // Parallel schedulers must never share a planned engine port: the spawn
    // probe binds it, and two schedulers on one number refuse each other.
    reserve_engine_plan(&mut config);
    config
}

type Spec = (&'static str, &'static str, CategoryId, Option<f64>);
type Spec5 = (
    &'static str,
    &'static str,
    CategoryId,
    Option<f64>,
    &'static str,
);

fn to5(specs: &[Spec]) -> Vec<Spec5> {
    specs
        .iter()
        .map(|(name, engine, category, vram)| (*name, *engine, *category, *vram, ""))
        .collect()
}

/// Writes flat model yamls into a fresh temp ModelConf dir; returns its path.
/// The 5th spec element appends raw extra lines (cost:, floor keys...).
fn write_modelconf5(specs: &[Spec5]) -> std::path::PathBuf {
    let dir = simpleharn::testutil::temp_dir("modelconf");
    for (name, engine, category, vram_gb, extra) in specs {
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
        if !extra.is_empty() {
            yaml.push_str(extra);
            if !extra.ends_with('\n') {
                yaml.push('\n');
            }
        }
        std::fs::write(dir.join(format!("{name}.yaml")), yaml).expect("model yaml");
    }
    dir
}

fn harness5(cfg: AppConfig, specs: &[Spec5]) -> (SchedulerHandle, MockRuntime) {
    harness5_witness(cfg, specs, Arc::new(simpleharn::budget::LedgerOnlyWitness))
}

/// `harness5` against a caller-supplied witness, for the cases that must control when
/// an admission decision is allowed to land.
fn harness5_witness(
    mut cfg: AppConfig,
    specs: &[Spec5],
    witness: Arc<dyn VramWitness>,
) -> (SchedulerHandle, MockRuntime) {
    // Catch every entry path, including configs a test built inline rather than
    // through `config`: categories still on the automatic plan get this
    // fixture's own reserved ports, an already-reserved plan stays untouched.
    reserve_engine_plan(&mut cfg);
    let runtime = MockRuntime::new();
    let mut adapters = BTreeMap::new();
    let adapter: Arc<dyn simpleharn::engine::EngineAdapter> = Arc::new(MockAdapter);
    adapters.insert("mock".to_string(), adapter);
    for engine in [
        "mock-slow",
        "mock-dies",
        "mock-dies-once",
        "mock-never",
        "mock-hold-kill",
        "mock-hold-never",
    ] {
        adapters.insert(
            engine.to_string(),
            Arc::new(MockAdapter) as Arc<dyn simpleharn::engine::EngineAdapter>,
        );
    }
    // The real adapters, under the ids they themselves serve: a launch validation
    // refusal must come from production code, not from a mock that accepts anything.
    let reals: Vec<Arc<dyn EngineAdapter>> = vec![
        Arc::new(StrataAdapter::default()),
        Arc::new(NinferAdapter::new("Ubuntu", "ops")),
        Arc::new(LlamaAdapter::new(PathBuf::from("llama-server.exe"))),
    ];
    for adapter in reals {
        adapters.insert(adapter.engine_id().to_string(), adapter);
    }
    let handle = Scheduler::start(
        cfg,
        write_modelconf5(specs),
        adapters,
        Arc::new(runtime.clone()) as Arc<dyn simpleharn::engine::EngineRuntime>,
        witness,
    );
    (handle, runtime)
}

fn harness(cfg: AppConfig, specs: &[Spec]) -> (SchedulerHandle, MockRuntime) {
    harness5(cfg, &to5(specs))
}

/// Polls `cond` against status snapshots until it holds, or fails the test.
async fn wait_until(
    handle: &SchedulerHandle,
    secs: u64,
    cond: impl Fn(&simpleharn::scheduler::SchedulerStatus) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let status = handle.status().await.expect("actor alive");
        if cond(&status) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "condition not met within {secs}s; last status: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Submits a load and vanishes: the command reaches the actor, the reply is discarded —
/// the true client-abort shape (the dropped waiter must not cancel the load).
async fn submit_and_vanish(handle: &SchedulerHandle, model: &str, category: CategoryId) {
    let client = handle.clone();
    let model = model.to_string();
    tokio::spawn(async move {
        let _ = client.swap(&model, category).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn slot_model(
    status: &simpleharn::scheduler::SchedulerStatus,
    category: CategoryId,
) -> Vec<Option<String>> {
    status
        .slots
        .iter()
        .filter(|s| s.category == category)
        .map(|s| s.model.clone())
        .collect()
}

fn terminal_jobs(
    status: &simpleharn::scheduler::SchedulerStatus,
) -> Vec<&simpleharn::scheduler::JobStatus> {
    status.jobs.iter().filter(|j| j.terminal).collect()
}

fn slot_state(
    status: &simpleharn::scheduler::SchedulerStatus,
    category: CategoryId,
) -> Vec<String> {
    status
        .slots
        .iter()
        .filter(|s| s.category == category)
        .map(|s| s.state.clone())
        .collect()
}

mod cancel_swap;
mod chaos_misc;
mod cost_floor;
mod launch_validation;
mod load_supersede;
mod parked_swap;
mod release_timeout;
mod rescan_drift;

/// The resident model of the cancel-mid-release scenarios: its engine parks inside
/// `kill` until the test releases it, so the swap's Dispose release is genuinely still
/// in flight when the operator's cancel lands.
const HOLD_MODEL: &str = "hold-a";
/// The model being swapped in over it.
const INCOMING_MODEL: &str = "model-b";

fn hold_kill_harness(cfg: AppConfig) -> (SchedulerHandle, MockRuntime) {
    harness5(
        cfg,
        &[
            (
                HOLD_MODEL,
                "mock-hold-kill",
                CategoryId::Work,
                Some(4.0),
                "",
            ),
            (INCOMING_MODEL, "mock", CategoryId::Work, Some(4.0), ""),
        ],
    )
}

/// Loads [`HOLD_MODEL`] into WORK and returns its engine's receipt.
async fn serve_hold_model(handle: &SchedulerHandle) -> simpleharn::scheduler::SwapReceipt {
    let Reply::Loaded(receipt) = handle
        .swap(HOLD_MODEL, CategoryId::Work)
        .await
        .expect("the resident model loads")
    else {
        panic!("expected Loaded, got the resident load")
    };
    receipt
}

/// Submits a slow load without keeping the waiter and returns its job id.
async fn spawn_and_find_job(handle: &SchedulerHandle) -> JobId {
    submit_and_vanish(handle, "qwen3", CategoryId::Work).await;
    wait_until(handle, 5, |s| {
        s.slots.iter().any(|slot| slot.loading_job.is_some())
    })
    .await;
    handle
        .status()
        .await
        .expect("actor alive")
        .slots
        .iter()
        .find_map(|slot| slot.loading_job)
        .expect("invariant: wait_until established a loading job")
}

/// A ledger-only witness the test can park. The admission decision waits on exactly
/// one `sample()`, so a gate with `samples` permits answers that many reads
/// and parks every later one: a load frozen in `Admit` is the state in which a cancel
/// can never prove its victim's release, which is the precondition both findings need.
/// Without it these scenarios would be a race against the witness rather than a state
/// the test holds.
struct GatedWitness {
    gate: Arc<tokio::sync::Semaphore>,
}

impl GatedWitness {
    /// Answers `samples` reads, then parks every later one.
    fn new(samples: usize) -> Self {
        GatedWitness {
            gate: Arc::new(tokio::sync::Semaphore::new(samples)),
        }
    }
}

#[async_trait::async_trait]
impl VramWitness for GatedWitness {
    fn source(&self) -> String {
        "gated-ledger".to_string()
    }

    fn ledger_only(&self) -> bool {
        true
    }

    async fn sample(&self) -> Result<MemorySample, String> {
        let permit = self
            .gate
            .acquire()
            .await
            .map_err(|_| "the test gate is closed".to_string())?;
        permit.forget();
        Err("witness unavailable (ledger-only mode)".to_string())
    }
}

/// Whether `model` is on the ledger as a queued load: registered, non-terminal, still
/// `Idle` because it has never been admitted.
fn is_queued(status: &SchedulerStatus, model: &str) -> bool {
    status.jobs.iter().any(|j| {
        j.model == model && j.kind == JobKind::Load && !j.terminal && j.state == SwapState::Idle
    })
}
