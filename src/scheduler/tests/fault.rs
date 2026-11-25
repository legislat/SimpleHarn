use super::*;

fn job(id: u64, kind: JobKind, model: &str, error: Option<&str>, outcome: &str) -> JobStatus {
    JobStatus {
        id: JobId(id),
        kind,
        // The rung is only meaningful with a real admission; these
        // fixtures exercise the fault rule.
        path: None,
        model: model.to_string(),
        category: CategoryId::Work,
        state: SwapState::Serving,
        terminal: true,
        outcome: Some(outcome.to_string()),
        error: error.map(str::to_string),
        // Recomputed here independently of `fault_of`, so a broken rule cannot
        // quietly make its own fixture consistent.
        is_fault: kind == JobKind::Load && error.is_some() && outcome != "cancelled",
        need_gib: None,
        free_at_admit_gib: None,
        observed_at_admit_gib: None,
        gate_target_gib: None,
        victims_released: Vec::new(),
        warnings: Vec::new(),
        timings: Default::default(),
    }
}

#[test]
fn only_a_failed_load_is_a_fault() {
    // The row dot and the headline share one rule: a supersede or
    // unload-during-load must not pin a red row (duplication review, 2026-10-26).
    let cancelled = job(
        7,
        JobKind::Load,
        "m",
        Some("cancelled by unload"),
        "cancelled",
    );
    assert!(!cancelled.is_fault, "a cancel is policy, not a fault");
    assert!(headline_fault(&[cancelled]).is_none());
    let failed = job(8, JobKind::Load, "m", Some("settle timeout"), "failed");
    assert!(failed.is_fault);
    assert!(headline_fault(&[failed]).is_some());
    let unload = job(9, JobKind::Unload, "m", Some("spawn failed"), "failed");
    assert!(
        !unload.is_fault,
        "unload failures live in the jobs strip only"
    );
}

fn failed(id: u64, model: &str) -> JobStatus {
    job(
        id,
        JobKind::Load,
        model,
        Some("engine not ready"),
        "engine not ready",
    )
}

fn ok(id: u64, model: &str) -> JobStatus {
    job(id, JobKind::Load, model, None, "loaded")
}

#[test]
fn newest_failure_wins() {
    let jobs = vec![failed(1, "a"), failed(2, "b")];
    assert_eq!(headline_fault(&jobs).expect("fault").id, JobId(2));
}

#[test]
fn cancels_and_unloads_are_not_faults() {
    // Review findings 1/2: an operator cancel (T8 unload-during-load, supersede)
    // and a failed unload never take the headline away from a real load failure.
    let jobs = vec![
        failed(1, "a"),
        job(
            2,
            JobKind::Load,
            "b",
            Some("cancelled: superseded"),
            "cancelled",
        ),
        job(3, JobKind::Unload, "c", Some("kill refused"), "unloaded"),
    ];
    assert_eq!(headline_fault(&jobs).expect("real fault").id, JobId(1));
}

#[test]
fn only_a_later_same_model_success_clears() {
    assert!(headline_fault(&[failed(1, "a")]).is_some());
    assert!(
        headline_fault(&[failed(1, "a"), ok(2, "b")]).is_some(),
        "a different model's success does not mask the fault"
    );
    assert!(
        headline_fault(&[
            failed(1, "a"),
            job(2, JobKind::Unload, "a", None, "unloaded")
        ])
        .is_some(),
        "an unload never clears"
    );
    assert!(
        headline_fault(&[failed(1, "a"), ok(2, "a")]).is_none(),
        "the same model+category loading successfully clears"
    );
    assert_eq!(
        headline_fault(&[failed(1, "a"), ok(2, "a"), failed(3, "a")])
            .expect("re-faulted")
            .id,
        JobId(3)
    );
}

async fn wait_down(health: &SchedulerHealth) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !health.is_down() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the watcher marks the actor down");
}

#[tokio::test]
async fn the_actor_watcher_marks_health_down_on_panic_and_normal_exit() {
    let panicked = Arc::new(SchedulerHealth::default());
    let actor = tokio::spawn(async { panic!("test-only actor panic") });
    watch_actor(actor, Arc::clone(&panicked));
    wait_down(&panicked).await;
    assert_eq!(panicked.reason().as_deref(), Some("scheduler stopped"));

    let stopped = Arc::new(SchedulerHealth::default());
    let actor = tokio::spawn(async {});
    watch_actor(actor, Arc::clone(&stopped));
    wait_down(&stopped).await;
    assert_eq!(stopped.reason().as_deref(), Some("scheduler stopped"));
}
