use super::*;

// --- unload during a swap, and the load parked behind it ----------------------------

/// The model that ends up queued on the category's only slot, behind the swap.
const PARKED_MODEL: &str = "model-c";

/// The three-model, single-slot WORK fixture both findings need: a resident engine
/// whose terminate can be held, a swap that cannot get past `Admit`, and
/// [`PARKED_MODEL`] to queue behind it. The release deadline is short so an
/// unconfirmed release terminalizes quickly rather than at the default.
fn parked_swap_harness() -> (SchedulerHandle, MockRuntime) {
    let cfg = AppConfig {
        release_timeout_s: 2,
        ..config(10)
    };
    let witness = GatedWitness::new(1); // one sample: the resident load's admission
    let (handle, runtime) = harness5_witness(
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
            (PARKED_MODEL, "mock", CategoryId::Work, Some(4.0), ""),
        ],
        Arc::new(witness),
    );
    (handle, runtime)
}

/// Serves [`HOLD_MODEL`] and submits the swap to [`INCOMING_MODEL`], waiting until it
/// sits frozen in `Admit` (the gate has no sample left for it), so the victim still
/// holds its slot and a live engine when the operator's cancel lands.
async fn serve_and_submit_frozen_swap(
    handle: &SchedulerHandle,
) -> simpleharn::scheduler::SwapReceipt {
    let victim = serve_hold_model(handle).await;
    submit_and_vanish(handle, INCOMING_MODEL, CategoryId::Work).await;
    wait_until(handle, 5, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.state == SwapState::Admit)
    })
    .await;
    victim
}

/// The job id of `model`'s row, whichever stage it is in.
fn job_of(status: &SchedulerStatus, model: &str) -> JobId {
    status
        .jobs
        .iter()
        .find(|j| j.model == model)
        .unwrap_or_else(|| panic!("`{model}` must be in the job ledger: {:?}", status.jobs))
        .id
}

#[tokio::test]
async fn unload_during_a_swap_frees_the_slot_after_cancelling_the_swap() {
    // `unload_slots` once chose cancel OR unload. A slot with an occupant
    // *and* an in-flight swap is where that choice loses the card: the cancel's reap
    // cannot prove the victim's release (the held-kill fake deliberately keeps an engine
    // nobody proved dead installed), so before the fix the operator was told `Done`
    // while the model stayed loaded and serving.
    let (handle, runtime) = parked_swap_harness();
    let victim = serve_and_submit_frozen_swap(&handle).await;
    assert_eq!(
        runtime.server_count(),
        1,
        "only the resident engine is live"
    );
    // Hold the victim's kills BY TARGET, and observe what the fake actually parked:
    // the scenario used to hinge on a bare kill COUNT from the
    // fake's budget (`hold_next_kills(1)`), so a correct product change to the reap
    // path re-armed the fake silently. The cancel's reap of the victim stays
    // unprovable (the fake keeps the engine installed over it); the test then
    // releases the gate itself, turning the operator's follow-up unload into a
    // terminate it can prove.
    runtime.hold_kill_of(victim.port);

    assert!(
        matches!(
            handle
                .unload_model(HOLD_MODEL)
                .await
                .expect("unload accepted"),
            Reply::Unloaded {
                unloading: 1,
                jobs_cancelled: 1
            }
        ),
        "the unload of a swapping slot commits to free the occupant behind the cancel"
    );

    wait_until(&handle, 10, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.terminal)
    })
    .await;
    // What was held unprovable was exactly the victim's reap — the parked SET, not
    // the fake's arithmetic.
    let held = runtime.held_kills();
    assert!(
        !held.is_empty() && held.iter().all(|(port, _)| *port == victim.port),
        "the parked kills are the victim's: {held:?} vs victim port {}",
        victim.port
    );
    runtime.release_held_kills();
    let status = handle.status().await.expect("actor alive");
    let swap = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the cancelled swap is in the ledger");
    assert_eq!(swap.outcome.as_deref(), Some("cancelled"), "{swap:?}");
    assert!(
        swap.error
            .as_deref()
            .unwrap_or("")
            .contains("unloaded by request"),
        "{swap:?}"
    );
    assert!(
        !swap.is_fault,
        "an operator's unload is not a fault: {swap:?}"
    );
    // The cancel proved nothing about the victim: no release timing, no released
    // victim. Whatever freed the slot therefore cannot be attributed to the swap.
    assert!(
        swap.victims_released.is_empty(),
        "the cancelled swap never proved a release: {:?}",
        swap.victims_released
    );
    assert!(
        swap.timings.release_ms.is_none(),
        "and its release stage never completed: {:?}",
        swap.timings
    );
    assert_eq!(
        runtime.spawn_count(),
        1,
        "a swap cancelled in Admit never spawns"
    );

    // The second half of that one unload command: the slot's own unload job, started
    // when the cancel it triggered terminalized. Without it the card stays spent, so
    // this wait is the falsifier: pre-fix the slot keeps its occupant and live engine.
    wait_until(&handle, 10, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let unload = status
        .jobs
        .iter()
        .find(|j| j.kind == JobKind::Unload && j.model == HOLD_MODEL)
        .expect("one unload must have run for the slot the cancel left occupied");
    assert_eq!(unload.outcome.as_deref(), Some("unloaded"), "{unload:?}");
    assert!(
        unload.timings.release_ms.is_some(),
        "the unload proved its own release: {unload:?}"
    );
    let swap = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the cancelled swap is in the ledger");
    assert!(
        unload.id > swap.id,
        "the unload followed the cancel it was waiting on: {:?} then {:?}",
        swap.id,
        unload.id
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "a single unload must leave no live engine behind"
    );
    assert!(
        runtime.is_dead(victim.port).await,
        "and no engine answering on the freed slot's port"
    );
}

