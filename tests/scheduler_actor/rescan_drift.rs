use super::*;

// --- reload-drift Axis-1: rescan retires queued intents (operator ruling) ------------
//
// Operator ruling: **Rescan invalidates queued/parked rows whose model changed or
// vanished; a no-change rescan must NOT drop queues.** The retirement carries
// the `superseded` vocabulary with a named reason, releases like every
// other queued retirement, and NEVER touches a live slot or an armed row
// (`Admit` and beyond is the committed-launch half the ruling excludes).

/// Kill-holding resident: its release parks until the test calls
/// `release_held_kills`, so the state behind a parked load is stable to drive
/// on rather than a race against `release_timeout_s`.
const DRIFT_RESIDENT: &str = "drift-resident";
/// The swap frozen in `Admit` (the gate witness has no sample left for it).
const DRIFT_FROZEN: &str = "drift-frozen";
/// The load that parks behind the frozen swap.
const DRIFT_QUEUED: &str = "drift-queued";

/// [`harness5_witness`]'s shape for the drift pins, additionally handing back
/// the ModelConf dir (to edit/delete yaml bytes) and the witness gate (to let
/// the survivor pin release its queued load).
fn drift_harness(
    specs: &[Spec5],
    samples: usize,
) -> (
    SchedulerHandle,
    MockRuntime,
    PathBuf,
    Arc<tokio::sync::Semaphore>,
) {
    let cfg = AppConfig {
        // Generous: every held release here is released deliberately by the
        // test, never by expiry; 30 s keeps the parked state stably observable.
        release_timeout_s: 30,
        ..config(10)
    };
    let runtime = MockRuntime::new();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    for engine in ["mock", "mock-hold-kill"] {
        adapters.insert(
            engine.to_string(),
            Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
        );
    }
    let witness = Arc::new(GatedWitness::new(samples));
    let gate = Arc::clone(&witness.gate);
    let dir = write_modelconf5(specs);
    let handle = Scheduler::start(
        cfg,
        dir.clone(),
        adapters,
        Arc::new(runtime.clone()) as Arc<dyn simpleharn::engine::EngineRuntime>,
        witness,
    );
    (handle, runtime, dir, gate)
}

/// The observable identity of a ledger row: what the rescan must not change for
/// a row that was never a candidate.
#[derive(Debug, PartialEq)]
struct RowIdentity {
    id: JobId,
    state: SwapState,
    terminal: bool,
    outcome: Option<String>,
    error: Option<String>,
}

fn row_identity(status: &SchedulerStatus, model: &str) -> RowIdentity {
    let row = status
        .jobs
        .iter()
        .find(|j| j.model == model)
        .unwrap_or_else(|| panic!("`{model}` must be on the ledger: {:?}", status.jobs));
    RowIdentity {
        id: row.id,
        state: row.state,
        terminal: row.terminal,
        outcome: row.outcome.clone(),
        error: row.error.clone(),
    }
}

/// Serves a kill-holding resident, submits a swap the gate freezes in `Admit`,
/// then parks `queued` on the slot (the park supersede-cancels the frozen swap,
/// whose reap parks on the resident's held kill). Returns the parked load's
/// still-pending caller reply and the resident's receipt (engine port).
async fn drift_park(
    handle: &SchedulerHandle,
    resident: &str,
    frozen: &str,
    queued: &str,
    category: CategoryId,
) -> (
    simpleharn::scheduler::SwapReceipt,
    tokio::task::JoinHandle<Result<Reply, SchedulerError>>,
) {
    let Reply::Loaded(resident_receipt) = handle
        .swap(resident, category)
        .await
        .expect("the resident loads")
    else {
        panic!("the resident must load");
    };
    submit_and_vanish(handle, frozen, category).await;
    // Armed and holding the slot: frozen in `Admit` when the gate has no sample
    // left for it, or advanced to a held-kill `Dispose` when it does — either
    // state keeps `slot.job` Some, which is all parking requires.
    wait_until(handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.model == frozen
                && j.kind == JobKind::Load
                && !j.terminal
                && j.state != SwapState::Idle
        })
    })
    .await;
    let client = handle.clone();
    let name = queued.to_string();
    let reply = tokio::spawn(async move { client.swap(&name, category).await });
    wait_until(handle, 5, |s| is_queued(s, queued)).await;
    (resident_receipt, reply)
}

