use super::*;

// --- cost classes + launch floor (operator design note, 2026-10-26) -----------

fn small_card() -> AppConfig {
    AppConfig {
        vram_total_gb: 10.0,
        vram_reserve_gb: 2.0,
        load_timeout_s: 5,
        ..AppConfig::default()
    }
}

#[tokio::test]
async fn greedy_loads_book_the_room_they_grab_not_their_nominal_size() {
    // strata-style: nominal 72 GiB "does not fit" the 10 GiB card, but it
    // offloads to RAM/SSD — the size gate must not refuse, and the ledger
    // books what it will actually grab (room at admit), not 72.
    let (handle, runtime) = harness5(
        small_card(),
        &[
            ("work06", "mock", CategoryId::Work, Some(4.0), ""),
            ("big", "mock", CategoryId::Rp, Some(72.0), "cost: greedy"),
        ],
    );
    handle
        .swap("work06", CategoryId::Work)
        .await
        .expect("work loads");
    // free = 10 - 2 - 4 = 4; greedy default floor 4 -> floor met.
    handle
        .swap("big", CategoryId::Rp)
        .await
        .expect("greedy always fits");
    wait_until(&handle, 5, |s| {
        s.slots
            .iter()
            .any(|sl| sl.category == CategoryId::Rp && sl.model.as_deref() == Some("big"))
    })
    .await;
    let status = handle.status().await.expect("actor alive");
    let rp = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Rp)
        .expect("RP slot");
    assert_eq!(
        rp.need_gib,
        Some(4.0),
        "books the room, not the nominal size"
    );
    assert_eq!(runtime.server_count(), 2);
}

#[tokio::test]
async fn greedy_below_the_launch_floor_refuses_before_spawn() {
    // The guard the operator asked for: a greedy load must not spawn onto a
    // card already busy with other work — refused before engine start.
    let cfg = AppConfig {
        greedy_launch_min_free_gb: 6.0,
        ..small_card()
    };
    let (handle, runtime) = harness5(
        cfg,
        &[
            ("work06", "mock", CategoryId::Work, Some(4.0), ""),
            ("big", "mock", CategoryId::Rp, Some(72.0), "cost: greedy"),
        ],
    );
    handle
        .swap("work06", CategoryId::Work)
        .await
        .expect("work loads");
    // free = 10 - 2 - 4 = 4 < floor 6; WORK is not evictable -> refuse.
    let error = handle
        .swap("big", CategoryId::Rp)
        .await
        .expect_err("the floor refuses the greedy load");
    assert!(
        matches!(
            error,
            SchedulerError::Admission(simpleharn::budget::AdmissionError::BelowLaunchFloor { .. })
        ),
        "distinct typed error, got {error}"
    );
    assert!(error.to_string().contains("launch floor"), "{error}");
    assert_eq!(runtime.server_count(), 1, "refused BEFORE spawn, no thrash");
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.model == "big"
                && j.terminal
                && j.error.as_deref().unwrap_or("").contains("launch floor")
        })
    })
    .await;
}