#[tokio::test]
async fn unload_replies_acceptance_before_any_release_evidence() {
    // The second half of the cancel-OR-unload defect, plus reply honesty: `unload_slots`
    // answers `Reply::Unloaded` as soon as the commands are dispatched, while the
    // kill it triggered is still in flight. The reply names the COMMITMENT (one
    // slot unloading, one job cancelled), never a release: a client still cannot
    // read a freed slot out of it, and the job ledger stays the release truth.
    let (handle, runtime) = parked_swap_harness();
    serve_and_submit_frozen_swap(&handle).await;

    assert!(
        matches!(
            handle
                .unload_model(HOLD_MODEL)
                .await
                .expect("unload accepted"),
            Reply::Unloaded {
                unloading: 1,
                jobs_cancelled: 1
            }
        ),
        "the reply is an acceptance while the release is still unproven"
    );

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "the acceptance the caller already holds arrived with the slot still occupied"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "and with the engine still live: no release had been proven yet"
    );
}

#[tokio::test]
async fn a_parked_load_is_answered_when_its_predecessor_keeps_the_occupant() {
    // The parked-load scenario: A serving, swap B frozen in Admit,
    // swap C parked behind it, B cancelled. B terminalizes with A still installed, so
    // C's queue place can never be satisfied: it must be answered, not left hanging,
    // and it must not fire later when the operator finally frees the slot.
    let (handle, runtime) = parked_swap_harness();
    let victim = serve_and_submit_frozen_swap(&handle).await;
    // Hold the victim's kills BY TARGET: parking C cancels B once
    // ("superseded by a newer load") and the operator's cancel below re-issues that
    // same reap. Both stay unprovable — and the test now ASSERTS the parked set the
    // fake really holds, instead of trusting a bare `hold_next_kills(2)` budget
    // whose count tracked the product's internal kill calls rather than the property.
    runtime.hold_kill_of(victim.port);

    let client = handle.clone();
    let parked = tokio::spawn(async move { client.swap(PARKED_MODEL, CategoryId::Work).await });
    wait_until(&handle, 5, |s| is_queued(s, PARKED_MODEL)).await;

    let swap = job_of(&handle.status().await.expect("actor alive"), INCOMING_MODEL);
    // Parking C already cancelled B; this is the operator's own cancel of the same job,
    // which must still terminalize it.

    // The parked SET, not the fake's arithmetic: the supersede
    // cancel's reap is parked on the victim's ticket the moment C queues — and after
    // the operator's cancel re-issues that same reap, a victim kill is parked again
    // (each cancel aborts the previous reap task, so exactly one is parked at any
    // instant). Both observations are checked: the parked ticket is the victim's,
    // never some other engine's.
    let held_deadline = Instant::now() + Duration::from_millis(1_500);
    let held = loop {
        let held = runtime.held_kills();
        if !held.is_empty() {
            break held;
        }
        assert!(
            Instant::now() < held_deadline,
            "the supersede cancel must park its reap of the victim: {held:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        held.iter().all(|(port, _)| *port == victim.port),
        "the parked kills are the victim's, not some other engine's: {held:?}"
    );

    handle.cancel(swap).await.expect("cancel accepted");
    let held_deadline = Instant::now() + Duration::from_millis(1_500);
    let held = loop {
        let held = runtime.held_kills();
        if !held.is_empty() {
            break held;
        }
        assert!(
            Instant::now() < held_deadline,
            "the operator's cancel re-issues the victim's reap and parks it again: {held:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        held.iter().all(|(port, _)| *port == victim.port),
        "the re-issued parked kill is still the victim's: {held:?}"
    );

    let answer = tokio::time::timeout(Duration::from_secs(8), parked)
        .await
        .expect("a parked load's waiter must be answered, not left to hang")
        .expect("the parked load's task must not panic");
    let error = answer.expect_err("a parked load whose slot never freed cannot report a load");
    assert!(
        matches!(error, SchedulerError::Cancelled(_)),
        "the parked load must be told it was cancelled, got {error}"
    );
    assert!(
        error.to_string().contains("did not free the slot"),
        "the answer must name why, so a client can act on it: {error}"
    );

    // Open the gate so the unload below can prove its terminate.
    runtime.release_held_kills();
    // Freeing the slot now must not resurrect the load its caller was answered about.
    handle
        .unload_model(HOLD_MODEL)
        .await
        .expect("unload accepted");
    wait_until(&handle, 10, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![None],
        "the unload must have freed the slot"
    );
    assert!(
        !is_queued(&status, PARKED_MODEL),
        "the answered load must no longer hold the slot's queue place"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "an abandoned parked load must not start when the slot is finally freed"
    );
    let parked_row = status
        .jobs
        .iter()
        .find(|j| j.model == PARKED_MODEL)
        .expect("the parked load keeps its ledger row");
    assert_eq!(
        parked_row.outcome.as_deref(),
        Some("cancelled"),
        "{parked_row:?}"
    );
    assert!(
        !parked_row.is_fault,
        "a busy card is not an engine fault: {parked_row:?}"
    );
}

#[tokio::test]
async fn unload_drops_a_parked_load_from_the_slot_it_targets() {
    // The `unload_slots` arm that drops a load PARKED on a targeted slot
    // (src/scheduler/mod.rs "unloaded by request before it started") had no test in
    // any file; the sibling `finish`-time strand-arm is pinned twice above, this one
    // never. Remove the arm and the parked-load defect reopens twice over: the parked
    // waiter hangs (it is answered by nothing else on this path), and the row keeps
    // the slot's queue place, so it fires on the slot's LATER finish (`finish` ->
    // `pending.remove` -> `start_pending`) - the load the operator explicitly
    // abandoned starts after the unload the operator explicitly asked for. Red on
    // the removed arm: the `jobs_cancelled` fact reads 0 and the parked waiter is
    // never answered with the unload's Cancelled (the 8 s below trips).
    let (handle, runtime) = parked_swap_harness();
    let victim = serve_and_submit_frozen_swap(&handle).await;
    // Hold the victim's kills BY TARGET BEFORE parking: parking
    // PARKED_MODEL supersede-cancels the frozen swap, and that cancel's reap of the
    // victim parks on the hold, so the parked row genuinely holds the slot's queue
    // place while the operator's unload lands.
    runtime.hold_kill_of(victim.port);
    let client = handle.clone();
    let parked = tokio::spawn(async move { client.swap(PARKED_MODEL, CategoryId::Work).await });
    wait_until(&handle, 5, |s| is_queued(s, PARKED_MODEL)).await;

    // The desired slot set: `on_unload` targets every slot of the category; this
    // fixture has the one WORK slot. It commits to unload (occupied, carrying a
    // job), and the command's only cancel fact is the parked row - the swap already
    // carries `CancelRequested` from the park's supersede, so re-cancelling it is no
    // new fact.
    assert!(
        matches!(
            handle
                .unload(CategoryId::Work)
                .await
                .expect("unload accepted"),
            Reply::Unloaded {
                unloading: 1,
                jobs_cancelled: 1
            }
        ),
        "the unload must drop the parked row and commit the occupied slot"
    );

    // The parked caller is answered here and now, while the kill is still parked -
    // not when the slot finally frees. With the arm the answer is this Cancelled;
    // without it the waiter never answers at all.
    let answer = tokio::time::timeout(Duration::from_secs(8), parked)
        .await
        .expect("the unload that dropped the parked load must answer its waiter")
        .expect("the parked load's task must not panic");
    let error = answer.expect_err("a load dropped by unload cannot report a load");
    assert!(
        matches!(error, SchedulerError::Cancelled(_)),
        "the parked load must be told it was cancelled, got {error}"
    );
    assert!(
        error
            .to_string()
            .contains("unloaded by request before it started"),
        "the answer must name the unload that dropped it: {error}"
    );

    let status = handle.status().await.expect("actor alive");
    let parked_row = status
        .jobs
        .iter()
        .find(|j| j.model == PARKED_MODEL)
        .expect("the parked load keeps its ledger row");
    assert!(parked_row.terminal, "{parked_row:?}");
    assert_eq!(
        parked_row.outcome.as_deref(),
        Some("cancelled"),
        "{parked_row:?}"
    );
    assert!(
        parked_row
            .error
            .as_deref()
            .unwrap_or("")
            .contains("unloaded by request before it started"),
        "the row records why the queued load was dropped: {parked_row:?}"
    );
    assert!(
        !parked_row.is_fault,
        "an operator's unload is not a fault: {parked_row:?}"
    );
    assert!(
        !is_queued(&status, PARKED_MODEL),
        "the unload must have taken the slot's queue place"
    );

    // Land the parked reap: the swap terminalizes with the victim's release proven,
    // the slot frees - and the abandoned load must NOT resurrect. Its queue place is
    // gone, so `finish` finds no pending row to arm: the spawn count is the proof
    // against the double-spawn half of the parked-load defect.
    runtime.release_held_kills();
    wait_until(&handle, 10, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    assert_eq!(
        runtime.spawn_count(),
        1,
        "only the resident ever spawned: a dropped parked load must never fire late"
    );
    assert_eq!(runtime.server_count(), 0, "the freed card holds no engine");
    assert!(
        runtime.is_dead(victim.port).await,
        "the victim's parked release landed"
    );
}

#[tokio::test]
async fn a_parked_load_has_a_queued_ledger_row_before_it_starts() {
    // The ledger half: `/har/jobs` is what the honest-ledger rule exists to make
    // honest,
    // and a parked load used to have no row at all while its request hung. The row it
    // gets at park time is the same one it runs on when it does start, and the one
    // the answered error is recorded against.
    let (handle, _runtime) = parked_swap_harness();
    serve_and_submit_frozen_swap(&handle).await;
    submit_and_vanish(&handle, PARKED_MODEL, CategoryId::Work).await;
    wait_until(&handle, 5, |s| is_queued(s, PARKED_MODEL)).await;

    let status = handle.status().await.expect("actor alive");
    let parked = status
        .jobs
        .iter()
        .find(|j| j.model == PARKED_MODEL)
        .expect("a parked load must have a ledger row while its caller waits");
    assert_eq!(parked.kind, JobKind::Load, "a queued load is a load row");
    assert!(!parked.terminal, "a queued load has not finished");
    assert_eq!(
        parked.state,
        SwapState::Idle,
        "queued: registered, never admitted ({parked:?})"
    );
    assert!(
        parked.error.is_none(),
        "a queued load has failed nothing: {parked:?}"
    );
    let swap = job_of(&status, INCOMING_MODEL);
    assert!(
        status
            .slots
            .iter()
            .any(|s| s.category == CategoryId::Work && s.loading_job == Some(swap)),
        "the slot still runs the swap this load is parked behind: {:?}",
        status.slots
    );

    handle.cancel(swap).await.expect("cancel accepted");
    wait_until(&handle, 8, |s| {
        s.jobs.iter().any(|j| j.model == PARKED_MODEL && j.terminal)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let parked = status
        .jobs
        .iter()
        .find(|j| j.model == PARKED_MODEL)
        .expect("the parked load's row is kept after it is dropped");
    assert_eq!(parked.outcome.as_deref(), Some("cancelled"), "{parked:?}");
    assert!(
        parked
            .error
            .as_deref()
            .unwrap_or("")
            .contains("did not free the slot"),
        "the row records why the queued load was dropped: {parked:?}"
    );
    assert!(!parked.is_fault, "{parked:?}");
}

#[tokio::test]
async fn a_parked_load_that_starts_runs_on_the_row_it_queued_with() {
    // The row-identity half: the ledger contract claims `start_pending` "arms
    // that same row rather than creating a second one", and `/har/jobs` is the
    // surface the honest-ledger rule exists to make honest. B parks behind A's
    // in-flight supersede
    // and is the parked load that ACTUALLY starts; the loaded receipt must carry
    // the single ledger row B registered at park time. If `start_pending` created
    // a second row — or armed a different one — the queued row would linger in the
    // ledger as a second `model-b` load row (length fails) or the receipt would
    // name an id no queued row ever held (equality fails).
    let (handle, runtime) = harness(
        config(10),
        &[
            ("model-a", "mock-slow", CategoryId::Work, Some(4.0)),
            ("model-b", "mock", CategoryId::Work, Some(4.0)),
        ],
    );
    submit_and_vanish(&handle, "model-a", CategoryId::Work).await;
    let Reply::Loaded(receipt) = handle
        .swap("model-b", CategoryId::Work)
        .await
        .expect("b loads")
    else {
        panic!("expected Loaded")
    };
    wait_until(&handle, 5, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work) == vec![Some("model-b".to_string())]
    })
    .await;
    assert_eq!(runtime.server_count(), 1, "only B's engine may survive");

    let status = handle.status().await.expect("actor alive");
    let b_rows: Vec<_> = status
        .jobs
        .iter()
        .filter(|j| j.model == "model-b" && j.kind == JobKind::Load)
        .collect();
    assert_eq!(
        b_rows.len(),
        1,
        "queue and start must share ONE row: {:?}",
        status
            .jobs
            .iter()
            .map(|j| (j.id, j.model.clone(), format!("{:?}", j.kind)))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        b_rows[0].id, receipt.job,
        "the Loaded receipt carries the parked row's id, not a fresh one"
    );
    assert_eq!(b_rows[0].state, SwapState::Serving, "{:?}", b_rows[0]);
}

#[tokio::test]
async fn cancel_names_a_stale_job_id_and_answers_done_for_a_terminal_row() {
    // The two outer arms of `on_cancel` were never exercised anywhere -
    // every cancel site in the suite targets a live job. `/har/jobs` offers Cancel
    // on rows that may have just terminalized and the page reloads across
    // restarts, so a stale id must get the TYPED `UnknownJob` (the API maps it to
    // HTTP 404): answering `Done` for a job that never existed is a silent lie.
    // The terminal row is the other arm: nothing left to cancel is honestly
    // `Done`, and the row must stay exactly as it was - no re-open, no rewrite.
    let (handle, _runtime) = harness(
        config(10),
        &[("model-a", "mock", CategoryId::Work, Some(4.0))],
    );
    handle
        .swap("model-a", CategoryId::Work)
        .await
        .expect("model-a loads");
    let status = handle.status().await.expect("actor alive");
    let row_id = job_of(&status, "model-a");
    let row = status
        .jobs
        .iter()
        .find(|j| j.id == row_id)
        .expect("invariant: job_of returned an existing row");
    assert!(row.terminal, "premise: the load row is already terminal");
    assert_eq!(row.outcome.as_deref(), Some("loaded"), "{row:?}");

    // Stale id: the typed refusal, naming the id the client asked about.
    let error = handle
        .cancel(JobId(999_999))
        .await
        .expect_err("a never-registered id cannot cancel anything");
    assert!(
        matches!(error, SchedulerError::UnknownJob(JobId(999_999))),
        "a stale id must answer typed UnknownJob, got {error}"
    );
    assert_eq!(
        error.to_string(),
        "unknown job job-999999",
        "and name the id"
    );

    // Terminal row: accepted as `Done`, the ledger untouched - the post-terminal
    // cancel is a no-op with a receipt, never a fresh job or a re-opened row.
    assert_eq!(
        handle.cancel(row_id).await.expect("actor alive"),
        Reply::Done,
        "a terminal row owes no work: the arm answers Done"
    );
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        status.jobs.len(),
        1,
        "neither cancel may create a ledger row: {:?}",
        status.jobs
    );
    let row = status
        .jobs
        .iter()
        .find(|j| j.id == row_id)
        .expect("the terminal row is kept");
    assert!(
        row.terminal,
        "a post-terminal cancel must not re-open the row: {row:?}"
    );
    assert_eq!(
        row.outcome.as_deref(),
        Some("loaded"),
        "and must not rewrite its outcome: {row:?}"
    );
}

#[tokio::test]
async fn restart_during_a_frozen_swap_cancels_the_swap_and_keeps_the_occupant() {
    // `on_restart` only takes the replacement path when the slot has
    // no job; with a swap in flight it silently degrades to `on_swap` — a crash-retry
    // recovery gesture that becomes a supersede. Nobody had observed what that
    // degradation does. Pinned here, honestly: the in-flight swap is cancelled,
    // HOLD_MODEL ends still installed on exactly its original live engine, nothing
    // spawns, and the restart's own queued load is answered (never left hanging)
    // with the unproven-release rule — the occupant nobody proved dead stays.
    let (handle, runtime) = parked_swap_harness();
    let victim = serve_and_submit_frozen_swap(&handle).await;

    let client = handle.clone();
    // The restart names the live occupant's port: this is the honest
    // crash-retry shape, the caller witnessed exactly this engine. The slot
    // carries a job here, so the port never reaches the successor check.
    let victim_port = victim.port;
    let restarting = tokio::spawn(async move {
        client
            .restart(HOLD_MODEL, CategoryId::Work, victim_port)
            .await
    });

    wait_until(&handle, 10, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.terminal)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let swap = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the superseded swap is on the ledger");
    assert_eq!(
        swap.outcome.as_deref(),
        Some("cancelled"),
        "the restart's degraded arm cancelled the swap: {swap:?}"
    );
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "the model the operator restarted keeps serving on the slot it held"
    );
    assert_eq!(runtime.server_count(), 1, "one live engine — the original");
    assert_eq!(
        runtime.spawn_count(),
        1,
        "the degraded restart spawned nothing"
    );
    assert!(
        !runtime.is_dead(victim.port).await,
        "and that engine is still answering"
    );

    let answer = tokio::time::timeout(Duration::from_secs(15), restarting)
        .await
        .expect("the restart waiter must be answered, not left hanging")
        .expect("the restart task must not panic");
    assert!(
        matches!(answer, Err(SchedulerError::Cancelled(_))),
        "the queued restart is answered against the kept occupant, got {answer:?}"
    );
}

#[tokio::test]
async fn a_late_restart_never_evicts_the_successor_the_first_retry_installed() {
    // Request A refuses on the corpse's port, restarts, and the reload
    // installs successor S. Request B — whose own send also refused on the corpse
    // earlier — dispatches its Restart after S is Installed. The old `on_restart`
    // found the slot by (model, category) alone and booked the healthy successor as
    // the victim of an already-recovered crash, evicting S's clients mid-generation.
    // The refused port the proxy now transmits tells the two generations apart.
    let (handle, runtime) = harness(
        config(10),
        &[("dies", "mock-dies-once", CategoryId::Work, Some(4.0))],
    );
    let Reply::Loaded(first) = handle
        .swap("dies", CategoryId::Work)
        .await
        .expect("first load")
    else {
        panic!("expected Loaded for the first load")
    };

    // Wait past the mock-dies-once self-death (scheduled 1.5 s after spawn): the
    // corpse is real, and the slot still names it as the occupant.
    tokio::time::sleep(Duration::from_millis(1_700)).await;

    // Request A's restart: the port matches the occupant, so the standard
    // reap + reload runs and a successor is Installed.
    let Reply::Loaded(second) = handle
        .restart("dies", CategoryId::Work, first.port)
        .await
        .expect("the first restart reloads")
    else {
        panic!("expected Loaded for the first restart")
    };
    assert_eq!(runtime.spawn_count(), 2, "the first retry really reloaded");

    // Port-reuse residual (documented in `on_restart`): if the allocator handed the
    // successor the corpse's exact port, no port-only check can tell the generations
    // apart — that corner stays open by design. The proxy can only ever have
    // witnessed the corpse's port, so here B names a port the successor provably
    // does not serve, which is exactly the case a real second refuse carries.
    let corpse_port = if second.port == first.port {
        first.port.wrapping_add(1)
    } else {
        first.port
    };

    // Request B's late restart: same model, the corpse's port, successor Installed.
    let reply = handle
        .restart("dies", CategoryId::Work, corpse_port)
        .await
        .expect("the late restart is answered");
    assert!(
        matches!(&reply, Reply::Already { port, .. } if *port == second.port),
        "the late Restart must be answered `Already` against the live successor, \
         not reload a third time: {reply:?}"
    );
    assert_eq!(
        runtime.spawn_count(),
        2,
        "the late Restart may not spawn a third engine for an already-recovered crash"
    );
    assert_eq!(runtime.server_count(), 1, "one live engine — the successor");
    assert!(
        !runtime.is_dead(second.port).await,
        "the successor keeps serving; its clients keep their engine"
    );
    let status = handle.status().await.expect("actor alive");
    let slot = status
        .slots
        .iter()
        .find(|s| s.model.as_deref() == Some("dies"))
        .expect("the successor stays installed");
    assert_eq!(slot.port, Some(second.port), "the successor keeps its port");
}

#[tokio::test]
async fn the_idle_sweep_never_starts_an_unload_behind_an_in_flight_job() {
    // The sweep half of the wedge class:
    // `on_tick` skips any slot whose `job` is `Some` — the filter that stops the 2 s
    // idle tick from unloading a slot mid-Quiesce/Dispose/Install (the double-release
    // / slot-with-no-engine wedge class). It is a guard that can silently disappear:
    // with the filter inverted, the tick below would unload HOLD_MODEL out from
    // under the frozen swap and record an `idle timeout` row — both fail here.
    // WORK carries the nonzero `idle_timeout_s` because the swapping slot is WORK:
    // the sweep only ever sees a slot whose category opts in, and the guard must
    // still hold when the age is over the threshold.
    let mut cfg = AppConfig {
        release_timeout_s: 2,
        ..config(10)
    };
    cfg.category_mut(CategoryId::Work)
        .expect("default config has WORK")
        .idle_timeout_s = 1;
    let witness = GatedWitness::new(1); // one sample: the resident load's admission
    let (handle, runtime) = harness5_witness(
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
            (PARKED_MODEL, "mock", CategoryId::Work, Some(4.0), ""),
        ],
        Arc::new(witness),
    );
    serve_and_submit_frozen_swap(&handle).await;
    // The frozen swap IS the in-flight job: model-b sits at Admit (GatedWitness
    // exhausted), so `slot.job` is `Some` for the whole window. (A second parked load
    // can't co-exist here: parking model-c would supersede-cancel the frozen model-b,
    // leaving the slot genuinely idle — that parked row's fate is the separate
    // the parked-load tests' subject, not the sweep guard.)

    // Past two sweep ticks (IDLE_TICK is 2 s) while the occupant's age is well over
    // the 1 s threshold: the in-flight job is the only thing that may spare it.
    tokio::time::sleep(Duration::from_millis(4_500)).await;
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "the sweep may not unload a slot that carries a job"
    );
    // The swap really is still in flight — otherwise the sweep had no job to spare.
    assert!(
        slot_model(&status, CategoryId::Work)[0].is_some()
            && status.jobs.iter().any(|j| {
                j.model == INCOMING_MODEL && !j.terminal && j.state == SwapState::Admit
            }),
        "premise: the swap must still occupy the slot when the sweep ticks fire: {:?}",
        status.jobs
    );
    assert!(
        !status
            .jobs
            .iter()
            .any(|j| j.kind == JobKind::Unload && j.model == HOLD_MODEL),
        "the sweep may not even START an unload for a slot that carries a job: {:?}",
        status.jobs
    );
    assert!(
        !status
            .jobs
            .iter()
            .any(|j| j.kind == JobKind::Unload && j.outcome.as_deref() == Some("idle timeout")),
        "no idle-timeout unload may fire behind an in-flight job: {:?}",
        status.jobs
    );
    let swap = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the swap is on the ledger");
    assert!(
        !swap.terminal
            || swap.state == SwapState::Admit
            || swap.outcome.as_deref() == Some("cancelled"),
        "the sweep left the swap's own walk alone: {swap:?}"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "the swapping slot kept its engine"
    );
    assert_eq!(
        runtime.spawn_count(),
        1,
        "and nothing was spawned or respawned"
    );
}