fn drift_specs(
    resident: &'static str,
    frozen: &'static str,
    queued: &'static str,
    category: CategoryId,
) -> Vec<Spec5> {
    vec![
        (resident, "mock-hold-kill", category, Some(4.0), ""),
        (frozen, "mock", category, Some(4.0), ""),
        (queued, "mock", category, Some(4.0), ""),
    ]
}

#[tokio::test]
async fn rescan_supersedes_a_queued_load_whose_yaml_changed() {
    // Pin (a): queue -> EDIT (content AND mtime move: the pin asserts the
    // fingerprint actually changed) -> Rescan -> the row retires with the named
    // reason, its caller is answered, no engine ever spawns for it, and the
    // gate arithmetic is untouched. Falsifier: disabling the invalidation keeps
    // the row queued and this fails.
    let (handle, runtime, dir, _gate) = drift_harness(
        &drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work),
        1,
    );
    let (_resident, parked_reply) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    let path = dir.join(format!("{DRIFT_QUEUED}.yaml"));
    let before = simpleharn::model::fingerprint(&path).expect("queued yaml readable");
    std::fs::write(
        &path,
        "name: drift-queued\nengine: mock\ncategory: WORK\nvram_gb: 44\n",
    )
    .expect("edit the queued yaml");
    let after = simpleharn::model::fingerprint(&path).expect("edited yaml readable");
    assert_ne!(
        before, after,
        "the fixture must move real bytes (this pin's same-bytes twin is \
         an_unchanged_rescan_keeps_the_queue_and_the_parked_load_still_loads)"
    );
    handle.reload().await.expect("rescan applied");

    let answered = parked_reply
        .await
        .expect("parked caller alive")
        .expect_err("the rescan answers the queued intent with a refusal");
    let SchedulerError::Cancelled(reason) = answered else {
        panic!("expected the rescan supersede, got {answered:?}");
    };
    assert!(
        reason.contains("superseded by rescan")
            && reason.contains("drift-queued.yaml")
            && reason.contains("changed since queued")
            && reason.contains("re-queue to apply"),
        "{reason}"
    );
    let status = handle.status().await.expect("actor alive");
    let row = status
        .jobs
        .iter()
        .find(|j| j.model == DRIFT_QUEUED)
        .expect("the retired row is on the ledger");
    assert!(row.terminal, "{row:?}");
    assert_eq!(row.outcome.as_deref(), Some("cancelled"), "{row:?}");
    assert_eq!(
        row.error.as_deref(),
        Some(format!("cancelled: {reason}").as_str()),
        "the reason is VISIBLE on the job row the page already renders: {row:?}"
    );
    assert!(
        !row.is_fault,
        "a rescan supersede is operator policy, not a fault"
    );
    assert_eq!(
        booking_gibs(&status.jobs),
        0.0,
        "budget no-leak probe: the parked row never booked, the retirement books nothing"
    );
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(DRIFT_RESIDENT.to_string())],
        "the resident serves on, untouched by the invalidation"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "no engine spawned for the retired intent"
    );
    assert!(
        !is_queued(&status, DRIFT_QUEUED),
        "the queue place was released: {:?}",
        status.jobs
    );

    // The leak falsifier behind the release: settling the frozen swap's reap
    // (which owns the slot) must find no queue place to arm — a retirement that
    // forgot `finish_queued_load`'s removal would spuriously start the dead intent.
    runtime.release_held_kills();
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;
    assert_eq!(runtime.spawn_count(), 1, "only the resident ever spawned");
    assert_eq!(runtime.server_count(), 0, "and the chain leaves no engine");
    assert_eq!(
        slot_model(&handle.status().await.expect("alive"), CategoryId::Work),
        vec![None]
    );
    assert_eq!(
        booking_gibs(&handle.status().await.expect("alive").jobs),
        0.0
    );
}

