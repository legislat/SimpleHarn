//! Admission conformance (scripted witness): the invariant contract stated in one
//! named target. The five scenarios name the invariants that make admission the
//! product: a witness outage refuses before any spawn; in-flight bookings are
//! debited so two concurrent loads cannot spend the same GiB; a same-slot victim
//! is credited at `max(booking, measured footprint)`; eviction releases the
//! cheapest sufficient victim and spares the rest; a launch floor that no release
//! can reach refuses before any kill. Plus one API-level leg: a load against an
//! unreadable card answers 503 `witness_unavailable` in the typed envelope.
//!
//! Scope and limits (a thin conformance target over the existing proven tests,
//! not a new harness):
//!
//! - Everything runs cardless: a scripted witness and the in-process mock engine,
//!   no GPU, no model files, no private handoff repo, no port bind.
//! - Each scenario maps to the public test that already carries it; the mapping
//!   lives in the task evidence and beside each `#[tokio::test]`. New assertion
//!   weight exists only where the audit found a gap: the `max(booking, measured)`
//!   release credit was pinned only by unit tests inside `src/scheduler/tests.rs`,
//!   never through the public integration layer.
//! - The scripted witness is a projection, not hardware truth. This target never
//!   replaces the operator-run card suite (real NVML readings, the physical
//!   release timing of a real engine process, a real swap on a real card).
//! - The falsifier for the credit rule runs in a disposable worktree:
//!   `scripts/seed-admission-regression.ps1`. It flips the `max` in the victim
//!   release projection to `min` and asserts this target goes red.

mod mock_runtime;
mod port_plan;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use mock_runtime::MockRuntime;
use port_plan::reserve_engine_plan;
use simpleharn::api::{control_router, ApiState, BootTimings};
use simpleharn::budget::{LedgerOnlyWitness, MemorySample, VramWitness};
use simpleharn::config::{AppConfig, CategoryId};
use simpleharn::engine::mock::MockAdapter;
use simpleharn::engine::{EngineAdapter, EngineRuntime};
use simpleharn::scheduler::{
    JobStatus, Reply, Scheduler, SchedulerHandle, SchedulerStatus, SwapState,
};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;

/// Scripted witness: samples come off the queue (the last one repeats), an
/// explicit override always wins, and an explicit kill makes every sample error.
/// Same shape as `tests/scheduler_witness.rs::FakeWitness`.
#[derive(Clone, Default)]
struct FakeWitness {
    queue: Arc<Mutex<VecDeque<MemorySample>>>,
    override_now: Arc<Mutex<Option<MemorySample>>>,
    /// Once set, every sample errors: models a driver wedging mid-window.
    dead: Arc<AtomicBool>,
}