#[tokio::test]
async fn the_idle_sweep_tick_fires_while_a_swapping_slot_is_spared() {
    // The positive half the guard pin above cannot give itself: its whole
    // window is also satisfied by a runtime in which the sweep never ticked once
    // (no fake clock, no tick counter in the status). This test runs the SAME
    // harness shape with one extra opt-in slot and makes the sweep show its work
    // inside the same window it must spare the swapping slot: an idle, occupied,
    // job-free AUX slot must land an `idle timeout` row - that row is the tick's
    // receipt - and only with that receipt can "no idle-timeout row for HOLD_MODEL"
    // be read as the guard acting rather than the guard idle. Red on an inverted
    // `slot.job` filter in `on_tick` (the sweep unloads the WORK slot too); red on
    // a sweep that never runs (the AUX receipt never lands).
    let mut cfg = AppConfig {
        release_timeout_s: 2,
        ..config(10)
    };
    cfg.category_mut(CategoryId::Work)
        .expect("default config has WORK")
        .idle_timeout_s = 1;
    let aux = cfg
        .category_mut(CategoryId::Aux)
        .expect("default config has AUX");
    aux.max_models = 1;
    aux.idle_timeout_s = 1;
    let witness = GatedWitness::new(2); // samples: the resident load and the AUX load
    let (handle, runtime) = harness5_witness(
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
            ("aux-idle", "mock", CategoryId::Aux, Some(4.0), ""),
        ],
        Arc::new(witness),
    );
    serve_hold_model(&handle).await;
    handle
        .swap("aux-idle", CategoryId::Aux)
        .await
        .expect("the AUX resident loads");
    submit_and_vanish(&handle, INCOMING_MODEL, CategoryId::Work).await;
    wait_until(&handle, 5, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.state == SwapState::Admit)
    })
    .await;

    // The same window as the guard pin: past two sweep ticks (IDLE_TICK is 2 s)
    // with both occupants aged well over the 1 s threshold. The AUX slot has no
    // job, so the sweep must fire there; the WORK slot carries the frozen swap,
    // so the guard must spare it.
    tokio::time::sleep(Duration::from_millis(4_500)).await;
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.kind == JobKind::Unload
                && j.model == "aux-idle"
                && j.outcome.as_deref() == Some("idle timeout")
        })
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    assert!(
        status
            .jobs
            .iter()
            .any(|j| j.kind == JobKind::Unload && j.model == "aux-idle"),
        "the tick's receipt: the idle AUX slot's unload row: {:?}",
        status.jobs
    );
    // With the tick proven inside this window, the negatives below are the guard
    // acting, not the sweep sleeping.
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "the swapping slot stayed spared while the sweep worked next door"
    );
    assert!(
        !status
            .jobs
            .iter()
            .any(|j| j.kind == JobKind::Unload && j.model == HOLD_MODEL),
        "and never started one for it: {:?}",
        status.jobs
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "only the spared WORK engine still answers"
    );
    assert_eq!(
        runtime.spawn_count(),
        2,
        "holder and aux-idle, nothing else"
    );
}
