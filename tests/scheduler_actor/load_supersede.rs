use super::*;

#[tokio::test]
async fn loads_model_into_empty_category_and_ledger() {
    let (handle, runtime) = harness(config(5), &[("qwen3", "mock", CategoryId::Work, Some(4.0))]);

    let reply = handle
        .swap("qwen3", CategoryId::Work)
        .await
        .expect("load succeeds");
    let Reply::Loaded(receipt) = reply else {
        panic!("expected Loaded, got {reply:?}")
    };
    assert_eq!(receipt.model, "qwen3");

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some("qwen3".to_string())]
    );
    let occupant_need = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Work)
        .unwrap()
        .need_gib;
    assert_eq!(occupant_need, Some(4.0));
    assert_eq!(runtime.server_count(), 1, "exactly one engine must be live");
}

#[tokio::test]
async fn same_model_same_category_is_already_and_idempotent() {
    let (handle, runtime) = harness(config(5), &[("qwen3", "mock", CategoryId::Work, Some(4.0))]);

    handle
        .swap("qwen3", CategoryId::Work)
        .await
        .expect("first load");
    let second = handle
        .swap("qwen3", CategoryId::Work)
        .await
        .expect("second is not an error");
    assert!(matches!(second, Reply::Already { .. }), "got {second:?}");
    assert_eq!(runtime.server_count(), 1, "no second engine may spawn");
}

#[tokio::test]
async fn concurrent_identical_loads_share_one_job_and_one_engine() {
    // Two loads for the same model and category must not race.
    let (handle, runtime) = harness(
        config(10),
        &[("qwen3", "mock-slow", CategoryId::Work, Some(4.0))],
    );

    let (a, b) = tokio::join!(
        handle.swap("qwen3", CategoryId::Work),
        handle.swap("qwen3", CategoryId::Work),
    );
    let receipt_of = |r: Result<Reply, SchedulerError>| match r.expect("both succeed") {
        Reply::Loaded(receipt) => receipt,
        other => panic!("expected Loaded, got {other:?}"),
    };
    let (ra, rb) = (receipt_of(a), receipt_of(b));
    assert_eq!(ra.job, rb.job, "both waiters must ride the same job");
    wait_until(&handle, 5, |s| {
        slot_model(s, CategoryId::Work) == vec![Some("qwen3".to_string())]
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        1,
        "exactly one engine despite two callers"
    );
}

#[tokio::test]
async fn different_model_replaces_occupant_and_kills_previous() {
    let (handle, runtime) = harness(
        config(5),
        &[
            ("model-a", "mock", CategoryId::Work, Some(4.0)),
            ("model-b", "mock", CategoryId::Work, Some(4.0)),
        ],
    );

    let Reply::Loaded(first) = handle
        .swap("model-a", CategoryId::Work)
        .await
        .expect("a loads")
    else {
        panic!("expected Loaded")
    };
    let Reply::Loaded(second) = handle
        .swap("model-b", CategoryId::Work)
        .await
        .expect("b loads")
    else {
        panic!("expected Loaded")
    };
    assert_ne!(first.port, second.port);

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some("model-b".to_string())]
    );
    assert!(
        runtime.is_dead(first.port).await,
        "the replaced engine must be dead (G12)"
    );
    assert_eq!(runtime.server_count(), 1, "no leaked engine after replace");
}

#[tokio::test]
async fn rapid_a_then_b_ends_with_exactly_b() {
    // A slow A is superseded by B; the end state is B serving, A's engine dead.
    let (handle, runtime) = harness(
        config(10),
        &[
            ("model-a", "mock-slow", CategoryId::Work, Some(4.0)),
            ("model-b", "mock", CategoryId::Work, Some(4.0)),
        ],
    );

    // Drop the waiter on purpose: the client aborts immediately after submitting A.
    submit_and_vanish(&handle, "model-a", CategoryId::Work).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let Reply::Loaded(receipt) = handle
        .swap("model-b", CategoryId::Work)
        .await
        .expect("b loads")
    else {
        panic!("expected Loaded")
    };
    assert_eq!(receipt.model, "model-b");

    wait_until(&handle, 5, |s| {
        s.jobs.iter().all(|j| j.terminal)
            && slot_model(s, CategoryId::Work) == vec![Some("model-b".to_string())]
    })
    .await;
    assert_eq!(runtime.server_count(), 1, "only B's engine may survive");
}

#[tokio::test]
async fn client_abort_mid_load_leaves_a_serving_slot_and_no_orphan() {
    // The waiter disappears; the load itself must complete.
    let (handle, runtime) = harness(
        config(10),
        &[("qwen3", "mock-slow", CategoryId::Work, Some(4.0))],
    );

    submit_and_vanish(&handle, "qwen3", CategoryId::Work).await;
    wait_until(&handle, 8, |s| {
        slot_model(s, CategoryId::Work) == vec![Some("qwen3".to_string())]
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        1,
        "the engine survives the aborted client"
    );
}

#[tokio::test]
async fn cancel_mid_load_kills_engine_and_empties_slot() {
    // T8: cancel is observed inside a step and completes cleanup.
    let (handle, runtime) = harness(
        config(10),
        &[("qwen3", "mock-slow", CategoryId::Work, Some(4.0))],
    );

    let job = spawn_and_find_job(&handle).await;
    handle.cancel(job).await.expect("cancel accepted");
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| j.id == job && j.terminal)
            && slot_model(s, CategoryId::Work).iter().all(Option::is_none)
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let job_status = status.jobs.iter().find(|j| j.id == job).unwrap();
    assert_eq!(job_status.state, SwapState::Idle);
    assert!(job_status
        .error
        .as_deref()
        .unwrap_or("")
        .contains("cancelled"));
    assert_eq!(
        runtime.server_count(),
        0,
        "cancelled load must not leak an engine"
    );
}