#[tokio::test]
async fn rescan_supersedes_a_queued_load_when_only_the_clock_moves() {
    // [O-1] M4-isolating pin: the SAME bytes rewritten (size provably identical)
    // and the mtime forced forward via `File::set_times` — size cannot
    // discriminate this leg, so it is the tree's only pin of the clock half of
    // `FileFingerprint`. Falsifier (reviewer's M4): stubbing `fingerprint()`'s
    // mtime arm to a constant makes this the SOLE failing test — every other
    // rescan pin moves the size too. The recorded pre/post values are asserted
    // so the leg is proven even where a same-byte rewrite would bump mtime on
    // its own.
    let (handle, runtime, dir, _gate) = drift_harness(
        &drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work),
        1,
    );
    let (_resident, parked_reply) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    let path = dir.join(format!("{DRIFT_QUEUED}.yaml"));
    let before = simpleharn::model::fingerprint(&path).expect("queued yaml readable");
    let original = std::fs::read(&path).expect("read the original bytes");
    std::fs::write(&path, &original).expect("rewrite the SAME bytes");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for the clock bump");
    file.set_times(
        std::fs::FileTimes::new().set_modified(
            before
                .mtime
                // 60_000 ms, not `from_secs(60)`: the seconds component is E and
                // clippy's `approx_constant` lint (rightly) objects to writing it.
                .checked_add(Duration::from_millis(60_000))
                .expect("mtime in range"),
        ),
    )
    .expect("force the mtime forward");
    drop(file);
    let after = simpleharn::model::fingerprint(&path).expect("rewritten yaml readable");
    assert_eq!(
        (before.size, after.size),
        (original.len() as u64, original.len() as u64),
        "size must NOT be the discriminator on this leg"
    );
    assert_ne!(
        before.mtime, after.mtime,
        "the clock leg must really move or the pin proves nothing: pre {:?} vs post {:?}",
        before.mtime, after.mtime
    );
    handle.reload().await.expect("rescan applied");
    let answered = tokio::time::timeout(Duration::from_secs(10), parked_reply)
        .await
        .expect(
            "the clock-only rewrite must retire the queued intent within seconds \
             (a still-waiting caller means the mtime half of the fingerprint is dead)",
        )
        .expect("parked caller alive")
        .expect_err("a clock-only change is still a change");
    let SchedulerError::Cancelled(reason) = answered else {
        panic!("expected the rescan supersede, got {answered:?}");
    };
    assert!(
        reason.contains("superseded by rescan")
            && reason.contains("drift-queued.yaml")
            && reason.contains("changed since queued")
            && reason.contains("re-queue to apply"),
        "{reason}"
    );
    let status = handle.status().await.expect("actor alive");
    assert!(
        !is_queued(&status, DRIFT_QUEUED),
        "the queue place was released: {:?}",
        status.jobs
    );
    runtime.release_held_kills();
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;
    assert_eq!(
        runtime.spawn_count(),
        1,
        "no engine ever spawned for the clock-retired intent"
    );
}

#[tokio::test]
async fn an_unchanged_rescan_keeps_the_queue_and_the_parked_load_still_loads() {
    // Pin (b), THE no-drop pin: queue -> Rescan with the bytes untouched -> the
    // row survives (and arms the SAME ledger row later) and
    // loads when the slot frees. Falsifier: an always-invalidate predicate
    // retires this row on the no-change rescan and the reply never Loads.
    let (handle, runtime, _dir, gate) = drift_harness(
        &drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work),
        1,
    );
    let (_resident, parked_reply) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    let queued_id = row_identity(&handle.status().await.expect("actor alive"), DRIFT_QUEUED);
    assert!(!queued_id.terminal && queued_id.state == SwapState::Idle);
    handle.reload().await.expect("first rescan applied");
    handle.reload().await.expect("second rescan applied");
    let status = handle.status().await.expect("actor alive");
    assert!(
        is_queued(&status, DRIFT_QUEUED),
        "a no-change rescan never drops a queue: {:?}",
        status.jobs
    );
    assert_eq!(
        row_identity(&status, DRIFT_QUEUED),
        queued_id,
        "and the survivor keeps its exact ledger row across both rescans"
    );

    // Release the held reap (the frozen swap terminalizes, freeing the slot) and
    // hand the parked load the witness sample it waits on: it must still load.
    gate.add_permits(1);
    runtime.release_held_kills();
    let Reply::Loaded(receipt) = parked_reply
        .await
        .expect("parked caller alive")
        .expect("the kept queue eventually loads")
    else {
        panic!("the parked load must end Loaded");
    };
    assert_eq!(
        receipt.job, queued_id.id,
        "the armed load rides the row registered at park time, not a fresh one"
    );
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(DRIFT_QUEUED.to_string())],
        "and it is the model the slot ended on"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "resident replaced, parked load serving, none leaked"
    );
}

