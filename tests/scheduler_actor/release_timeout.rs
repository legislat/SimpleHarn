use super::*;

// --- the release-timeout arm: `Dispose` + `Released(Err)` ------------------------------

#[tokio::test]
async fn a_swap_whose_victim_never_proves_its_release_times_out_and_keeps_the_occupant() {
    // G12's answer to "an engine refused to die", reached the only way the product can
    // reach it: a swap whose victim's terminate never lands and nobody cancels it. The
    // four b6b33d1 tests all cancel first, so they only ever enter
    // `(CancelRequested, Released(Err))`; this arm — the one that must NOT claim the
    // engine dead, must name the holder, and must leave the slot loaded — had no test.
    let cfg = AppConfig {
        // The smallest deadline the validator accepts (a zero one fails its own release).
        release_timeout_s: 1,
        ..config(10)
    };
    let (handle, runtime) = hold_kill_harness(cfg);
    let victim = serve_hold_model(&handle).await;
    // Hold exactly the victim's kill: the release stays unprovable as a *state*, not as
    // a race against the deadline.
    runtime.hold_next_kills(1);

    let client = handle.clone();
    let swapping = tokio::spawn(async move { client.swap(INCOMING_MODEL, CategoryId::Work).await });
    wait_until(&handle, 10, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.state == SwapState::Dispose)
    })
    .await;
    assert_eq!(
        runtime.spawn_count(),
        1,
        "the gate is release confirmation, not a delay: no spawn may fire before it (F-6)"
    );

    let reply = tokio::time::timeout(Duration::from_secs(30), swapping)
        .await
        .expect("the release deadline must answer the waiter, never leave it hanging")
        .expect("the swap task must not panic");
    let error = match reply {
        Ok(other) => panic!("an unprovable release must refuse the swap, got {other:?}"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, SchedulerError::ReleaseTimeout { holder } if holder == HOLD_MODEL),
        "the refusal is the typed release timeout naming the holder: {error}"
    );
    assert_eq!(
        error.to_string(),
        format!("release timeout: {HOLD_MODEL} still holds the slot"),
        "the operator-facing text of the G12 timeout"
    );

    let status = handle.status().await.expect("actor alive");
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == INCOMING_MODEL)
        .expect("the refused swap is in the ledger");
    assert!(job.terminal, "{job:?}");
    assert_eq!(
        job.kind,
        JobKind::Load,
        "a refused load is a fault row, not a silent unload: {job:?}"
    );
    assert_eq!(
        job.state,
        SwapState::Idle,
        "the exit funnel walks Failed -> Reap -> Idle: {job:?}"
    );
    assert_eq!(
        job.outcome.as_deref(),
        Some("kill failed for hold-a: release not confirmed within release_timeout_s"),
        "the row carries the reaper's own reason for giving up: {job:?}"
    );
    assert!(
        job.error
            .as_deref()
            .unwrap_or("")
            .contains("release timeout: hold-a still holds the slot"),
        "{job:?}"
    );
    assert!(
        job.timings.release_ms.is_none(),
        "the release stage never completed: {:?}",
        job.timings
    );
    assert!(
        job.is_fault,
        "an engine that refused to die is a fault: {job:?}"
    );

    // The b6b33d1 rule: an unconfirmed release may not claim a death.
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "the victim keeps its occupant"
    );
    // Live card-run proof (c), confirmed-release posture at the ledger half: nobody
    // proved the victim
    // dead, so its own load row keeps the honest Serving/loaded truth — the row
    // moves only at a confirmed-release commit (`commit_victim_release` in
    // `src/scheduler/occupant.rs`), and a timeout is not one. Falsifier (run in
    // place, revert): terminalize the victim row on the `Released(Err)` path
    // too, and this row reads unloaded while its engine still answers its port
    // two asserts below.
    let resident = status
        .jobs
        .iter()
        .find(|j| j.model == HOLD_MODEL && j.kind == JobKind::Load)
        .expect("the resident's own load row is on the ledger");
    assert!(
        resident.terminal
            && resident.state == SwapState::Serving
            && resident.outcome.as_deref() == Some("loaded"),
        "a timed-out release leaves the occupant AND its job serving: {resident:?}"
    );
    let kept = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Work)
        .expect("WORK slot");
    assert_eq!(
        kept.need_gib,
        Some(4.0),
        "and it is still debited for what it really holds (the 19[1] ledger half)"
    );
    let debited: f64 = status.slots.iter().filter_map(|s| s.need_gib).sum();
    assert_eq!(
        debited, 4.0,
        "one real occupant is debited exactly once: {:?}",
        status.slots
    );
    assert_eq!(
        booking_gibs(&status.jobs),
        0.0,
        "the terminal refused load must not book its own 4.0 GiB on top"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "the engine nobody proved dead is still live"
    );
    assert!(
        !runtime.is_dead(victim.port).await,
        "and it still answers its port"
    );
    assert_eq!(
        runtime.spawn_count(),
        1,
        "the refused swap never spawned its replacement"
    );

    // Recovery: the held terminate, once freed, must complete the reap the timeout
    // abandoned — no stranded engine, no slot that reads loaded with nothing behind it.
    runtime.release_held_kills();
    assert!(
        matches!(
            handle
                .unload(CategoryId::Work)
                .await
                .expect("unload accepted"),
            Reply::Unloaded {
                unloading: 1,
                jobs_cancelled: 0
            }
        ),
        "the slot the timeout kept is unloadable"
    );
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
        .expect("the follow-up unload ran and is on the ledger");
    assert_eq!(unload.outcome.as_deref(), Some("unloaded"), "{unload:?}");
    assert!(
        unload.timings.release_ms.is_some(),
        "this time the release was proven: {:?}",
        unload.timings
    );
    // Live card-run proof, operator-path wording at the commit: the unload that finally
    // proved the release also terminalizes the kept LOAD row — the ledger no
    // longer claims a dead engine serves, and it names the operator release
    // `unloaded`, never the eviction word `evicted` (`victim.evicted` is the
    // one discriminator inside `terminalize_victim_load_row`). Falsifier: hard-
    // code `evicted` at the commit site and this row accuses the operator's own
    // unload of an eviction.
    let resident = status
        .jobs
        .iter()
        .find(|j| j.model == HOLD_MODEL && j.kind == JobKind::Load)
        .expect("the resident's load row stays on the ledger");
    assert!(resident.terminal, "the row stays terminal: {resident:?}");
    assert_eq!(
        resident.state,
        SwapState::Idle,
        "the proven release terminalized the victim's load row: {resident:?}"
    );
    assert_eq!(
        resident.outcome.as_deref(),
        Some("unloaded"),
        "the operator release names itself honestly: {resident:?}"
    );
    assert!(
        !resident.is_fault,
        "a proven release is not a fault: {resident:?}"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "freeing the held kill must leave no stranded engine"
    );
    assert!(
        runtime.is_dead(victim.port).await,
        "and no engine answering on the freed slot's port"
    );
    assert_eq!(
        booking_gibs(&status.jobs),
        0.0,
        "and no debit left for a slot that is now empty"
    );
    assert_eq!(runtime.spawn_count(), 1, "recovery spawns nothing either");
}

