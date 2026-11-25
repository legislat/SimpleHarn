use super::*;

#[tokio::test]
async fn unload_frees_slot_ledger_and_engine() {
    let (handle, runtime) = harness(config(5), &[("qwen3", "mock", CategoryId::Work, Some(4.0))]);

    handle.swap("qwen3", CategoryId::Work).await.expect("loads");
    handle
        .unload(CategoryId::Work)
        .await
        .expect("unload accepted");
    wait_until(&handle, 5, |s| {
        slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    assert_eq!(runtime.server_count(), 0, "unload must kill the engine");
    // Idempotent: unloading an empty category moved nothing, and the reply says so —
    // an idempotent unload used to answer an indistinguishable `Done`.
    assert!(matches!(
        handle.unload(CategoryId::Work).await.expect("idempotent"),
        Reply::Unloaded {
            unloading: 0,
            jobs_cancelled: 0
        }
    ));
}

#[tokio::test]
async fn unload_model_during_load_cancels_and_kills_engine() {
    // Unloading a still-loading model cancels its swap and the
    // cancellation still runs kill + release confirmation.
    let (handle, runtime) = harness(
        config(10),
        &[("qwen3", "mock-slow", CategoryId::Work, Some(4.0))],
    );

    let job = spawn_and_find_job(&handle).await;
    assert!(
        matches!(
            handle.unload_model("qwen3").await.expect("accepted"),
            Reply::Unloaded {
                unloading: 0,
                jobs_cancelled: 1
            }
        ),
        "a loading slot had no occupant: the reply names the cancelled job, not an unload"
    );
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| j.id == job && j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let job_status = status.jobs.iter().find(|j| j.id == job).unwrap();
    assert!(job_status
        .error
        .as_deref()
        .unwrap_or("")
        .contains("unloaded by request"));
    assert_eq!(
        runtime.server_count(),
        0,
        "cancelled swap must not leak an engine"
    );
    // Unloading a model that is not loaded moves nothing; the reply says so.
    assert!(matches!(
        handle.unload_model("ghost").await.expect("idempotent"),
        Reply::Unloaded {
            unloading: 0,
            jobs_cancelled: 0
        }
    ));
}

// --- cancel while a victim release is in flight -------------------------------------

/// Drives the same-slot swap of [`INCOMING_MODEL`] over [`HOLD_MODEL`] until the job
/// sits in `Dispose` with the victim's kill unconfirmed, and returns the swap's job id.
async fn swap_parked_in_dispose_over_a_victim(handle: &SchedulerHandle) -> JobId {
    submit_and_vanish(handle, INCOMING_MODEL, CategoryId::Work).await;
    wait_until(handle, 5, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.state == SwapState::Dispose)
    })
    .await;
    handle
        .status()
        .await
        .expect("actor alive")
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .map(|j| j.id)
        .expect("invariant: wait_until established the swap job")
}

#[tokio::test]
async fn cancel_of_a_swap_mid_victim_release_kills_the_victim_and_empties_its_slot() {
    // Unload during a hung swap is the operator's commonest recovery
    // gesture. Cancelling there must still kill the victim and confirm it: a job that
    // terminalizes over an unconfirmed release leaves a dead-but-installed occupant,
    // which debits the ledger forever and blocks reload with 200 `already`.
    let (handle, runtime) = hold_kill_harness(config(10));
    let victim = serve_hold_model(&handle).await;
    assert_eq!(runtime.server_count(), 1, "the victim engine is live");
    swap_parked_in_dispose_over_a_victim(&handle).await;

    assert!(matches!(
        handle
            .unload(CategoryId::Work)
            .await
            .expect("cancel accepted"),
        Reply::Unloaded {
            unloading: 1,
            jobs_cancelled: 1
        }
    ));
    assert_eq!(
        runtime.server_count(),
        1,
        "nothing may die behind the operator's back before the cancel's own reap runs"
    );
    runtime.release_held_kills();

    wait_until(&handle, 8, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the cancelled swap is in the ledger");
    assert_eq!(job.outcome.as_deref(), Some("cancelled"), "{job:?}");
    assert!(
        job.error
            .as_deref()
            .unwrap_or("")
            .contains("unloaded by request"),
        "{job:?}"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "the victim engine must not survive the cancelled swap"
    );
    assert!(
        runtime.is_dead(victim.port).await,
        "the victim's port must stop answering (G12)"
    );
    assert_eq!(
        runtime.spawn_count(),
        1,
        "a swap cancelled in Dispose must never spawn its replacement"
    );
}

#[tokio::test]
async fn a_repeat_unload_of_a_stuck_swap_reports_nothing_new() {
    // With the first unload's cancel still stuck in
    // `CancelRequested` (the victim's kill held), a second `unload_model` moves
    // nothing — the unload commitment is already armed behind the same job and
    // the job is already cancelled — yet pre-fix it answered like a command
    // that had armed a second release and cancelled a second job.
    let (handle, runtime) = hold_kill_harness(config(10));
    let victim = serve_hold_model(&handle).await;
    assert_eq!(runtime.server_count(), 1, "the victim engine is live");
    swap_parked_in_dispose_over_a_victim(&handle).await;

    assert!(matches!(
        handle
            .unload_model(HOLD_MODEL)
            .await
            .expect("first unload accepted"),
        Reply::Unloaded {
            unloading: 1,
            jobs_cancelled: 1
        }
    ));
    // The stuck state the finding names: the cancel has landed, the job is
    // non-terminal, and the slot is armed to unload behind it.
    wait_until(&handle, 5, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.state == SwapState::CancelRequested)
    })
    .await;

    assert!(
        matches!(
            handle
                .unload_model(HOLD_MODEL)
                .await
                .expect("repeat unload accepted"),
            Reply::Unloaded {
                unloading: 0,
                jobs_cancelled: 0
            }
        ),
        "the repeat command armed and cancelled nothing new and must not say otherwise"
    );

    runtime.release_held_kills();
    wait_until(&handle, 8, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        0,
        "the reported-nothing repeat must still leave the held kill landed"
    );
    assert!(
        runtime.is_dead(victim.port).await,
        "and the victim's port refused, not just the count"
    );
}