#[tokio::test]
async fn rescan_supersedes_a_queued_load_whose_file_was_deleted() {
    // Pin (c): queue -> delete the model's yaml -> Rescan -> retired with the
    // exact removal reason. Falsifier: disabling the invalidation leaves the
    // row queued for a model that no longer exists.
    let (handle, runtime, dir, _gate) = drift_harness(
        &drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work),
        1,
    );
    let (_resident, parked_reply) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    std::fs::remove_file(dir.join(format!("{DRIFT_QUEUED}.yaml"))).expect("delete the queued yaml");
    handle.reload().await.expect("rescan applied");

    let answered = parked_reply
        .await
        .expect("parked caller alive")
        .expect_err("the deleted model's intent is refused");
    let SchedulerError::Cancelled(reason) = answered else {
        panic!("expected the removal supersede, got {answered:?}");
    };
    assert_eq!(
        reason, "superseded by rescan (removed from the catalog)",
        "the exact named reason for the vanished model"
    );
    let status = handle.status().await.expect("actor alive");
    assert!(!is_queued(&status, DRIFT_QUEUED));
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(DRIFT_RESIDENT.to_string())],
        "the resident is untouched by a queue retirement"
    );
    runtime.release_held_kills();
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;
    assert_eq!(
        runtime.spawn_count(),
        1,
        "the deleted model never got an engine"
    );
}

#[tokio::test]
async fn rescan_retires_only_the_queued_load_whose_yaml_moved() {
    // Pin (d): two queued loads (WORK and AUX), edit ONE yaml -> Rescan ->
    // exactly one retirement; the survivor stays queued. Falsifier: an
    // always-invalidate predicate retires both here (and the no-change pin (b)).
    let mut specs = drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work);
    specs.extend(drift_specs(
        "drift-aux-resident",
        "drift-aux-frozen",
        "drift-aux-queued",
        CategoryId::Aux,
    ));
    let (handle, _runtime, dir, _gate) = drift_harness(&specs, 4);
    let (_w, _wreply) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    let (_a, areply) = drift_park(
        &handle,
        "drift-aux-resident",
        "drift-aux-frozen",
        "drift-aux-queued",
        CategoryId::Aux,
    )
    .await;
    std::fs::write(
        dir.join(format!("{DRIFT_QUEUED}.yaml")),
        "name: drift-queued\nengine: mock\ncategory: WORK\nvram_gb: 44\n",
    )
    .expect("edit only the WORK queued yaml");
    handle.reload().await.expect("rescan applied");

    let status = handle.status().await.expect("actor alive");
    assert!(!is_queued(&status, DRIFT_QUEUED), "the edited row retires");
    let retired = status
        .jobs
        .iter()
        .find(|j| j.model == DRIFT_QUEUED && j.terminal)
        .expect("terminalized");
    assert!(
        retired
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("changed since queued"),
        "{retired:?}"
    );
    assert!(
        is_queued(&status, "drift-aux-queued"),
        "the untouched queue survives: {:?}",
        status.jobs
    );
    // The survivor's load-then-complete fate is pin (b)'s subject; here the aux
    // row must simply have survived the rescan. Dropping the caller is the
    // client-abort shape the actor already handles (a dropped waiter never strands
    // a row).
    drop(areply);
}

