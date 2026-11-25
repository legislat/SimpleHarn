use super::*;

// --- chaos legs (`contrib/chaos` methodology) ----------------------------------------
//
// Berth's `contrib/chaos` injects a fault at a seam and asserts the orchestrator
// names it and leaves no orphan. Two legs, cardless, on the fake engine — never a
// second `verify.ps1`. Each pins a different release failure and each fails without
// the behavior it pins (falsifier named in its comment).

#[tokio::test]
async fn chaos_engine_killed_mid_release_is_confirmed_and_leaves_no_orphan() {
    // Leg A. The harness parks a release (the victim's terminate does not land), and
    // while it is parked the engine is killed OUT OF BAND — the `contrib/chaos` fault
    // at the release-confirm seam. The harness must read the release from the gone
    // table entry (evidence returns), not from a side effect of its own kill: it names
    // the CONFIRMED release (never a timeout), the victim is gone (never kept: G12's
    // "gone or kept, never both"), no engine survives on its port, and the next
    // admission still works.
    // Falsifier (run in place, revert): remove `commit_victim_release` from the
    // `(Dispose, Released(Ok))` arm in `src/scheduler/events.rs` — the unload-acceptance
    // race this leg exists to catch — and the "slot empty / no orphan / next admission"
    // asserts below go red while the timeout arm stays green. The unload row's
    // `unloaded` outcome stays the release wording this leg pins (live card-run
    // proof re-check: the
    // fix rewrites only the victim's LOAD row, never an unload row's note).
    let (handle, runtime) = hold_kill_harness(config(10));
    let victim = serve_hold_model(&handle).await;
    assert_eq!(runtime.server_count(), 1, "the victim engine is live");

    // Park exactly the unload's kill, then drive the operator unload: the reap is now
    // parked inside `kill` — a release in flight the test owns.
    runtime.hold_next_kills(1);
    assert!(matches!(
        handle
            .unload(CategoryId::Work)
            .await
            .expect("unload accepted"),
        Reply::Unloaded { unloading: 1, .. }
    ));
    wait_until(&handle, 5, |s| {
        s.jobs.iter().any(|j| {
            j.model == HOLD_MODEL && j.kind == JobKind::Unload && j.state == SwapState::Dispose
        })
    })
    .await;
    assert_eq!(
        runtime.server_count(),
        1,
        "the release is genuinely in flight: nothing has died yet"
    );

    // The chaos: the engine dies out of band while the harness's own terminate parks,
    // then the parked terminate is let go. The reap's `kill` finds the handle already
    // gone and its `release_confirmed` reads the gone entry — release evidence, not a
    // kill side effect.
    runtime.external_kill(victim.port).await;
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
        .find(|j| j.model == HOLD_MODEL && j.kind == JobKind::Unload)
        .expect("the unload is on the ledger");
    assert_eq!(
        job.outcome.as_deref(),
        Some("unloaded"),
        "the ledger names the confirmed release, never a timeout: {job:?}"
    );
    assert!(
        job.timings.release_ms.is_some(),
        "the release was proven, so it carries a measured duration: {:?}",
        job.timings
    );
    assert!(!job.is_fault, "a proven release is not a fault: {job:?}");
    assert_eq!(
        slot_state(&status, CategoryId::Work),
        vec!["empty".to_string()],
        "the victim is gone, not kept-and-dead (never both): {:?}",
        slot_model(&status, CategoryId::Work)
    );
    assert_eq!(runtime.server_count(), 0, "no orphan engine remains");
    assert!(
        runtime.is_dead(victim.port).await,
        "and no engine answers on the released port (G12)"
    );

    // The next admission still works: the freed slot takes a fresh model.
    assert!(matches!(
        handle
            .swap(INCOMING_MODEL, CategoryId::Work)
            .await
            .expect("the next load admits"),
        Reply::Loaded(_)
    ));
    assert_eq!(
        slot_model(
            &handle.status().await.expect("actor alive"),
            CategoryId::Work
        ),
        vec![Some(INCOMING_MODEL.to_string())],
        "the chaos left the slot reusable"
    );
    assert!(
        runtime.is_dead(victim.port).await,
        "the old corpse's port stays dead after the next admission"
    );
}