impl FakeWitness {
    fn with(samples: Vec<MemorySample>) -> Self {
        FakeWitness {
            queue: Arc::new(Mutex::new(samples.into())),
            override_now: Arc::new(Mutex::new(None)),
            dead: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set(&self, sample: MemorySample) {
        *self.override_now.lock().expect("witness override lock") = Some(sample);
    }
}

#[async_trait]
impl VramWitness for FakeWitness {
    fn source(&self) -> String {
        "fake".to_string()
    }

    async fn sample(&self) -> Result<MemorySample, String> {
        if self.dead.load(Ordering::Relaxed) {
            return Err("nvml died".to_string());
        }
        if let Some(sample) = self
            .override_now
            .lock()
            .expect("witness override lock")
            .clone()
        {
            return Ok(sample);
        }
        let mut queue = self.queue.lock().expect("witness queue lock");
        if queue.len() > 1 {
            Ok(queue.pop_front().expect("len checked"))
        } else {
            Ok(queue.front().cloned().unwrap_or(MemorySample {
                total_gib: 72.0,
                used_gib: 0.0,
                processes: vec![],
                attribution: simpleharn::budget::AttributionQuality::Unavailable,
            }))
        }
    }
}

/// A real witness that cannot answer and does not claim ledger mode: the card is
/// unreadable, so there is no honest number to budget against.
struct DeadWitness;

#[async_trait]
impl VramWitness for DeadWitness {
    fn source(&self) -> String {
        "dead".to_string()
    }

    async fn sample(&self) -> Result<MemorySample, String> {
        Err("nvml absent and nvidia-smi failed".to_string())
    }
}

fn sample(total: f64, used: f64) -> MemorySample {
    MemorySample {
        total_gib: total,
        used_gib: used,
        processes: vec![],
        attribution: simpleharn::budget::AttributionQuality::Unavailable,
    }
}

/// Starts the scheduler over the mock runtime with raw model yaml per model, so
/// a scenario can carry the `cost:` / `launch_only_with_at_least_vram_free:`
/// keys the plain per-model builder cannot. Returns the runtime handle too:
/// the kill/spawn counts falsify a "refused before any spawn" claim.
fn harness(
    mut cfg: AppConfig,
    specs: &[(&str, &str)],
    witness: Arc<dyn VramWitness>,
) -> (SchedulerHandle, MockRuntime, PathBuf) {
    reserve_engine_plan(&mut cfg);
    let runtime = MockRuntime::new();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert("mock".to_string(), Arc::new(MockAdapter));
    let dir = simpleharn::testutil::temp_dir("admission-conformance");
    for (name, yaml) in specs {
        std::fs::write(dir.join(format!("{name}.yaml")), yaml).expect("model yaml");
    }
    let handle = Scheduler::start(
        cfg,
        dir.clone(),
        adapters,
        Arc::new(runtime.clone()) as Arc<dyn EngineRuntime>,
        witness,
    );
    (handle, runtime, dir)
}

async fn wait_until(handle: &SchedulerHandle, secs: u64, cond: impl Fn(&SchedulerStatus) -> bool) {
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

/// The newest ledger row of `model`'s job.
fn job_row<'a>(status: &'a SchedulerStatus, model: &str) -> &'a JobStatus {
    status
        .jobs
        .iter()
        .rev()
        .find(|job| job.model == model)
        .unwrap_or_else(|| panic!("`{model}` must be in the job ledger: {:?}", status.jobs))
}

/// The measured pair currently on a catalog entry (the fold's persisted view).
async fn measured_of(handle: &SchedulerHandle, model: &str) -> (Option<f64>, Option<f64>) {
    let catalog = handle.catalog().await.expect("actor alive");
    catalog
        .models
        .iter()
        .find(|m| m.name == model)
        .map_or((None, None), |m| (m.measured_gb, m.measured_min_gb))
}

async fn await_measured(handle: &SchedulerHandle, model: &str, want: (Option<f64>, Option<f64>)) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = measured_of(handle, model).await;
        if got == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "range for {model} never reached {want:?}: got {got:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Invariant 1, witness outage. Carried publicly by
/// `scheduler_witness.rs::unreadable_card_refuses_rather_than_guesses`; stated
/// here as the named contract: a witness that cannot answer is a named refusal,
/// never a fallback onto bookkeeping.
#[tokio::test]
async fn a_dead_witness_refuses_before_any_spawn() {
    let cfg = AppConfig {
        settle_timeout_s: 1,
        ..AppConfig::default()
    };
    let (handle, _runtime, _dir) = harness(
        cfg,
        &[(
            "qwen3",
            "name: qwen3\nengine: mock\ncategory: WORK\nvram_gb: 4\n",
        )],
        Arc::new(DeadWitness),
    );

    let error = handle
        .swap("qwen3", CategoryId::Work)
        .await
        .expect_err("no sample, no load");
    let message = error.to_string();
    assert!(message.contains("witness unavailable"), "was: {message}");
    assert!(
        message.contains("nvml absent and nvidia-smi failed"),
        "the witness's own reason must travel: {message}"
    );

    let status = handle.status().await.expect("actor alive");
    assert!(
        status.slots.iter().all(|slot| slot.model.is_none()),
        "refused before spawn"
    );
    assert!(job_row(&status, "qwen3").state == SwapState::AdmitFail);
}

/// Invariant 1 at the API boundary: the scheduler refusal reaches the client as
/// 503 with the stable `witness_unavailable` code (the class a client branches
/// on, distinct from the over-commit `unavailable`). Driven through
/// `tower::ServiceExt::oneshot`, so no port is bound and no listener starts.
#[tokio::test]
async fn an_api_load_against_an_unreadable_card_answers_503_witness_unavailable() {
    let config = AppConfig::default();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert("mock".to_string(), Arc::new(MockAdapter));
    let modelconf = simpleharn::testutil::temp_dir("admission-conformance-api");
    std::fs::write(
        modelconf.join("qwen3.yaml"),
        "name: qwen3\nengine: mock\ncategory: WORK\nvram_gb: 4\n",
    )
    .expect("model yaml");
    let scheduler = Scheduler::start(
        config.clone(),
        modelconf,
        adapters,
        Arc::new(MockRuntime::new()) as Arc<dyn EngineRuntime>,
        Arc::new(DeadWitness),
    );
    let control = control_router(Arc::new(ApiState::new(
        config,
        scheduler,
        simpleharn::listeners::ListenerRegistry::new(),
        Arc::new(LedgerOnlyWitness),
        BootTimings::default(),
    )));

    let response = control
        .oneshot(
            Request::post("/har/load")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"qwen3"}"#))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("envelope JSON");
    assert_eq!(body["code"], "witness_unavailable", "body: {body}");
    assert_eq!(body["error"]["code"], "witness_unavailable", "body: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("nvml absent and nvidia-smi failed"),
        "the witness's reason must reach the client verbatim: {body}"
    );
}

/// Invariant 2, in-flight booking debit. Carried publicly by
/// `scheduler_witness.rs::concurrent_loads_cannot_double_spend_the_same_bytes`;
/// stated here as the named contract: a load that is short only on a live
/// in-flight booking waits instead of being admitted over bytes that are still
/// committed, and no engine spawns over the debit. The wait never strands: the
/// release terminalizes the booking row and re-gates the parked load.
#[tokio::test]
async fn in_flight_bookings_are_debited_so_two_loads_cannot_double_spend() {
    // Card 72, reserve 2, a constant 58 GiB used: a0 (10) fits (12 free), a1 (18)
    // replaces it and holds in the settle window with its 18 GiB booked while the
    // card still counts the released victim's bytes, and b (4, RP) then sees
    // free -6 once a1's booking is debited.
    let witness = FakeWitness::with(vec![sample(72.0, 58.0)]);
    let cfg = AppConfig {
        settle_timeout_s: 30,
        vram_reserve_gb: 2.0,
        ..AppConfig::default()
    };
    let (handle, runtime, _dir) = harness(
        cfg,
        &[
            (
                "a0",
                "name: a0\nengine: mock\ncategory: WORK\nvram_gb: 10\n",
            ),
            (
                "a1",
                "name: a1\nengine: mock\ncategory: WORK\nvram_gb: 18\n",
            ),
            ("b", "name: b\nengine: mock\ncategory: RP\nvram_gb: 4\n"),
        ],
        Arc::new(witness.clone()),
    );

    handle.swap("a0", CategoryId::Work).await.expect("a0 loads");
    let swapping = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.swap("a1", CategoryId::Work).await })
    };
    // a1's booking is live while its victim's bytes have not returned.
    wait_until(&handle, 10, |status| {
        status
            .jobs
            .iter()
            .any(|job| job.model == "a1" && job.state == SwapState::Dispose)
    })
    .await;

    let waiting = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.swap("b", CategoryId::Rp).await })
    };
    // The operator ruling on over-limit loads names the response: the deficit
    // rides on a live paper booking, so the row parks instead of dying AdmitFail.
    wait_until(&handle, 10, |status| {
        status
            .jobs
            .iter()
            .any(|job| job.model == "b" && job.state == SwapState::Idle)
    })
    .await;
    for _ in 0..20 {
        assert!(
            !waiting.is_finished(),
            "the in-flight 18 GiB debit must hold b out"
        );
        let status = handle.status().await.expect("actor alive");
        assert!(
            status
                .slots
                .iter()
                .all(|slot| slot.model.as_deref() != Some("b")),
            "b must never occupy a slot while a1's booking is live"
        );
        assert_eq!(
            runtime.spawn_count(),
            1,
            "neither a1 nor b may spawn an engine over the live debit"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // The card frees: a1 installs; then unloading a1 terminalizes its row, the
    // terminal funnel re-gates the parked row, and b's POST answers with receipt.
    witness.set(sample(72.0, 40.0));
    let Reply::Loaded(receipt) = swapping
        .await
        .expect("task alive")
        .expect("a1 loads once the card frees")
    else {
        panic!("expected Loaded");
    };
    assert_eq!(receipt.model, "a1");
    handle.unload_model("a1").await.expect("unload accepted");
    let Reply::Loaded(receipt) = tokio::time::timeout(Duration::from_secs(15), waiting)
        .await
        .expect("b admits once a1's row leaves the ledger")
        .expect("task alive")
        .expect("b loads after the release")
    else {
        panic!("expected Loaded");
    };
    assert_eq!(receipt.model, "b");
}

/// Invariant 3, victim credit `max(booking, measured)`. This is the audit's real
/// gap: the 2026-11-06 release projection was pinned only by unit tests inside
/// `src/scheduler/tests.rs` (the greedy-credit falsifier `a_greedy_residents_...`
/// family), never through a public integration target. Here a greedy mock row
/// books its nominal 12.0 while its own serving-window fold witnesses 68.4 GiB
/// on the card; a replacement gated on a 60 GiB launch floor physically fits, so
/// the same-slot release must be credited at the measured footprint and the swap
/// must reach Loaded. Crediting the booking alone (the pre-ruling shape, and the
/// exact regression `scripts/seed-admission-regression.ps1` plants: `max` ->
/// `min`) reads free 13.6 against the floor and dies at the no-release guard
/// with a kill-free `admission refused` - this test's `Loaded` expectation is
/// then the red leg. The settle window remains the physical truth: the victim
/// dies first and the re-gate sees the physically re-sampled card.
#[tokio::test]
async fn a_same_slot_victim_is_credited_at_the_larger_of_booking_and_measured() {
    let witness = FakeWitness::with(vec![sample(72.0, 0.0)]);
    let cfg = AppConfig {
        settle_timeout_s: 10,
        measured_sample_s: 1,
        ..AppConfig::default()
    };
    let (handle, runtime, _dir) = harness(
        cfg,
        &[
            (
                "host",
                "name: host\nengine: mock\ncategory: WORK\nvram_gb: 12\ncost: greedy\n",
            ),
            (
                "repl",
                "name: repl\nengine: mock\ncategory: WORK\nvram_gb: 67.4\ncost: greedy\n\
                 launch_only_with_at_least_vram_free: 60\n",
            ),
        ],
        Arc::new(witness.clone()),
    );
    let Reply::Loaded(host) = handle
        .swap("host", CategoryId::Work)
        .await
        .expect("host loads")
    else {
        panic!("expected Loaded");
    };
    // The card filling by 68.4 GiB is this greedy row's own footprint: the fold
    // stamps `live_measured_gb` for the spawn's ticket, and the booking stays the
    // nominal 12 (DOCTRINE 12: a projection is not a booking).
    witness.set(sample(72.0, 68.4));
    await_measured(&handle, "host", (Some(68.4), Some(68.4))).await;

    let loading = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.swap("repl", CategoryId::Work).await })
    };
    // With the measured credit the admit already fits (free 69.6 over the 60
    // floor), so the swap reaches the settle window on the victim release - the
    // state a booking-only credit never reaches. Hold the card full so the window
    // is genuinely waiting, then give the bytes back.
    wait_until(&handle, 15, |status| {
        status
            .jobs
            .iter()
            .any(|job| job.model == "repl" && job.state == SwapState::Dispose)
    })
    .await;
    witness.set(sample(72.0, 4.0));
    let Reply::Loaded(receipt) = loading
        .await
        .expect("task alive")
        .expect("repl loads on the projected release, not on the booked 12")
    else {
        panic!("expected Loaded");
    };
    assert_eq!(receipt.model, "repl");

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        status
            .slots
            .iter()
            .find(|slot| slot.category == CategoryId::Work)
            .and_then(|slot| slot.model.as_deref()),
        Some("repl"),
        "the replacement serves"
    );
    let job = job_row(&status, "repl");
    assert_eq!(job.gate_target_gib, Some(60.0), "the floor was the gate");
    // The credit was spent for real: the released host's engine is gone and the
    // only live engine is the replacement's. (A same-slot replacement is not an
    // eviction, so `victims_released` stays empty by codebase semantics.)
    assert!(
        runtime.is_dead(host.port).await,
        "the credited release really killed the host engine"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "only the replacement engine remains live"
    );
    assert_eq!(
        runtime.spawn_count(),
        2,
        "the host engine and the replacement engine, nothing refused"
    );
}