// --- the `releasing` slot state (operator ruling) -------------------------------------

#[tokio::test]
async fn a_parked_unload_shows_releasing_on_its_slot_until_the_release_is_confirmed() {
    // The missing label the ruling names: the slot does NOT null at accept
    // (`unload_replies_acceptance_before_any_release_evidence` keeps that pin),
    // and between accept and driver-confirmed release `/har/status` must say
    // `releasing` — not `serving` (a lie about intent) and not `empty` (a lie
    // about the card) — and `empty` only after the release is proven.
    let (handle, runtime) = hold_kill_harness(config(10));
    let occupant = serve_hold_model(&handle).await;
    assert_eq!(
        slot_state(
            &handle.status().await.expect("actor alive"),
            CategoryId::Work
        ),
        vec!["serving".to_string()],
        "a quiet serving slot reads serving"
    );

    assert!(matches!(
        handle
            .unload_model(HOLD_MODEL)
            .await
            .expect("unload accepted"),
        Reply::Unloaded {
            unloading: 1,
            jobs_cancelled: 0
        }
    ));
    wait_until(&handle, 5, |s| {
        s.slots
            .iter()
            .any(|sl| sl.category == CategoryId::Work && sl.state == "releasing")
            && s.jobs
                .iter()
                .any(|j| j.kind == JobKind::Unload && j.model == HOLD_MODEL && !j.terminal)
    })
    .await;
    let parked = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_state(&parked, CategoryId::Work),
        vec!["releasing".to_string()],
        "the parked-release window reads releasing on the slot"
    );
    let unload = parked
        .jobs
        .iter()
        .find(|j| j.kind == JobKind::Unload && j.model == HOLD_MODEL)
        .expect("the unload is on the ledger");
    assert!(
        !unload.terminal,
        "the label names a release the driver has not confirmed: {unload:?}"
    );
    assert_eq!(
        slot_model(&parked, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "releasing keeps the occupant visible while it is installed"
    );

    runtime.release_held_kills();
    wait_until(&handle, 8, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_state(s, CategoryId::Work) == vec!["empty".to_string()]
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        0,
        "the confirmed release left no engine behind"
    );
    assert!(
        runtime.is_dead(occupant.port).await,
        "and no engine answering on the freed slot's port"
    );
}