#[tokio::test]
async fn rescan_retires_a_parked_twin_of_a_serving_model_without_touching_the_slot() {
    // Pin (e), the supersede interplay: resident serving, a swap frozen in
    // `Admit` (the slot's live job), and a parked TWIN of the resident itself
    // (parked by the same on_swap path whose deliberate supersede of queued
    // twins 19[5] ruled WONTFIX — that vocabulary stays: this retirement is a
    // RESCAN supersede, named differently). The invalidation must reach neither
    // the resident (live slot, ticket, engine) nor the frozen swap (armed
    // state): the 19[5] WONTFIX stays honored and no new dedup debate opens.
    // Falsifier: widening the candidate set past `Idle` terminalizes the frozen
    // `Admit` row through this path and the RowIdentity compare below fails.
    let (handle, runtime, dir, _gate) = drift_harness(
        &drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work),
        1,
    );
    // The first parked row is the queued twin that 19[5] rules superseded — the
    // park path deliberately answers it with "before it started"; this pin keeps
    // that vocabulary intact and adds the rescan one beside it.
    let (resident, twin_of_queued) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    // Park a twin of the RESIDENT: its load row is terminal ("loaded"), so the
    // park's supersede-cancel lands on the frozen swap — and this time the
    // parked model IS the resident's name.
    let client = handle.clone();
    let name = DRIFT_RESIDENT.to_string();
    let twin_reply = tokio::spawn(async move { client.swap(&name, CategoryId::Work).await });
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.model == DRIFT_RESIDENT
                && j.kind == JobKind::Load
                && !j.terminal
                && j.state == SwapState::Idle
        })
    })
    .await;
    let superseded = twin_of_queued
        .await
        .expect("queued-twin caller alive")
        .expect_err("19[5]: the deliberate supersede answers the older queued twin");
    let SchedulerError::Cancelled(older) = superseded else {
        panic!("expected the 19[5] supersede, got {superseded:?}");
    };
    assert!(
        older.contains("superseded by a newer load before it started"),
        "the 19[5] vocabulary stands beside the rescan one: {older}"
    );
    let status_before = handle.status().await.expect("actor alive");
    let frozen_before = row_identity(&status_before, DRIFT_FROZEN);
    assert!(
        !frozen_before.terminal,
        "the frozen swap must be live for this pin"
    );

    std::fs::write(
        dir.join(format!("{DRIFT_RESIDENT}.yaml")),
        "name: drift-resident\nengine: mock-hold-kill\ncategory: WORK\nvram_gb: 44\n",
    )
    .expect("edit the resident's yaml");
    handle.reload().await.expect("rescan applied");

    let answered = twin_reply
        .await
        .expect("twin caller alive")
        .expect_err("the twin intent is refused by the rescan");
    let SchedulerError::Cancelled(reason) = answered else {
        panic!("expected the rescan supersede, got {answered:?}");
    };
    assert!(
        reason.contains("superseded by rescan") && reason.contains("changed since queued"),
        "{reason}"
    );
    assert!(
        !reason.contains("before it started"),
        "19[5] separation: the rescan reason is not the queued-twin supersede: {reason}"
    );
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(DRIFT_RESIDENT.to_string())],
        "the resident holds its slot through its own twin's retirement"
    );
    assert_eq!(
        status
            .slots
            .iter()
            .find(|s| s.category == CategoryId::Work)
            .and_then(|s| s.port),
        Some(resident.port),
        "same port — the same ticket's authority, untouched"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "and the engine is still answering"
    );
    assert!(
        !runtime.is_dead(resident.port).await,
        "the resident's port accepts: a queue retirement never killed it"
    );
    assert_eq!(
        row_identity(&status, DRIFT_FROZEN),
        frozen_before,
        "the in-flight (armed) swap is not a candidate: same row, same stage, same notes"
    );
    runtime.release_held_kills();
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;
    assert_eq!(
        runtime.spawn_count(),
        1,
        "no engine ever spawned for the retired twin"
    );
}