/// Invariant 4, eviction ordering. Carried publicly by
/// `scheduler_eviction.rs::eviction_picks_the_cheapest_sufficient_victim` (plus
/// its tie-break and multi-victim siblings); stated here as the named contract:
/// a short load evicts the cheapest sufficient victim set and the cheaper
/// insufficient bystander survives, recorded in the ledger.
#[tokio::test]
async fn eviction_releases_the_cheapest_sufficient_victim_and_spares_the_rest() {
    // Card 72, reserve 2: AUX holds 12 + 20 (the card says so), WORK wants 53 ->
    // 38 free, deficit 15: the 12 alone does not cover, 12+20 overshoots, the 20
    // alone is the cheapest sufficient set.
    let witness = FakeWitness::with(vec![sample(72.0, 32.0)]);
    let mut cfg = AppConfig {
        settle_timeout_s: 10,
        ..AppConfig::default()
    };
    cfg.category_mut(CategoryId::Aux)
        .expect("default config has AUX")
        .max_models = 2;
    let (handle, _runtime, _dir) = harness(
        cfg,
        &[
            (
                "aux12",
                "name: aux12\nengine: mock\ncategory: AUX\nvram_gb: 12\n",
            ),
            (
                "aux20",
                "name: aux20\nengine: mock\ncategory: AUX\nvram_gb: 20\n",
            ),
            (
                "work53",
                "name: work53\nengine: mock\ncategory: WORK\nvram_gb: 53\n",
            ),
        ],
        Arc::new(witness.clone()),
    );

    handle
        .swap("aux12", CategoryId::Aux)
        .await
        .expect("aux12 loads");
    handle
        .swap("aux20", CategoryId::Aux)
        .await
        .expect("aux20 loads");
    let loading = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.swap("work53", CategoryId::Work).await })
    };
    wait_until(&handle, 10, |status| {
        status
            .jobs
            .iter()
            .any(|job| job.model == "work53" && job.state == SwapState::Dispose)
    })
    .await;
    witness.set(sample(72.0, 12.0));
    let Reply::Loaded(receipt) = loading
        .await
        .expect("task alive")
        .expect("work loads after the sufficient victim is released")
    else {
        panic!("expected Loaded");
    };
    assert_eq!(receipt.model, "work53");

    let status = handle.status().await.expect("actor alive");
    let aux: Vec<Option<String>> = status
        .slots
        .iter()
        .filter(|slot| slot.category == CategoryId::Aux)
        .map(|slot| slot.model.clone())
        .collect();
    assert!(
        aux.contains(&Some("aux12".to_string())),
        "the cheaper insufficient victim must survive: {aux:?}"
    );
    assert!(
        !aux.contains(&Some("aux20".to_string())),
        "the sufficient victim was evicted: {aux:?}"
    );
    assert_eq!(
        job_row(&status, "work53").victims_released,
        vec!["aux20".to_string()],
        "the ledger records exactly the set the planner selected"
    );
}