#[tokio::test]
async fn an_eviction_victim_reads_releasing_while_its_loading_job_is_still_live() {
    // The cross-slot half of the same predicate: a victim's slot carries no
    // job of its own — the commitment lives on the LOADING job's victim set.
    // Two victims pin both halves of the predicate the `Already` denial uses:
    // the dequeued `victim` (its kill parked in `Dispose`) and the still
    // queued `victims` entry. Falsifier: drop the `victims` scan from
    // `is_committed_release_target` and the queued victim keeps reading
    // `serving` while the harness has already committed to evict it; derive
    // the label from `slot.job` instead and neither victim slot ever reads
    // `releasing`.
    let mut cfg = AppConfig {
        vram_total_gb: 16.0,
        vram_reserve_gb: 2.0,
        ..config(10)
    };
    if let Some(aux) = cfg
        .categories
        .iter_mut()
        .find(|cat| cat.id == CategoryId::Aux)
    {
        aux.max_models = 3; // two evictable victims must coexist
    }
    cfg.greedy_launch_min_free_gb = 6.0;
    let (handle, runtime) = harness5(
        cfg,
        &[
            ("work8", "mock", CategoryId::Work, Some(8.0), ""),
            ("aux1", "mock-hold-kill", CategoryId::Aux, Some(3.0), ""),
            ("aux2", "mock-hold-kill", CategoryId::Aux, Some(3.0), ""),
            (
                "greedy72",
                "mock",
                CategoryId::Rp,
                Some(72.0),
                "cost: greedy",
            ),
        ],
    );
    handle
        .swap("work8", CategoryId::Work)
        .await
        .expect("work loads");
    handle
        .swap("aux1", CategoryId::Aux)
        .await
        .expect("aux1 loads");
    handle
        .swap("aux2", CategoryId::Aux)
        .await
        .expect("aux2 loads");
    // Capacity 14 fully booked: free = 0, floor 6 -> BOTH 3 GiB AUX victims
    // are committed (WORK is protected). The first victim's kill parks, so the
    // second one is still only queued when the window is observed.
    let client = handle.clone();
    let swapping = tokio::spawn(async move { client.swap("greedy72", CategoryId::Rp).await });
    wait_until(&handle, 5, |s| {
        let committed = |name: &str| {
            s.slots
                .iter()
                .any(|sl| sl.model.as_deref() == Some(name) && sl.state == "releasing")
        };
        committed("aux1") && committed("aux2")
    })
    .await;
    let parked = handle.status().await.expect("actor alive");
    let slot_of = |name: &str| {
        parked
            .slots
            .iter()
            .find(|sl| sl.model.as_deref() == Some(name))
            .expect("the victim keeps its slot table row")
    };
    assert_eq!(slot_of("aux1").state, "releasing", "the dequeued victim");
    assert_eq!(slot_of("aux2").state, "releasing", "the queued victim");
    assert_eq!(
        slot_of("aux1").model.as_deref(),
        Some("aux1"),
        "a committed release keeps the occupant visible"
    );
    let rp_slot = parked
        .slots
        .iter()
        .find(|sl| sl.category == CategoryId::Rp)
        .expect("RP slot");
    assert_eq!(
        rp_slot.state, "loading",
        "the loading slot reads loading while its victims release: {rp_slot:?}"
    );
    assert!(
        parked
            .jobs
            .iter()
            .any(|j| j.model == "greedy72" && !j.terminal),
        "the eviction commitment is a live job: {:?}",
        parked.jobs
    );

    runtime.release_held_kills();
    let reply = tokio::time::timeout(Duration::from_secs(20), swapping)
        .await
        .expect("the swap completes once both victims' releases confirm")
        .expect("the swap task must not panic");
    reply.expect("the load succeeds after the eviction");
    wait_until(&handle, 8, |s| {
        s.slots.iter().any(|sl| {
            sl.category == CategoryId::Rp
                && sl.model.as_deref() == Some("greedy72")
                && sl.state == "serving"
        }) && s
            .slots
            .iter()
            .all(|sl| sl.category != CategoryId::Aux || sl.state == "empty")
    })
    .await;
}