#[test]
fn chaos_harness_died_mid_drain_names_the_in_flight_job_and_orphans_nothing() {
    // Leg B. `contrib/chaos` kills the harness itself while a drain is in flight and
    // checks the survivor names the unfinished work and reaps no orphan.
    //
    // Cardless honesty about the method: the mock owns no OS process, and dropping the
    // `SchedulerHandle` does NOT stop the actor (it is a detached `tokio::spawn` and
    // the in-flight reap holds a channel sender), so an in-process analog cannot fake a
    // process `kill`. The closest honest analog: run the WHOLE harness on a private
    // tokio runtime and DROP that runtime — the actor task and the mock's engine
    // listeners all live on it (no OS child), so dropping it takes every one down,
    // which is what a job-object child with kill-on-close does in production. This leg
    // pins the LEDGER truth (the last snapshot names the in-flight drain job and its
    // releasing victims) and that no in-process engine listener outlives its owner.
    //
    // Stays card-only (the mock cannot prove the OS trait, so the leg does not claim
    // it): the OS reaping a real engine child via its job object when the harness
    // process is SIGKILLed, and the harness's own self-reap leaving no orphan across a
    // process boundary. Those need a freed card window, not this in-process fake.
    //
    // Falsifier (run in place, revert): make `is_committed_release_target` in
    // `src/scheduler/commands.rs` return `false` — the `releasing` label then never
    // names the draining victim and the "victim reads releasing at death" assert below
    // goes red while the loaded occupants stay green.
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
    let specs = [
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
    ];

    // Drive a real drain to the seam on a private runtime, snapshot the ledger at the
    // death moment, then kill the harness by dropping that runtime.
    let snapshot;
    let loaded_ports;
    let mock;
    {
        let actor_rt = tokio::runtime::Runtime::new().expect("actor runtime");
        let outcome = actor_rt.block_on(async {
            let (handle, mock) = harness5(cfg, &specs);
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
            // Book capacity fully: the greedy RP load must DRAIN both AUX victims;
            // the first victim's kill parks, so the drain is observable in flight.
            let driver = handle.clone();
            tokio::spawn(async move {
                let _ = driver.swap("greedy72", CategoryId::Rp).await;
            });
            wait_until(&handle, 5, |s| {
                s.jobs
                    .iter()
                    .any(|j| j.model == "greedy72" && j.state == SwapState::Dispose)
                    && s.slots
                        .iter()
                        .any(|sl| sl.model.as_deref() == Some("aux1") && sl.state == "releasing")
            })
            .await;
            let snap = handle.status().await.expect("actor alive");
            let ports: Vec<u16> = snap.slots.iter().filter_map(|sl| sl.port).collect();
            (snap, ports, mock)
        });
        snapshot = outcome.0;
        loaded_ports = outcome.1;
        mock = outcome.2;
        // The harness "dies": dropping the runtime aborts the actor and, with it, every
        // in-process engine listener it owned (the mock keeps its table entries, so
        // `server_count` stays a stale count — only the TCP `is_dead` truth is real).
        drop(actor_rt);
    }

    // The last ledger truth names the in-flight drain job.
    let drain = snapshot
        .jobs
        .iter()
        .find(|j| j.model == "greedy72")
        .expect("the admitting drain job is on the ledger at death");
    assert!(
        !drain.terminal && drain.state == SwapState::Dispose,
        "the death snapshot names the unfinished drain: {drain:?}"
    );
    assert_eq!(
        drain.kind,
        JobKind::Load,
        "a drain is a load row: {drain:?}"
    );
    let victim = snapshot
        .slots
        .iter()
        .find(|sl| sl.model.as_deref() == Some("aux1"))
        .expect("the drained victim keeps its row at death");
    assert_eq!(
        victim.state, "releasing",
        "the ledger names the victim being drained at death: {victim:?}"
    );

    // No engine listener outlives its owner: every port the harness held is refused
    // once the harness is gone. The mock owns no OS child, so the OS-level child reap
    // stays card-only (see the method note above).
    let check_rt = tokio::runtime::Runtime::new().expect("check runtime");
    for port in &loaded_ports {
        check_rt.block_on(async {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if mock.is_dead(*port).await {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "engine on port {port} outlived the harness that owned it"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });
    }
    assert!(
        !loaded_ports.is_empty(),
        "the death must have had live engines to orphan"
    );
}

#[tokio::test]
async fn a_nameless_yaml_loads_by_its_file_stem_end_to_end() {
    // End-to-end: the yaml carries no `name:` key at all.
    // The file stem is the identity the client loads by, the identity the actor
    // books into the slot, and (through `ModelEntry::name`) the engine log name.
    let dir = simpleharn::testutil::temp_dir("modelconf");
    std::fs::write(
        dir.join("stemonly.yaml"),
        "engine: mock\ncategory: work\nvram_gb: 4\n",
    )
    .expect("seed nameless model");
    let runtime = MockRuntime::new();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert("mock".to_string(), Arc::new(MockAdapter));
    let handle = Scheduler::start(
        config(5),
        dir,
        adapters,
        Arc::new(runtime.clone()) as Arc<dyn simpleharn::engine::EngineRuntime>,
        Arc::new(simpleharn::budget::LedgerOnlyWitness),
    );
    let reply = handle
        .swap("stemonly", CategoryId::Work)
        .await
        .expect("the stem is the loadable identity");
    match reply {
        Reply::Loaded(receipt) => assert_eq!(receipt.model, "stemonly"),
        other => panic!("expected Loaded, got {other:?}"),
    }
    let catalog = handle.catalog().await.expect("actor alive");
    assert_eq!(
        catalog
            .models
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["stemonly"],
        "and the catalog carries the stem"
    );
}