#[tokio::test]
async fn second_unload_of_a_slow_unload_kills_the_victim_and_empties_its_slot() {
    // The mirror case of the unproven-release rule: `begin_unload_slot` sets `victim`, never
    // `spawned`, so the second /har/unload of a slow unload cancels the first while
    // its kill is in flight — and must still finish the job it cancels.
    let (handle, runtime) = hold_kill_harness(config(10));
    let victim = serve_hold_model(&handle).await;

    handle
        .unload(CategoryId::Work)
        .await
        .expect("first unload accepted");
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.model == HOLD_MODEL
                && j.kind == simpleharn::scheduler::JobKind::Unload
                && j.state == SwapState::Dispose
        })
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        1,
        "the engine is still live when the second unload lands"
    );

    // The command disarms an in-flight UNLOAD job, which is a
    // change of `unloading` commitment (the release is re-armed behind the
    // cancel), not a cancellation of a load — `cancelled` per README names only
    // load jobs. Pre-fix this answered `jobs_cancelled: 1` for an Unload row.
    assert!(
        matches!(
            handle
                .unload(CategoryId::Work)
                .await
                .expect("second unload accepted"),
            Reply::Unloaded {
                unloading: 1,
                jobs_cancelled: 0
            }
        ),
        "the repeat unload may claim the rearmed release, never a cancelled load"
    );
    runtime.release_held_kills();

    wait_until(&handle, 8, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == HOLD_MODEL && j.kind == simpleharn::scheduler::JobKind::Unload)
        .expect("the cancelled unload is in the ledger");
    assert_eq!(job.outcome.as_deref(), Some("cancelled"), "{job:?}");
    assert_eq!(
        runtime.server_count(),
        0,
        "a cancelled unload must leave no live engine"
    );
    assert!(runtime.is_dead(victim.port).await, "and no answering port");
}

#[tokio::test]
async fn cancel_with_an_unconfirmed_release_keeps_the_occupant_and_says_cancelled() {
    // The half the fix must NOT touch: a release that timed out was never confirmed,
    // so the harness may not claim the engine is gone. The slot keeps its occupant (and
    // its ledger debit) and the row stays a plain cancel.
    let cfg = AppConfig {
        release_timeout_s: 2,
        ..config(10)
    };
    let (handle, runtime) = hold_kill_harness(cfg);
    let victim = serve_hold_model(&handle).await;
    swap_parked_in_dispose_over_a_victim(&handle).await;

    handle
        .unload(CategoryId::Work)
        .await
        .expect("cancel accepted");
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;

    let status = handle.status().await.expect("actor alive");
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the cancelled swap is in the ledger");
    assert_eq!(job.outcome.as_deref(), Some("cancelled"), "{job:?}");
    assert!(
        job.error
            .as_deref()
            .unwrap_or("")
            .contains("unloaded by request"),
        "{job:?}"
    );
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "an unconfirmed release may not clear the slot of an engine nobody proved dead"
    );
    assert_eq!(runtime.server_count(), 1, "the engine was never killed");
    assert!(
        !runtime.is_dead(victim.port).await,
        "and it is still answering on its port"
    );
}

