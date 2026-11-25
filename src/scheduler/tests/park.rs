//! Park-ladder pins (operator rulings): the gate parks
//! a short load instead of refusing when the deficit is covered by waitable
//! sources - other loads' in-flight bookings and/or a release already in
//! progress - and the launch floor is reachable. A release wait additionally
//! requires every named blocker to be a releasing resident; the
//! booking park keeps no blocker gate. Every other short stay keeps its named
//! refusal. The pins share the fixtures owned by the parent test module.

use super::*;
use crate::scheduler::{AdmissionError, Event, Reclaim, SchedulerError, Victim};
/// Shared fixture for the in-progress-release park pins: a WORK
/// slot holding a resident whose operator `Unload` job is in progress (a
/// non-terminal unload row on the slot, driver release not yet confirmed —
/// the solar shape), plus a free RP slot where the row under test
/// arms, on a 72 GiB card with the 2 GiB reserve and the 2 GiB tolerance.
/// `resident` is the occupant's booked and live-folded GiB (the figure the
/// release projects); `aux` (when given) seats an AUX resident the RP row
/// could evict — the `considered` candidate the park must not need and never
/// kills. The parked slot is the last one. The caller's `state` arrives from
/// `fold_state` (pins that drive releases by hand) or `death_state` (the pin
/// that lets the superseded release actually reap), and the fixture owns the
/// rest. Returns the state, the row under test's entry, the unload job id,
/// and the parked row's slot.
fn release_short_fixture(
    dir: &std::path::Path,
    mut state: ActorState,
    waiting_name: &str,
    waiting_yaml: &str,
    resident: (f64, Option<f64>),
    aux: Option<(&str, f64)>,
) -> (ActorState, ModelEntry, JobId, usize) {
    let resident_yaml = dir.join("work-resident.yaml");
    std::fs::write(
        &resident_yaml,
        "engine: mock\ncategory: work\nvram_gb: 10\n",
    )
    .expect("seed");
    let waiting_path = dir.join(format!("{waiting_name}.yaml"));
    std::fs::write(&waiting_path, waiting_yaml).expect("seed waiting");
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    let resident_entry = rescan_entry(&resident_yaml, "work-resident");
    let waiting = rescan_entry(&waiting_path, waiting_name);
    let mut catalog = vec![resident_entry.clone(), waiting.clone()];
    if let Some((aux_name, _)) = aux {
        let aux_yaml = dir.join(format!("{aux_name}.yaml"));
        std::fs::write(&aux_yaml, "engine: mock\ncategory: aux\nvram_gb: 4\n").expect("seed aux");
        catalog.push(rescan_entry(&aux_yaml, aux_name));
    }
    state.catalog = catalog;
    // Seat the serving resident exactly as `install` leaves it, then rebook
    // it to the fixture's size: the release projects the max of the booking
    // and this spawn's own folded footprint.
    let resident_ticket = fold_ticket(18_150);
    let resident_row = JobId(state.next_job);
    seat_spawn(&mut state, &resident_entry, &resident_ticket, 18_150);
    state
        .jobs
        .get_mut(&resident_row)
        .expect("seated resident row")
        .need = Some(Gibibytes(resident.0));
    let occupant = state.slots[0].occupant.as_mut().expect("seated resident");
    occupant.need = Gibibytes(resident.0);
    occupant.live_measured_gb = resident.1;
    // The operator's unload, armed in the shape `begin_unload_slot` leaves
    // between the accept and the driver-confirmed release: the resident still
    // holds the slot, the unload row books nothing on either ladder.
    let unload = JobId(state.next_job);
    state.next_job += 1;
    state.jobs.insert(
        unload,
        Job {
            kind: JobKind::Unload,
            path: None,
            model: resident_entry.name.clone(),
            engine: String::new(),
            category: CategoryId::Work,
            state: SwapState::Dispose,
            terminal: false,
            outcome: None,
            error: None,
            need: None,
            resource: crate::model::Resource::Gpu,
            ram_need: None,
            cost: crate::model::CostClass::Fixed,
            vram_gate: crate::model::VramGate::default(),
            load_timeout_s: None,
            gate_target: None,
            free_at_admit: None,
            observed_at_admit: None,
            release_first: false,
            container: None,
            timings: JobTimings::default(),
            t0: Instant::now(),
            waiters: Vec::new(),
            tasks: Vec::new(),
            spawned: None,
            readiness: None,
            measured: None,
            victim: Some(Victim {
                model: resident_entry.name.clone(),
                ticket: resident_ticket.clone(),
                slot: 0,
                evicted: false,
                cost: resident.0,
            }),
            victims: Vec::new(),
            reclaim: Reclaim::default(),
            victims_released: Vec::new(),
            released_cost: 0.0,
            holder_released_cost: 0.0,
            other_need_at_admit: 0.0,
            note: Some("unloaded".to_string()),
            warnings: Vec::new(),
            slot: Some(0),
            queue_fingerprint: None,
        },
    );
    state.slots[0].job = Some(unload);
    // The remaining slots: the optional evictable AUX resident, then the free
    // RP slot the row under test parks on.
    let mut next_port = 18_151u16;
    if let Some((aux_name, aux_need)) = aux {
        state.slots.push(Slot {
            category: CategoryId::Aux,
            index: state.slots.len(),
            engine_port: next_port,
            occupant: Some(Occupant {
                model: aux_name.to_string(),
                need: Gibibytes(aux_need),
                ram_need: None,
                port: next_port,
                ticket: fold_ticket(next_port),
                loaded_at: Instant::now(),
                launched_at_wall: std::time::SystemTime::now(),
                spawn_config: None,
                stale_config: false,
                serve_id: None,
                measure: None,
                live_measured_gb: None,
            }),
            job: None,
        });
        next_port += 1;
    }
    let rp_slot = state.slots.len();
    state.slots.push(Slot {
        category: CategoryId::Rp,
        index: rp_slot,
        engine_port: next_port,
        occupant: None,
        job: None,
    });
    (state, waiting, unload, rp_slot)
}