#[tokio::test]
async fn a_release_timeout_slot_reads_serving_again_and_never_stuck_releasing() {
    // G12 keeps the occupant installed when a release cannot be proven, so the
    // terminal job is not a commitment: after the deadline the slot must flip
    // back to `serving`. Falsifier: drop the `!job.terminal` half of
    // `is_committed_release_target` and the label stays `releasing` forever
    // over an engine the harness has given up on.
    let cfg = AppConfig {
        release_timeout_s: 2,
        ..config(10)
    };
    let (handle, runtime) = hold_kill_harness(cfg);
    serve_hold_model(&handle).await;
    runtime.hold_next_kills(1);

    let client = handle.clone();
    let swapping = tokio::spawn(async move { client.swap(INCOMING_MODEL, CategoryId::Work).await });
    wait_until(&handle, 10, |s| {
        s.jobs
            .iter()
            .any(|j| j.model == INCOMING_MODEL && j.state == SwapState::Dispose)
    })
    .await;
    let parked = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_state(&parked, CategoryId::Work),
        vec!["releasing".to_string()],
        "a swap's committed victim reads releasing while its release is in flight"
    );

    let reply = tokio::time::timeout(Duration::from_secs(30), swapping)
        .await
        .expect("the release deadline must answer the waiter, never leave it hanging")
        .expect("the swap task must not panic");
    assert!(
        matches!(reply, Err(SchedulerError::ReleaseTimeout { .. })),
        "the unprovable release refuses the swap: {reply:?}"
    );
    wait_until(&handle, 10, |s| s.jobs.iter().all(|j| j.terminal)).await;
    let after = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&after, CategoryId::Work),
        vec![Some(HOLD_MODEL.to_string())],
        "the victim keeps its occupant (G12)"
    );
    assert_eq!(
        slot_state(&after, CategoryId::Work),
        vec!["serving".to_string()],
        "a terminal job is not a commitment: the slot must not stay stuck releasing"
    );

    // Leave no parked engine: free the held kill and unload for real.
    runtime.release_held_kills();
    handle
        .unload(CategoryId::Work)
        .await
        .expect("the kept slot is unloadable");
    wait_until(&handle, 10, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_state(s, CategoryId::Work) == vec!["empty".to_string()]
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        0,
        "no engine survives the freed reap"
    );
}