#[tokio::test]
async fn a_second_cancel_of_the_same_swap_still_reaps_the_victim() {
    // The reap decision must not key off the state the cancel itself writes: pressing
    // Unload twice (or Unload then loading a third model) cancels a job that is already
    // `CancelRequested` with its re-issued kill still parked.
    let (handle, runtime) = hold_kill_harness(config(10));
    let victim = serve_hold_model(&handle).await;
    let job = swap_parked_in_dispose_over_a_victim(&handle).await;

    handle
        .unload(CategoryId::Work)
        .await
        .expect("first cancel accepted");
    wait_until(&handle, 5, |s| {
        s.jobs
            .iter()
            .any(|j| j.id == job && j.state == SwapState::CancelRequested)
    })
    .await;

    assert!(matches!(
        handle.cancel(job).await.expect("second cancel accepted"),
        Reply::Done
    ));
    runtime.release_held_kills();

    wait_until(&handle, 8, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        0,
        "the second cancel must not abandon the victim's reap"
    );
    assert!(runtime.is_dead(victim.port).await, "and no answering port");
}

#[tokio::test]
async fn unload_all_clears_every_category_and_cancels_inflight() {
    // The tray's Unload All: every occupied slot releases, every
    // in-flight job cancels, nothing leaks.
    let (handle, runtime) = harness(
        config(10),
        &[
            ("work06", "mock", CategoryId::Work, Some(4.0)),
            ("rp06", "mock-slow", CategoryId::Rp, Some(4.0)),
        ],
    );

    handle
        .swap("work06", CategoryId::Work)
        .await
        .expect("loads");
    submit_and_vanish(&handle, "rp06", CategoryId::Rp).await;
    wait_until(&handle, 5, |s| {
        s.slots
            .iter()
            .any(|slot| slot.category == CategoryId::Rp && slot.loading_job.is_some())
    })
    .await;

    assert!(matches!(
        handle.unload_all().await.expect("accepted"),
        Reply::Unloaded {
            unloading: 1,
            jobs_cancelled: 1
        }
    ));
    wait_until(&handle, 8, |s| {
        s.slots.iter().all(|slot| slot.model.is_none()) && s.jobs.iter().all(|j| j.terminal)
    })
    .await;
    assert_eq!(runtime.server_count(), 0, "unload-all kills every engine");
    assert!(matches!(
        handle.unload_all().await.expect("idempotent"),
        Reply::Unloaded {
            unloading: 0,
            jobs_cancelled: 0
        }
    ));
}

#[tokio::test]
async fn control_plane_answers_while_a_slow_load_runs() {
    // A1: v3.3 held a global lock through wait_ready; SE must not.
    let (handle, _runtime) = harness(
        config(10),
        &[
            ("slow", "mock-slow", CategoryId::Work, Some(4.0)),
            ("quick", "mock", CategoryId::Rp, Some(4.0)),
        ],
    );

    submit_and_vanish(&handle, "slow", CategoryId::Work).await;
    tokio::time::sleep(Duration::from_millis(150)).await; // let the load get going

    let started = Instant::now();
    let status = handle
        .status()
        .await
        .expect("status must answer during a load");
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "status queued behind the load"
    );
    assert!(
        status.slots.iter().any(|s| s.loading_job.is_some()),
        "the load must be visible as a job"
    );

    let started = Instant::now();
    handle
        .swap("quick", CategoryId::Rp)
        .await
        .expect("other categories load freely");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "RP queued behind WORK's load"
    );
    handle
        .unload(CategoryId::Work)
        .await
        .expect("unload must not queue behind the load");
}

#[tokio::test]
async fn engine_that_never_becomes_ready_fails_within_the_deadline() {
    let (handle, runtime) = harness(
        config(1),
        &[("broken", "mock-never", CategoryId::Work, Some(4.0))],
    );

    let error = handle
        .swap("broken", CategoryId::Work)
        .await
        .expect_err("must fail");
    assert!(error.to_string().contains("ready"), "was: {error}");
    assert_eq!(
        runtime.server_count(),
        0,
        "failed load must reap its engine"
    );
    let status = handle.status().await.expect("actor alive");
    assert!(
        slot_model(&status, CategoryId::Work)
            .iter()
            .all(Option::is_none),
        "failed load must leave the slot empty, got {:?}",
        slot_model(&status, CategoryId::Work)
    );
    assert!(terminal_jobs(&status).iter().any(|j| j.error.is_some()));
    // The double-load funnel is runtime-enforced only by a `debug_assert`
    // compiled out of the release binary, so the failure tests must pin the STATES,
    // not just strings. A failed load terminalizes by walking Failed -> Reap -> Idle;
    // a regression that terminalizes without completing the walk leaves the row in
    // Failed/Reap/Dispose and `/har/jobs` shows the operator a state that never
    // happened — that row's state is `Idle` here, exactly once, only if every edge
    // was stepped.
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == "broken")
        .expect("the failed load is in the ledger");
    assert!(job.terminal, "{job:?}");
    assert_eq!(
        job.state,
        SwapState::Idle,
        "the funnel walked to Idle: {job:?}"
    );
}