/// The solar park case (jobs 4/15/17): the
/// only memory the RP row lacks is a WORK resident whose operator unload is
/// in progress. Pre-fix the park guard's first leg (`in_flight > 0`) was
/// false — an unload is not a load — and the row died `AdmitFail
/// Insufficient` 452 ms before the room came back on its own. Now the
/// release is a waitable source: the row parks, the terminal funnel re-arms
/// it at the unload's terminal, and the freed card admits it on the ordinary
/// rung with no kill. Falsifier: drop the release credit from the park arm
/// and the first gate terminalizes the row — the `terminal`, `pending`, and
/// unanswered-waiter legs go red together, exactly the pre-fix refusal the
/// matrix recorded.
#[tokio::test]
async fn a_load_short_on_an_in_progress_release_waits_then_admits() {
    let dir = crate::testutil::temp_dir("release-park-solar");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-load",
        "engine: mock\ncategory: rp\nvram_gb: 40\n",
        (65.0, Some(70.1)),
        None,
    );
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, vec![reply_tx]);
    // free = 72 - 2 - 70.5 = -0.5 against need 40: short 40.5; `in_flight` is
    // zero (the unload row is not a load); and the release projects 70.1 GiB
    // (the folded footprint over the 65 GiB booking) — the whole deficit is
    // waitable, and the only named blocker is the release itself.
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        !row.terminal,
        "a release in progress is a wait, not a refusal: {:?}",
        row.error
    );
    assert!(row.error.is_none(), "{:?}", row.error);
    assert_eq!(row.state, SwapState::Idle, "the row is parked, not armed");
    assert_eq!(
        state.pending.get(&rp),
        Some(&parked),
        "the row waits in its slot's queue place"
    );
    assert!(
        state.slots[rp].job.is_none(),
        "the parked row releases the gate's slot claim"
    );
    assert!(
        state.slots[0].occupant.is_some(),
        "the releasing resident still holds its slot"
    );
    assert_eq!(
        state.jobs[&unload].state,
        SwapState::Dispose,
        "the release keeps running"
    );
    assert!(row.victims.is_empty(), "the park plans no kill");
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the caller keeps waiting - the row answers nothing at park time"
    );
    // The release confirms on the actor's turn: the WORK slot frees, the
    // unload row terminalizes, and the terminal funnel re-arms the parked row
    // through the queue path — the missing piece was the park decision, not
    // the wake.
    state.apply_event(unload, Event::Released(Ok(Duration::from_millis(452))));
    assert!(
        state.jobs[&unload].terminal,
        "the unload reached its terminal"
    );
    assert!(
        state.slots[0].occupant.is_none(),
        "the confirmed release freed the WORK slot"
    );
    assert_eq!(
        state.slots[rp].job,
        Some(parked),
        "the drain re-arms the parked row on its free slot"
    );
    assert!(state.pending.is_empty(), "and takes its queue place");
    assert_eq!(
        state.jobs[&parked].state,
        SwapState::Admit,
        "the row stands before the gate again"
    );
    assert!(!state.jobs[&parked].terminal);
    // Re-gate on the freed card: the load admits on the ordinary rung and
    // serves — the retry that cost the operator 452 ms now happens on its own.
    state.finish_admit_card(parked, Ok(gate_sample(2.0)));
    let row = &state.jobs[&parked];
    assert!(
        !row.terminal,
        "the roomy card admits the waiting load: {:?}",
        row.error
    );
    assert_eq!(
        row.path,
        Some(AdmissionPath::FitsNow),
        "no kill on this path"
    );
    assert_eq!(
        row.state,
        SwapState::Spawn,
        "the row advanced past Idle: the re-gate armed it"
    );
    state.apply_event(
        parked,
        Event::Spawned(Ok(crate::engine::Spawned {
            port: 18_151,
            pid: 8_801,
        })),
    );
    state.apply_event(
        parked,
        Event::Probed(crate::engine::Probe::Ready, Some("rp-load".to_string())),
    );
    let row = &state.jobs[&parked];
    assert!(row.terminal, "the load completes as any ordinary load");
    assert_eq!(row.outcome.as_deref(), Some("loaded"));
    assert_eq!(
        state.slots[rp].occupant.as_ref().map(|o| o.model.as_str()),
        Some("rp-load"),
        "the waiter waited; the engine now serves"
    );
    match reply_rx.try_recv() {
        Ok(Ok(Reply::Loaded(receipt))) => {
            assert_eq!(receipt.model, "rp-load");
            assert_eq!(receipt.port, 18_151);
        }
        other => panic!("the waiter receives the load receipt: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Release-park case: an evictable candidate exists, but the release
/// alone covers the deficit, so the candidate is NOT needed: the row parks on
/// the release and the candidate keeps serving. Pre-fix the
/// `considered.is_empty()` leg refused this exact shape (a false
/// refusal) and the operator had to retry after the release even though
/// nothing needed to be killed. Falsifier: restore
/// `considered.is_empty()` in the release arm and this row takes
/// `AdmitFail` instead of parking.
#[tokio::test]
async fn a_release_park_ignores_a_candidate_it_does_not_need() {
    let dir = crate::testutil::temp_dir("release-park-cell4b");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, _unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-load",
        "engine: mock\ncategory: rp\nvram_gb: 40\n",
        (65.0, Some(70.1)),
        Some(("aux-held", 5.0)),
    );
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, vec![reply_tx]);
    // The planner sees the 5 GiB AUX candidate but cannot cover 40.5 with it,
    // so `considered` is non-empty; the 70.1 GiB release covers the whole
    // deficit, so the candidate is irrelevant and the park ignores it.
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        !row.terminal,
        "the release covers the deficit; the candidate is not needed: {:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::Idle, "the row is parked");
    assert_eq!(state.pending.get(&rp), Some(&parked));
    assert!(
        row.victims.is_empty(),
        "the park never plans or kills the candidate"
    );
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the waiter keeps waiting"
    );
    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.slots[1].state, "serving",
        "the AUX candidate survives the park untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Release-park trap case: the release covers only part of the