#[tokio::test]
async fn admission_refusal_carries_the_numbers_and_keeps_the_slot() {
    // 50 + 50 > 72 - 2: the second load must refuse with need/free in the message.
    let (handle, runtime) = harness(
        config(5),
        &[
            ("big-a", "mock", CategoryId::Work, Some(50.0)),
            ("big-b", "mock", CategoryId::Rp, Some(50.0)),
        ],
    );

    handle
        .swap("big-a", CategoryId::Work)
        .await
        .expect("a fits");
    let error = handle
        .swap("big-b", CategoryId::Rp)
        .await
        .expect_err("b must refuse");
    let message = error.to_string();
    assert!(message.contains("need 50.0 GiB"), "message was: {message}");
    assert!(message.contains("free 20.0 GiB"), "message was: {message}");

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Rp),
        vec![None],
        "refused load leaves the slot empty"
    );
    assert!(status
        .jobs
        .iter()
        .any(|j| j.state == SwapState::AdmitFail && j.terminal));
    assert_eq!(
        runtime.server_count(),
        1,
        "only the admitted engine is live"
    );
}

/// `min_turn` juggle guard: A serves RP under a one-hour knob;
/// B into WORK needs RP's bytes; the planner must refuse, name A and the knob
/// in the blockers tail, and keep A serving. Kills A-B-A-B ping-pong (measured
/// swaps run 14-45 s). Falsifier: delete the young-holder filter in
/// `plan_victims` and B evicts A — the expect_err below panics.
#[tokio::test]
async fn a_young_holder_refuses_to_be_evicted_and_names_the_reason() {
    let (handle, runtime) = harness5(
        config(5),
        &[
            (
                "hold-a",
                "mock",
                CategoryId::Rp,
                Some(50.0),
                "min_turn: 3600",
            ),
            ("want-b", "mock", CategoryId::Work, Some(30.0), ""),
        ],
    );

    handle
        .swap("hold-a", CategoryId::Rp)
        .await
        .expect("a loads into the empty RP slot");
    let error = handle
        .swap("want-b", CategoryId::Work)
        .await
        .expect_err("b must refuse while A's turn is inside min_turn");
    let message = error.to_string();
    assert!(
        message.contains("min_turn"),
        "the refusal must name the knob: {message}"
    );
    assert!(
        message.contains("hold-a"),
        "the refusal must name the row it refuses to evict: {message}"
    );
    assert!(
        message.contains("3600"),
        "the refusal must name the seconds: {message}"
    );

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Rp),
        vec![Some("hold-a".to_string())],
        "the guarded holder keeps serving"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "a refused swap kills nothing and spawns nothing"
    );
}

/// Boundary leg of the guard: with `min_turn: 1` the refusal is time-based, not
/// static — after the turn ages past the knob the same eviction admits.
/// Falsifier: make the guard age-blind (always block) and this swap refuses;
/// make it `served <= min_turn` at 1.5 s the eviction still refuses.
#[tokio::test]
async fn a_holder_whose_turn_passed_min_turn_is_evicted_again() {
    let (handle, runtime) = harness5(
        config(5),
        &[
            ("hold-a", "mock", CategoryId::Rp, Some(50.0), "min_turn: 1"),
            ("want-b", "mock", CategoryId::Work, Some(30.0), ""),
        ],
    );

    let Reply::Loaded(a) = handle
        .swap("hold-a", CategoryId::Rp)
        .await
        .expect("a loads")
    else {
        panic!("expected Loaded")
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    handle
        .swap("want-b", CategoryId::Work)
        .await
        .expect("the aged holder is evictable again");

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some("want-b".to_string())],
        "b must be serving"
    );
    assert_eq!(
        slot_model(&status, CategoryId::Rp),
        vec![None],
        "the evicted slot is empty"
    );
    assert!(
        runtime.is_dead(a.port).await,
        "the evicted engine must be dead (G12)"
    );
}

/// Non-goal pinned: the knob guards cross-slot eviction only. An explicit
/// request for a slot's own model is the user's word and stays open no matter
/// how young the holder is (the same-slot door in `start_queued_load`).
/// Falsifier: consult `min_turn` on the same-slot path and this swap refuses.
#[tokio::test]
async fn an_explicit_same_slot_request_still_replaces_a_young_holder() {
    let (handle, runtime) = harness5(
        config(5),
        &[
            (
                "hold-a",
                "mock",
                CategoryId::Work,
                Some(50.0),
                "min_turn: 3600",
            ),
            ("want-b", "mock", CategoryId::Work, Some(30.0), ""),
        ],
    );

    let Reply::Loaded(a) = handle
        .swap("hold-a", CategoryId::Work)
        .await
        .expect("a loads")
    else {
        panic!("expected Loaded")
    };
    handle
        .swap("want-b", CategoryId::Work)
        .await
        .expect("the same-slot door is never blocked by min_turn");

    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        slot_model(&status, CategoryId::Work),
        vec![Some("want-b".to_string())]
    );
    assert!(
        runtime.is_dead(a.port).await,
        "the replaced engine must be dead (G12)"
    );
}

#[tokio::test]
async fn under_specified_model_refuses_with_the_real_reason() {
    // The refusal must name under-specification, not blame a neighbor.
    let (handle, _runtime) = harness(config(5), &[("ghost", "mock", CategoryId::Work, None)]);

    let error = handle
        .swap("ghost", CategoryId::Work)
        .await
        .expect_err("must refuse");
    assert!(
        error.to_string().contains("under-specified"),
        "was: {error}"
    );
}