#[tokio::test]
async fn floor_victims_release_in_full_not_until_the_size_fits() {
    // Review CRITICAL (2026-10-26): the floor is planned against plan_victims,
    // but the release-minimality re-check must not skip remaining floor victims
    // just because the (greedy, booked-small) need already fits after one kill.
    let mut cfg = AppConfig {
        vram_total_gb: 16.0,
        vram_reserve_gb: 2.0,
        load_timeout_s: 5,
        ..AppConfig::default()
    };
    if let Some(aux) = cfg
        .categories
        .iter_mut()
        .find(|cat| cat.id == CategoryId::Aux)
    {
        aux.max_models = 3; // two evictable victims must coexist
    }
    cfg.greedy_launch_min_free_gb = 6.0;
    let (handle, _runtime) = harness5(
        cfg,
        &[
            ("work8", "mock", CategoryId::Work, Some(8.0), ""),
            ("aux1", "mock", CategoryId::Aux, Some(3.0), ""),
            ("aux2", "mock", CategoryId::Aux, Some(3.0), ""),
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
    // must be released (WORK is protected). After only one kill the budget
    // available is 3 < 6: the re-check must continue, not spawn on a half-busy card.
    handle
        .swap("greedy72", CategoryId::Rp)
        .await
        .expect("greedy proceeds once both victims free the floor");
    wait_until(&handle, 8, |s| {
        s.slots
            .iter()
            .any(|sl| sl.category == CategoryId::Rp && sl.model.as_deref() == Some("greedy72"))
    })
    .await;
    let status = handle.status().await.expect("alive");
    let still_there = |cat: CategoryId, name: &str| {
        status
            .slots
            .iter()
            .any(|sl| sl.category == cat && sl.model.as_deref() == Some(name))
    };
    assert!(still_there(CategoryId::Work, "work8"), "WORK is protected");
    assert!(
        !still_there(CategoryId::Aux, "aux1") && !still_there(CategoryId::Aux, "aux2"),
        "BOTH floor victims must be released, not just the first: {status:?}"
    );
    let rp = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Rp)
        .expect("RP slot");
    assert_eq!(
        rp.need_gib,
        Some(6.0),
        "greedy books the room it grabbed (budget + freed victims), never 0"
    );
}

#[tokio::test]
async fn fixed_models_honour_an_explicit_launch_floor_below_their_size() {
    // A model that would fit is still refused when the floor is not met:
    // the floor is a launch guard, not a size calculation.
    let (handle, runtime) = harness5(
        small_card(),
        &[
            (
                "small",
                "mock",
                CategoryId::Work,
                Some(2.0),
                "launch_only_with_at_least_vram_free: 9",
            ),
            ("filler", "mock", CategoryId::Rp, Some(4.0), ""),
        ],
    );
    handle
        .swap("filler", CategoryId::Rp)
        .await
        .expect("filler loads");
    // free = 10 - 2 - 4 = 4 < floor 9 (RP's 4 GiB evictable but insufficient).
    let error = handle
        .swap("small", CategoryId::Work)
        .await
        .expect_err("floor refuses a model that would otherwise fit");
    assert!(error.to_string().contains("launch floor"), "{error}");
    assert_eq!(runtime.server_count(), 1);
}

#[tokio::test]
async fn greedy_with_unknown_size_books_the_room() {
    // Greedy + scheme sentinel: no size to declare, no estimate — still loads.
    let (handle, _runtime) = harness5(
        small_card(),
        &[(
            "ghost",
            "mock",
            CategoryId::Work,
            None,
            "cost: greedy\nmodel: scheme://strata/models/ghost",
        )],
    );
    handle
        .swap("ghost", CategoryId::Work)
        .await
        .expect("greedy needs no size");
    wait_until(&handle, 5, |s| {
        s.slots
            .iter()
            .any(|sl| sl.category == CategoryId::Work && sl.model.as_deref() == Some("ghost"))
    })
    .await;
    let status = handle.status().await.expect("alive");
    let work = status
        .slots
        .iter()
        .find(|s| s.category == CategoryId::Work)
        .expect("WORK slot");
    assert_eq!(work.need_gib, Some(8.0), "books the whole room (10-2)");
}

#[tokio::test]
async fn exact_refuses_without_a_declared_number() {
    // `exact` claims engine-computed precision — guessing is refused, not estimated.
    let (handle, _runtime) = harness5(
        small_card(),
        &[(
            "faith",
            "mock",
            CategoryId::Work,
            None,
            "cost: exact\nmodel: F:/x.gguf",
        )],
    );
    let error = handle
        .swap("faith", CategoryId::Work)
        .await
        .expect_err("exact without vram_gb is a refusal");
    assert!(error.to_string().contains("exact"), "{error}");
    assert!(
        matches!(
            error,
            SchedulerError::Admission(simpleharn::budget::AdmissionError::ExactUndeclared)
        ),
        "{error}"
    );
}

#[tokio::test]
async fn exact_with_a_number_gates_like_fixed() {
    let (handle, runtime) = harness5(
        small_card(),
        &[("nine", "mock", CategoryId::Work, Some(9.0), "cost: exact")],
    );
    // free = 10 - 2 = 8 < 9 -> Insufficient (exact numbers are binding).
    let error = handle
        .swap("nine", CategoryId::Work)
        .await
        .expect_err("exact numbers bind the gate");
    assert!(error.to_string().contains("insufficient VRAM"), "{error}");
    assert_eq!(runtime.server_count(), 0);
}