#[tokio::test]
async fn a_failed_spawn_parks_in_failed_then_walks_the_funnel_to_idle() {
    // `SwapState::Failed` was asserted by zero tests, so a
    // regression that terminalized a failure without walking Failed -> Reap -> Idle
    // (or skipped Quiesce) was green suite-wide while `/har/jobs` — the surface the
    // ledger-honesty rule exists to make honest — showed a state that never
    // happened. The runtime enforces
    // the walk only with a `debug_assert!` compiled out of the release binary.
    // `mock-hold-never` makes the observation *deterministic*, not a race: the engine
    // spawns and never becomes ready, so `fail_job` parks the job in `Failed` and its
    // own reap kill parks (gate) — the job sits in `Failed` as a state the test reads
    // — until the test releases the kill, at which point the funnel completes to Idle.
    let (handle, runtime) = harness(
        config(1),
        &[("dying", "mock-hold-never", CategoryId::Work, Some(4.0))],
    );

    let client = handle.clone();
    let swapping = tokio::spawn(async move { client.swap("dying", CategoryId::Work).await });

    // The failed spawn parks in `Failed` with its reap unconfirmed: observe the state.
    wait_until(&handle, 8, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == "dying" && j.state == SwapState::Failed)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == "dying")
        .expect("the failed spawn is in the ledger");
    assert_eq!(job.state, SwapState::Failed, "cleanup pending: {job:?}");
    assert!(!job.terminal, "the park in Failed is not terminal: {job:?}");
    assert!(
        job.error.as_deref().unwrap_or("").contains("ready"),
        "the failure names readiness: {job:?}"
    );

    // Releasing the parked reap lets the funnel complete; the row must land on Idle.
    runtime.release_held_kills();
    let answer = tokio::time::timeout(Duration::from_secs(15), swapping)
        .await
        .expect("the funnel must answer the waiter")
        .expect("the swap task must not panic");
    answer.expect_err("a never-ready engine refuses the swap");
    let status = handle.status().await.expect("actor alive");
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == "dying")
        .expect("the failed spawn keeps its row");
    assert!(job.terminal, "{job:?}");
    assert_eq!(
        job.state,
        SwapState::Idle,
        "Failed -> Reap -> Idle completes the funnel: {job:?}"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "the funnel reaped the engine it spawned"
    );
}

#[tokio::test]
async fn failed_swap_that_released_its_victim_still_reaps_its_own_engine() {
    // The kill target used to be `job.victim` when one existed, and `job.victim` is never
    // cleared once queued — so a swap that evicted its victim, spawned, and then failed
    // readiness re-killed a corpse and left the engine it had started holding the card and
    // its port while the slot read empty.
    let (handle, runtime) = harness5(
        config(1),
        &[
            ("aux30", "mock", CategoryId::Aux, Some(30.0), ""),
            ("work60", "mock-never", CategoryId::Work, Some(60.0), ""),
        ],
    );
    handle
        .swap("aux30", CategoryId::Aux)
        .await
        .expect("aux30 loads");
    assert_eq!(runtime.server_count(), 1, "the victim engine is live");

    let error = handle
        .swap("work60", CategoryId::Work)
        .await
        .expect_err("the never-ready engine must fail its load");
    assert!(
        error.to_string().contains("ready"),
        "must fail on readiness, not on admission: {error}"
    );

    let status = handle.status().await.expect("actor alive");
    let swap_job = terminal_jobs(&status)
        .into_iter()
        .find(|job| job.model == "work60")
        .expect("the swap is in the ledger");
    assert_eq!(
        swap_job.state,
        SwapState::Idle,
        "the failure walked Failed -> Reap -> Idle before terminalizing: {swap_job:?}"
    );
    assert_eq!(
        swap_job.victims_released,
        vec!["aux30".to_string()],
        "the eviction must really have happened, or this test proves nothing: {swap_job:?}"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "both the evicted victim and the failed engine must be reaped"
    );
    assert!(
        slot_model(&status, CategoryId::Aux)
            .iter()
            .all(Option::is_none),
        "the evicted slot stays empty"
    );
    assert!(
        slot_model(&status, CategoryId::Work)
            .iter()
            .all(Option::is_none),
        "a failed load must not leave an occupant"
    );
}