#[tokio::test]
async fn a_serving_model_gains_its_launch_stamp_and_ambers_on_the_next_rescan() {
    // Axis-2 end-to-end, resident side (operator ruling): the serving slot
    // carries the server-formatted wall-clock launch stamp from INSTALL, the
    // verdict moves ONLY on a rescan (the pre-rescan leg proves the status path
    // never stats), and the stamp never moves under the marker.
    // Falsifier: removing the install-time capture makes the rescan unable to
    // amber (third assert); computing the verdict in the snapshot flips the
    // pre-rescan assert early.
    let (handle, _runtime, dir, _gate) =
        drift_harness(&[("stamponly", "mock", CategoryId::Work, Some(4.0), "")], 1);
    let Reply::Loaded(_receipt) = handle
        .swap("stamponly", CategoryId::Work)
        .await
        .expect("stamponly loads")
    else {
        panic!("expected Loaded");
    };
    let status = handle.status().await.expect("actor alive");
    let slot = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Work)
        .expect("WORK slot");
    assert_eq!(slot.model.as_deref(), Some("stamponly"));
    let stamp = slot
        .launched_at
        .clone()
        .expect("a serving slot carries its wall-clock launch, server-formatted");
    assert_eq!(stamp.len(), 20, "fixed-width ISO-8601 seconds: {stamp}");
    assert_eq!(&stamp[4..5], "-");
    assert_eq!(
        &stamp[10..11],
        "T",
        "the delimiter the page prints verbatim: {stamp}"
    );
    assert!(
        stamp.ends_with('Z'),
        "UTC, no client-side offset math: {stamp}"
    );
    assert_eq!(slot.stale_config, Some(false), "clean at install");

    std::fs::write(
        dir.join("stamponly.yaml"),
        // The `cpu_offload` flag key: a pure cost edit is now
        // re-booked without a reload, so an amber pin must move a flag —
        // config the running engine never read — to earn the verdict.
        "name: stamponly\nengine: mock\ncategory: WORK\nvram_gb: 44\ncpu_offload: true\n",
    )
    .expect("operator edit");
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        status
            .slots
            .iter()
            .find(|s| s.category == CategoryId::Work)
            .and_then(|s| s.stale_config),
        Some(false),
        "an edit is invisible to status reads until a rescan: stat on reload/install ONLY"
    );
    handle.reload().await.expect("rescan applied");
    let status = handle.status().await.expect("actor alive");
    let slot = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Work)
        .expect("WORK slot");
    assert_eq!(slot.stale_config, Some(true), "the rescan is the judge");
    assert_eq!(
        slot.launched_at.as_deref(),
        Some(stamp.as_str()),
        "the launch stamp rides the install, not the marker"
    );
}

#[tokio::test]
async fn one_edit_retires_the_queue_and_ambers_the_resident_on_the_same_rescan() {
    // Interlock 3: the same operator edit, one rescan, two honest surfaces — the
    // parked intent retires with the round-1 rescan wording AND the resident of
    // the same model ambers. Neither may swallow the other, and the armed (frozen)
    // swap is untouched by both. Falsifier: retiring the resident kills the slot
    // assert; queue-only marking kills the stale assert.
    let (handle, runtime, dir, _gate) = drift_harness(
        &drift_specs(DRIFT_RESIDENT, DRIFT_FROZEN, DRIFT_QUEUED, CategoryId::Work),
        1,
    );
    let (_resident, _queued_reply) = drift_park(
        &handle,
        DRIFT_RESIDENT,
        DRIFT_FROZEN,
        DRIFT_QUEUED,
        CategoryId::Work,
    )
    .await;
    let client = handle.clone();
    let name = DRIFT_RESIDENT.to_string();
    let twin = tokio::spawn(async move { client.swap(&name, CategoryId::Work).await });
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.model == DRIFT_RESIDENT
                && j.kind == JobKind::Load
                && !j.terminal
                && j.state == SwapState::Idle
        })
    })
    .await;
    std::fs::write(
        dir.join(format!("{DRIFT_RESIDENT}.yaml")),
        // The `cpu_offload` flag key: a pure cost edit is now
        // re-booked without a reload; this pin judges the amber verdict, so
        // the edit moves a flag the running engine never read.
        "name: drift-resident\nengine: mock-hold-kill\ncategory: WORK\nvram_gb: 44\ncpu_offload: true\n",
    )
    .expect("edit the resident's yaml");
    handle.reload().await.expect("rescan applied");
    let answered = twin
        .await
        .expect("twin caller alive")
        .expect_err("the parked intent retires");
    assert!(
        matches!(&answered, SchedulerError::Cancelled(reason)
            if reason.contains("superseded by rescan") && reason.contains("changed since queued")),
        "{answered:?}"
    );
    let status = handle.status().await.expect("actor alive");
    let slot = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Work)
        .expect("WORK slot");
    assert_eq!(
        slot.model.as_deref(),
        Some(DRIFT_RESIDENT),
        "the resident still serves"
    );
    assert_eq!(
        slot.stale_config,
        Some(true),
        "and the same rescan ambers it — the pair lands together"
    );
    runtime.release_held_kills();
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;
    assert_eq!(runtime.spawn_count(), 1, "no engine for the retired intent");
}