/// Invariant 5, unreachable-floor refusal. Carried publicly by
/// `scheduler_eviction.rs::an_unreachable_launch_floor_refuses_before_any_release`
/// (DECISIONS row 10); stated here as the named contract: when no
/// release the harness may perform reaches the launch floor, admission refuses
/// BEFORE any kill, names the guard, and leaves the resident installed.
#[tokio::test]
async fn an_unreachable_launch_floor_refuses_before_any_kill() {
    // Card 72, reserve 2, tolerance 2 (defaults): holder books 20, the card shows
    // 60 used (40 GiB of it foreign). free at admit = 72 - 2 - 60 + 20 = 30.0 and
    // no cross-category victim exists, so even releasing everything the harness
    // may release leaves 65 - 2 > 30.0 + 0.0: the floor is unreachable.
    let witness = FakeWitness::with(vec![sample(72.0, 0.0)]);
    let (handle, runtime, _dir) = harness(
        AppConfig {
            settle_timeout_s: 2,
            ..AppConfig::default()
        },
        &[
            (
                "holder",
                "name: holder\nengine: mock\ncategory: WORK\nvram_gb: 20\n",
            ),
            (
                "greedy65",
                "name: greedy65\nengine: mock\ncategory: WORK\nvram_gb: 55\ncost: greedy\n\
                 launch_only_with_at_least_vram_free: 65\n",
            ),
        ],
        Arc::new(witness.clone()),
    );
    let Reply::Loaded(holder) = handle
        .swap("holder", CategoryId::Work)
        .await
        .expect("the resident loads onto the empty card")
    else {
        panic!("expected Loaded");
    };
    // The card after install: holder's 20 plus 40 GiB of foreign occupancy the
    // harness may never release.
    witness.set(sample(72.0, 60.0));

    let error = handle
        .swap("greedy65", CategoryId::Work)
        .await
        .expect_err("no release can reach the floor: the guard must refuse");
    let message = error.to_string();
    assert!(
        message.contains("launch floor not met: at least 65.0 GiB free required, free 30.0 GiB"),
        "the refusal is the floor refusal with the compared figure: {message}"
    );
    assert!(
        message.contains("No releases allow for the VRAM floor. Check foreign residents"),
        "the refusal must name the guard, not a bare floor miss: {message}"
    );

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        status
            .slots
            .iter()
            .find(|slot| slot.category == CategoryId::Work)
            .and_then(|slot| slot.model.as_deref()),
        Some("holder"),
        "the resident a release could not save stays installed"
    );
    let job = job_row(&status, "greedy65");
    assert!(job.terminal, "{job:?}");
    assert_eq!(
        job.outcome.as_deref(),
        Some("admission refused"),
        "refused at the gate, never at settle: {job:?}"
    );
    assert_eq!(job.gate_target_gib, Some(65.0), "the floor was the gate");
    assert!(
        job.victims_released.is_empty(),
        "the guard refuses before any release: {:?}",
        job.victims_released
    );
    assert_eq!(runtime.spawn_count(), 1, "the refused load never spawned");
    assert!(
        !runtime.is_dead(holder.port).await,
        "no kill ever reached the resident's engine"
    );
}