/// deficit and the evictable candidate IS needed after the wait. Parking here
/// would turn the wait into a deferred eviction (the re-gate would have to
/// kill the candidate), so the row refuses with its named blockers and the
/// retry after the release runs the ordinary planner. `sum(considered)` never
/// joins the release coverage. Falsifier: cover with
/// `in_flight + releasing + sum(considered)` and this row parks — the wait
/// swallowing a real kill; the `terminal` and `Insufficient` legs go red.
#[tokio::test]
async fn a_release_that_leaves_a_candidate_needed_still_refuses() {
    let dir = crate::testutil::temp_dir("release-park-cell4c");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-load",
        "engine: mock\ncategory: rp\nvram_gb: 40\n",
        (20.0, None),
        Some(("aux-held", 25.0)),
    );
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, vec![reply_tx]);
    // free = 72 - 2 - 70.5 = -0.5, need 40: short 40.5. The release projects
    // 20.0 and the AUX candidate 25.0 — together enough, apart not: after the
    // release the candidate is needed, so this is case c.
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        row.terminal,
        "a wait that would end in a kill is a named refusal, never a park"
    );
    assert!(
        matches!(
            row.error,
            Some(SchedulerError::Admission(
                AdmissionError::Insufficient { .. }
            ))
        ),
        "{:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(
        row.victims.is_empty(),
        "the planner killed nothing on paper for a park that did not happen"
    );
    assert!(state.pending.is_empty(), "no queue place for a refusal");
    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.slots[1].state, "serving",
        "the candidate survives the refusal untouched"
    );
    assert!(
        !state.jobs[&unload].terminal,
        "the release keeps running: the retry after it plans the candidate"
    );
    assert_eq!(state.jobs[&unload].state, SwapState::Dispose);
    match reply_rx.try_recv() {
        Ok(Err(SchedulerError::Admission(AdmissionError::Insufficient { .. }))) => {}
        other => panic!("the caller receives the named refusal: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Blocker-gate leg: a resident a live job has already committed to release is a
/// blocker, never a victim — but the release park waits only when EVERY
/// named blocker is a releasing resident. A second, non-releasing blocker
/// survives the release, so this row refuses with both named instead of
/// parking on a wait that cannot clear it. (The booking park keeps
/// no blocker gate; that behavior is the rule, not an over-park.) Falsifier:
/// drop `blockers_are_releasing` from the release arm and this row parks on a
/// foreign, unreleasable holder: the `terminal` and `Insufficient` legs go red
/// and `pending` grows.
#[tokio::test]
async fn a_second_non_releasing_blocker_refuses_the_release_park() {
    let dir = crate::testutil::temp_dir("release-park-cell2");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-load",
        "engine: mock\ncategory: rp\nvram_gb: 40\n",
        (65.0, Some(70.1)),
        None,
    );
    // A second WORK-category resident nobody is releasing and nothing may
    // evict: the release in progress cannot clear it.
    let held_port = 18_151u16;
    let held_slot = state.slots.len();
    state.slots.push(Slot {
        category: CategoryId::Work,
        index: held_slot,
        engine_port: held_port,
        occupant: Some(Occupant {
            model: "work-held".to_string(),
            need: Gibibytes(30.0),
            ram_need: None,
            port: held_port,
            ticket: fold_ticket(held_port),
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: None,
            stale_config: false,
            serve_id: None,
            measure: None,
            live_measured_gb: None,
        }),
        job: None,
    });
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, vec![reply_tx]);
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        row.terminal,
        "a blocker the release cannot clear refuses the wait, not parks it"
    );
    match &row.error {
        Some(SchedulerError::Admission(AdmissionError::Insufficient {
            blockers, facts, ..
        })) => {
            // Falsifier for the release-credit fix: without crediting the
            // in-progress release in the refusal account, `unreleasable` is
            // the whole occupied card (70.5); with it, the release projection
            // (70.1) leaves only the driver-side remainder.
            let unreleasable = facts
                .unreleasable
                .expect("a live gate sample fills the R3 figures");
            assert!(
                unreleasable.0 < 1.0,
                "bytes leaving under command are not a dead end: {facts:?}"
            );
            assert!(
                blockers
                    .iter()
                    .any(|b| b.reason == "release in progress" && b.model == "work-resident"),
                "the releasing resident is named as a blocker, never re-planned: {blockers:?}"
            );
            assert!(
                blockers
                    .iter()
                    .any(|b| b.reason == "nothing may evict WORK" && b.model == "work-held"),
                "the non-releasing blocker is named beside it: {blockers:?}"
            );
        }
        other => panic!("the refusal names both blockers: {other:?}"),
    }
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(state.pending.is_empty(), "no queue place for a refusal");
    assert!(
        state.slots[held_slot].occupant.is_some(),
        "no resident was killed for a refusal"
    );
    assert!(!state.jobs[&unload].terminal, "the release keeps running");
    match reply_rx.try_recv() {
        Ok(Err(SchedulerError::Admission(AdmissionError::Insufficient { .. }))) => {}
        other => panic!("the caller receives the named refusal: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Floor-reach leg one: the floor is out of reach today, but the in-progress
/// release projection lifts `best_free` to the floor, so the row parks — a
/// floor the release CAN reach is a reachable floor. Falsifier: drop
/// `releasing_credit` from `best_free` and `no_releases_reach` flips this row
/// to `BelowLaunchFloor`: a waitable floor read as a floor the card denies.
#[tokio::test]
async fn a_release_that_reaches_the_launch_floor_parks() {
    let dir = crate::testutil::temp_dir("release-park-cell3-reach");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, _unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-floor",
        "engine: mock\ncategory: rp\nvram_gb: 10\nlaunch_only_with_at_least_vram_free: 40\n",
        (65.0, Some(70.1)),
        None,
    );
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, Vec::new());
    // free = -0.5: the floor 40 is short 40.5, and
    // best_free = -0.5 + 70.1 (release) = 69.6 clears 40 - 2 tolerance.
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        !row.terminal,
        "the release lifts the floor into reach, so this is a wait: {:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::Idle, "the row is parked");
    assert_eq!(state.pending.get(&rp), Some(&parked));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Floor-reach leg two: even the release projection cannot lift `best_free`
/// to the floor, so `no_releases_reach` still refuses before any wait — a
/// park would end only in the same refusal, late and un-named. Falsifier:
/// park whenever `releasing > 0` regardless of the floor guard and this row
/// parks on a floor the card denies: the `BelowLaunchFloor` leg goes red.
#[tokio::test]
async fn a_release_that_cannot_reach_the_launch_floor_refuses() {
    let dir = crate::testutil::temp_dir("release-park-cell3-deny");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, _unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-floor",
        "engine: mock\ncategory: rp\nvram_gb: 10\nlaunch_only_with_at_least_vram_free: 72\n",
        (65.0, Some(70.1)),
        None,
    );
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, Vec::new());
    // best_free = -0.5 + 70.1 = 69.6 < 72 - 2: the floor stays out of reach
    // even after the release lands.
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        row.terminal,
        "a floor the release cannot reach refuses, it does not park"
    );
    assert!(
        matches!(
            row.error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "{:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(state.pending.is_empty(), "no queue place for a refusal");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unconfirmed release: the release terminalizes UNCONFIRMED and the resident stays
/// installed (G12). The parked row re-gates — the terminal funnel cannot tell
/// a dead release from a live one — and now refuses with the named,
/// unreleasable blocker: terminal, answered once, no re-park, no second wait,
/// no spin. Falsifier: let a re-gate park whenever its slot is empty again
/// and this row waits forever on a release that already failed: the
/// `terminal`, `Insufficient`, and empty-`pending` legs go red.
#[tokio::test]
async fn an_unconfirmed_release_regates_the_parked_row_to_a_named_refusal() {
    let dir = crate::testutil::temp_dir("release-park-cell6");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting, unload, rp) = release_short_fixture(
        &dir,
        fold_state(&dir),
        "rp-load",
        "engine: mock\ncategory: rp\nvram_gb: 40\n",
        (65.0, Some(70.1)),
        None,
    );
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, vec![reply_tx]);
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    assert_eq!(state.pending.get(&rp), Some(&parked), "the row waits");
    // The reap cannot prove the death: the unload terminalizes with the
    // release unconfirmed, and the resident stays installed on purpose.
    state.apply_event(
        unload,
        Event::Released(Err(crate::engine::SpawnError::KillFailed {
            holder: "work-resident".to_string(),
            reason: "release not confirmed within the deadline".to_string(),
        })),
    );
    assert!(
        state.jobs[&unload].terminal,
        "the failed release still reaches a terminal"
    );
    assert!(
        state.slots[0].occupant.is_some(),
        "an unconfirmed release keeps the resident installed (G12)"
    );
    assert_eq!(
        state.slots[rp].job,
        Some(parked),
        "the terminal funnel re-gates the parked row anyway"
    );
    assert_eq!(state.jobs[&parked].state, SwapState::Admit);
    // The re-gate: no release credit anymore (the unload row is terminal), a
    // WORK resident nobody may evict — the ordinary named refusal.
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    let row = &state.jobs[&parked];
    assert!(
        row.terminal,
        "the wait cannot outlive the release it waited on"
    );
    assert!(
        matches!(
            row.error,
            Some(SchedulerError::Admission(
                AdmissionError::Insufficient { .. }
            ))
        ),
        "{:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(state.pending.is_empty(), "no re-park: the wait cannot spin");
    match reply_rx.try_recv() {
        Ok(Err(SchedulerError::Admission(AdmissionError::Insufficient { .. }))) => {}
        other => panic!("the caller receives the named refusal once: {other:?}"),
    }
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ),
        "exactly once"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The matrix section 12 cross-slot scenario: while the RP row is parked on
/// the WORK release, a client asks for a DIFFERENT WORK model. `on_swap`
/// registers the new WORK in `pending[WORK]` and supersedes the running job
/// on that slot — the unload row goes `CancelRequested` while the victim
/// release re-arms and continues (the slot still reports `releasing`); the RP
/// park is on another category and is NOT touched: no supersede, no answer,
/// queue place and waiters kept. Applying the confirmed release then seats
/// the queued WORK load on the freed slot and re-gates the parked RP row,
/// and the re-gate re-parks on the newer booking (a full spawn is out of
/// scope here). Falsifier: let `on_swap` sweep parked rows across categories
/// or answer the parked row on supersede, let the confirmed release leave the
/// queued WORK unseated or answer the park instead of re-gating it, or let
/// the re-gate die instead of re-parking, and the RP
/// `pending`/not-terminal/unanswered-waiter, WORK seated, RP re-gate, or
/// re-park legs go red.
#[tokio::test]
async fn a_work_arrival_during_a_release_park_supersedes_the_unload_not_the_park() {
    let dir = crate::testutil::temp_dir("release-park-crossslot");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // The superseded unload's re-armed reap runs for real: the confirming
    // stand-in runtime, so the release continues exactly as the matrix
    // records instead of vanishing with the aborted task.
    let (base, _runtime, mut rx) = death_state();
    let (mut state, waiting, unload, rp) = release_short_fixture(
        &dir,
        base,
        "rp-load",
        "engine: mock\ncategory: rp\nvram_gb: 40\n",
        (65.0, Some(70.1)),
        None,
    );
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(rp, waiting, vec![reply_tx]);
    state.finish_admit_card(parked, Ok(gate_sample(70.5)));
    assert_eq!(state.pending.get(&rp), Some(&parked), "the RP row parks");
    let work2_yaml = dir.join("work-2.yaml");
    std::fs::write(&work2_yaml, "engine: mock\ncategory: work\nvram_gb: 30\n").expect("seed work2");
    let work2 = rescan_entry(&work2_yaml, "work-2");
    state.catalog.push(work2);
    let (third_tx, mut third_rx) = tokio::sync::oneshot::channel();
    state.on_swap("work-2".to_string(), CategoryId::Work, third_tx);
    // The RP park survives untouched: same row, same queue place, not
    // terminal, waiter still unanswered.
    assert_eq!(
        state.pending.get(&rp),
        Some(&parked),
        "the parked RP row keeps its queue place: a cross-category arrival does not touch it"
    );
    assert!(
        !state.jobs[&parked].terminal,
        "the RP row is not superseded, answered, or cancelled"
    );
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "and its waiter keeps waiting"
    );
    // The new WORK row waits in `pending[WORK]` on the busy slot.
    let newer = *state
        .pending
        .get(&0)
        .expect("the new WORK row queues on the WORK slot");
    assert_eq!(state.jobs[&newer].model, "work-2");
    assert_eq!(state.jobs[&newer].state, SwapState::Idle);
    assert_ne!(newer, parked, "a different row, on a different slot");
    // The superseded unload row is `CancelRequested` but NOT terminal: the
    // victim release continues under the re-armed reap, and the slot still
    // reads `releasing` to the status page.
    let unload_row = &state.jobs[&unload];
    assert_eq!(
        unload_row.state,
        SwapState::CancelRequested,
        "the newer load supersedes the unload job..."
    );
    assert!(
        !unload_row.terminal,
        "...without terminalizing it before the release lands"
    );
    assert!(
        state.slots[0].occupant.is_some(),
        "the releasing resident still holds the slot"
    );
    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.slots[0].state, "releasing",
        "the victim release is still the committed window"
    );
    assert!(
        matches!(
            third_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the new WORK waits too: its slot is not free yet"
    );
    // The re-armed release runs to its confirmation (the parked row's armed
    // witness sample may land first — the pins drive gates by hand, so it is
    // only drained here).
    let mut release = None;
    for _ in 0..4 {
        let message = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the actor channel delivers inside the test deadline")
            .expect("the actor channel is open");
        let ActorMsg::Event { job, event } = message else {
            panic!("expected actor events on the channel");
        };
        match event {
            Event::Admitted(_, _) => {
                assert_eq!(job, parked, "only the parked row ever armed a sample");
            }
            other => {
                release = Some((job, other));
                break;
            }
        }
    }
    let (release_job, event) = release.expect("the superseded unload's release is re-armed");
    assert_eq!(
        release_job, unload,
        "the release continues under the cancel-requested unload row"
    );
    assert!(
        matches!(event, Event::Released(Ok(_))),
        "and confirms: {event:?}"
    );
    // The terminal funnel's outcomes under the confirmed release (these are
    // outcomes, not an intra-turn call order: the pin cannot falsify one):
    // the confirmed release seats the queued WORK load on the freed slot and
    // re-gates the parked RP row - the park keeps its wait and is not
    // answered - and the re-gate re-parks on the newer booking.
    state.apply_event(release_job, event);
    assert_eq!(
        state.slots[0].job,
        Some(newer),
        "the confirmed release seats the queued WORK load"
    );
    assert_eq!(
        state.jobs[&newer].state,
        SwapState::Admit,
        "the WORK load re-gates onto its now-free slot"
    );
    assert!(
        !state.jobs[&newer].terminal,
        "the WORK load keeps waiting for its gate"
    );
    assert!(!state.pending.contains_key(&0), "its queue place is spent");
    assert_eq!(
        state.slots[rp].job,
        Some(parked),
        "the same terminal re-gates the parked RP row"
    );
    assert_eq!(state.jobs[&parked].state, SwapState::Admit);
    assert!(
        !state.jobs[&parked].terminal,
        "the park re-gates, it is not answered"
    );
    // Re-gated with the release landed and the newer WORK booking (30 GiB)
    // the only in-flight source: free = 72 - 2 - 25 - 30 = 15, the 25 GiB
    // deficit is covered by the booking alone, so the row parks again — one
    // wait can end in the next wait, never in a spin.
    state.finish_admit_card(parked, Ok(gate_sample(25.0)));
    assert!(
        !state.jobs[&parked].terminal,
        "the park re-parks on the remaining booking: {:?}",
        state.jobs[&parked].error
    );
    assert_eq!(
        state.pending.get(&rp),
        Some(&parked),
        "the wait continues with its queue place"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
