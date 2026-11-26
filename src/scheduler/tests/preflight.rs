use super::*;

/// One `ActorState` with the shape `begin_spawn` consumes: a `Dispose` load
/// row on one slot whose plan-assigned engine port is `port`, the mock
/// adapter registered, the mock catalog row present, and a runtime that
/// counts spawns so "the refusal lands before the runtime" is an observed
/// fact. `cfg` travels unchanged so a pin can carry its own protected set.
fn preflight_state(
    port: u16,
    cfg: crate::config::AppConfig,
) -> (
    ActorState,
    Arc<SettleRuntime>,
    mpsc::UnboundedReceiver<ActorMsg>,
    JobId,
) {
    let (tx, rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(SettleRuntime {
        confirm: true,
        ..Default::default()
    });
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        cfg,
        crate::testutil::temp_dir("engine-port-preflight"),
        adapters,
        Arc::clone(&runtime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.catalog = vec![ModelEntry::from_flat(
        &BTreeMap::from([
            ("name".to_string(), "preflight-m".to_string()),
            ("engine".to_string(), "mock".to_string()),
            ("cost".to_string(), "fixed".to_string()),
        ]),
        "preflight-m",
    )
    .expect("valid row")];
    let ticket = SpawnTicket {
        generation: 3,
        kind: ResourceKind::ChildProcess,
        pid: 4_242,
        port,
    };
    let job_id = seat_spawn_row(&mut state, "preflight-m", SwapState::Dispose, &ticket, None);
    (state, runtime, rx, job_id)
}

/// The one verdict `begin_spawn`'s task sends for its job.
async fn next_actor_event(rx: &mut mpsc::UnboundedReceiver<ActorMsg>, job_id: JobId) -> Event {
    let message = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the spawn verdict lands inside the test deadline")
        .expect("the actor channel is open");
    let ActorMsg::Event { job, event } = message else {
        panic!("expected the spawn verdict for {job_id}");
    };
    assert_eq!(job, job_id, "the event belongs to the seated row");
    event
}

/// A port a foreign listener holds refuses the spawn before the runtime is
/// ever called, and the refusal names the blocked port and the excluded-range
/// class — the shape a reserved port must take so the operator reads a port
/// blocker on the job row instead of an engine that died at its own bind with
/// a bare exit code. Falsifier: on the pre-probe code path the runtime spawn
/// runs first, the scripted answer is `Spawned(Ok)`, and this pin dies at the
/// `SpawnFailed` match — there is no `SpawnFailed` reason containing the held
/// port pre-change, and `spawns()` is 1 instead of 0.
#[tokio::test]
async fn a_held_engine_port_refuses_the_spawn_with_a_named_blocker() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold an engine port");
    let port = held.local_addr().expect("held addr").port();
    let (mut state, runtime, mut rx, job_id) =
        preflight_state(port, crate::config::AppConfig::default());

    assert!(state.begin_spawn(job_id));
    let Event::Spawned(Err(crate::engine::SpawnError::SpawnFailed { reason })) =
        next_actor_event(&mut rx, job_id).await
    else {
        panic!("the held port must refuse the spawn by name");
    };
    assert!(
        reason.contains(&port.to_string()),
        "the blocker names the blocked port: {reason}"
    );
    assert!(
        reason.contains("excluded port range"),
        "the blocker hints at reserved Windows ranges: {reason}"
    );
    assert!(
        reason.contains("netsh int ipv4 show excludedportrange"),
        "the blocker names the operator's check command: {reason}"
    );
    assert!(
        !reason.contains("exited early") && !reason.contains("exit code"),
        "the probe's own words, never the generic early-exit text: {reason}"
    );
    assert_eq!(
        runtime.spawns(),
        0,
        "the refusal lands before the runtime spawn: a process that never started cannot exit early"
    );

    state.apply_event(
        job_id,
        Event::Spawned(Err(crate::engine::SpawnError::SpawnFailed { reason })),
    );
    assert!(
        drive_ram_release(&mut state, &mut rx).await,
        "the stamped spawn is reaped on the confirmed release"
    );
    let row = &state.jobs[&job_id];
    assert!(row.terminal);
    let detail = row
        .error
        .as_ref()
        .expect("the row keeps the refusal")
        .to_string();
    assert!(
        detail.contains(&port.to_string()),
        "the job detail keeps the blocked port: {detail}"
    );
    let projected = state
        .snapshot()
        .jobs
        .into_iter()
        .find(|job| job.id == job_id)
        .expect("the row projects to the jobs ledger");
    assert!(
        projected
            .error
            .as_deref()
            .is_some_and(|error| error.contains(&port.to_string())),
        "the projected job detail names the blocked port: {projected:?}"
    );
    assert!(projected.is_fault, "a refused load is a fault on the page");
}

/// The probe is a gate, not a detour: a bindable port passes it and the real
/// spawn runs exactly once, unchanged. Falsifier: make the probe refuse on a
/// successful bind (or hand its listener to the runtime instead of dropping
/// it) and the expected `Spawned(Ok)` answer never arrives.
#[tokio::test]
async fn a_bindable_engine_port_spawns_through_the_probe_unchanged() {
    let picked = std::net::TcpListener::bind("127.0.0.1:0").expect("pick a free port");
    let port = picked.local_addr().expect("addr").port();
    drop(picked);
    let (mut state, runtime, mut rx, job_id) =
        preflight_state(port, crate::config::AppConfig::default());

    assert!(state.begin_spawn(job_id));
    let Event::Spawned(Ok(spawned)) = next_actor_event(&mut rx, job_id).await else {
        panic!("a bindable port must spawn unchanged");
    };
    assert_eq!(spawned.port, port, "the plan's port is the spawn's port");
    assert_eq!(
        runtime.spawns(),
        1,
        "the probe never skips or replaces the real spawn"
    );
}

/// The probe's own safety fence: a protected port is never bound, even for
/// an instant. Here the protected coordinate is a held ephemeral stand-in,
/// so a probe that missed the filter refuses on the held listener instead of
/// skipping. Falsifier: drop the `protected_ports` filter in `begin_spawn`
/// and this leg answers `SpawnFailed` where the skip must answer `Spawned(Ok)`.
#[tokio::test]
async fn the_bind_probe_never_binds_a_protected_port() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("hold the protected stand-in");
    let port = held.local_addr().expect("addr").port();
    let mut cfg = crate::config::AppConfig::default();
    cfg.protected_ports.insert(port);
    let (mut state, runtime, mut rx, job_id) = preflight_state(port, cfg);

    assert!(state.begin_spawn(job_id));
    let Event::Spawned(Ok(spawned)) = next_actor_event(&mut rx, job_id).await else {
        panic!("a protected port must never be probed");
    };
    assert_eq!(spawned.port, port);
    assert_eq!(runtime.spawns(), 1, "the protected skip is not a refusal");
}
