use super::*;

fn seed_one_model(dir: &std::path::Path, name: &str) {
    std::fs::write(
        dir.join(format!("{name}.yaml")),
        format!("name: {name}\nengine: mock\ncategory: work\n"),
    )
    .expect("model yaml");
}

/// Test double for the reconcile probe: canned `docker ps`
/// answers consumed one per probe (`None` = docker could not answer) and
/// the declared-name lists it was asked about. Everything else keeps the
/// NeverRuntime posture: a load reaching this runtime is a test bug.
struct ReconcileRuntime {
    answers: Mutex<std::collections::VecDeque<Option<BTreeSet<String>>>>,
    asked: Mutex<Vec<Vec<String>>>,
}

impl ReconcileRuntime {
    fn new(answers: Vec<Option<BTreeSet<String>>>) -> Self {
        ReconcileRuntime {
            answers: Mutex::new(answers.into()),
            asked: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for ReconcileRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the R5 reconcile pin must never load");
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        panic!("the R5 reconcile pin must never load");
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        panic!("the R5 reconcile pin must never load");
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the R5 reconcile pin must never load");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the R5 reconcile pin must never load");
    }
    async fn unclaimed_container_names(&self, declared: &[String]) -> Option<BTreeSet<String>> {
        self.asked
            .lock()
            .expect("asked lock")
            .push(declared.to_vec());
        self.answers
            .lock()
            .expect("answers lock")
            .pop_front()
            .unwrap_or(None)
    }
}

/// State shaped for the reconcile pins: `on_reconcile_done` is called directly, so
/// the runtime is never reached and the actor channel never read.
fn reconcile_state() -> ActorState {
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    ActorState::empty(
        crate::config::AppConfig::default(),
        crate::testutil::temp_dir("r5-state"),
        adapters,
        Arc::new(ReconcileRuntime::new(Vec::new())) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    )
}

/// Stranded-container warns captured for one container name: the message
/// half pins the report-only wording, the field half the named container.
fn stranded_warns(capture: &Arc<Mutex<Vec<CapturedWarn>>>, container: &str) -> usize {
    capture
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(message, fields)| {
            message.contains("unclaimed running container")
                && message.contains("no claim in the ledger")
                && fields
                    .get("container")
                    .is_some_and(|name| name.contains(container))
        })
        .count()
}

#[tokio::test]
async fn the_reconcile_warns_once_and_an_unchanged_set_never_re_warns() {
    let capture = install_warn_capture();
    // Warm the reporting callsite and PROVE delivery before the measured
    // window (the D9 rule): a sentinel name no assertion below reads about.
    let mut warm = reconcile_state();
    warm.on_reconcile_done(Some(["r5-warmup".to_string()].into_iter().collect()));
    assert!(
        stranded_warns(&capture, "r5-warmup") == 1,
        "the reporting callsite was warmed but never delivered"
    );

    let mut state = reconcile_state();
    let stranded: BTreeSet<String> = ["r5-unit-a".to_string()].into_iter().collect();
    state.on_reconcile_done(Some(stranded.clone()));
    assert_eq!(
        stranded_warns(&capture, "r5-unit-a"),
        1,
        "the first report names the stranded container once"
    );
    state.on_reconcile_done(Some(stranded.clone()));
    assert_eq!(
        stranded_warns(&capture, "r5-unit-a"),
        1,
        "repeated rescans of the same stranded set must not spam the ring"
    );
    state.on_reconcile_done(None);
    state.on_reconcile_done(Some(stranded));
    assert_eq!(
        stranded_warns(&capture, "r5-unit-a"),
        1,
        "a docker that briefly could not answer must not re-warn what the next probe re-confirms"
    );
}

/// The `/har/status` rider of the reconcile report: the SAME dedupe state backs the
/// payload, so page and log can never disagree. Polarity pinned both ways
/// (the inverted-`skip_serializing_if` GOTCHAS class): an empty last-reported
/// set normalizes to `None` and the key is ABSENT from the serialized
/// status, never `null` or `[]`; a failed probe keeps the last-reported set
/// exactly like the warn dedupe.
#[tokio::test]
async fn the_status_snapshot_carries_the_stranded_set_and_drops_it_when_empty() {
    let mut state = reconcile_state();
    assert!(
        state.snapshot().stranded_containers.is_none(),
        "no probe yet: the key has no shape to leak"
    );
    state.on_reconcile_done(None);
    assert!(
        state.snapshot().stranded_containers.is_none(),
        "a failed probe invents nothing for the page either"
    );
    let stranded: BTreeSet<String> = ["r5-payload".to_string()].into_iter().collect();
    state.on_reconcile_done(Some(stranded.clone()));
    let reported = state.snapshot();
    assert_eq!(reported.stranded_containers, Some(stranded));
    assert_eq!(
        serde_json::to_value(&reported).expect("status serializes")["stranded_containers"],
        serde_json::json!(["r5-payload"]),
        "the warned names ride /har/status verbatim"
    );
    state.on_reconcile_done(None);
    assert!(
            state.snapshot().stranded_containers.is_some(),
            "a docker that briefly could not answer keeps the last-reported set (same rule as the warn)"
        );
    state.on_reconcile_done(Some(BTreeSet::new()));
    let cleared = state.snapshot();
    assert!(cleared.stranded_containers.is_none());
    assert!(
        serde_json::to_value(&cleared)
            .expect("status serializes")
            .get("stranded_containers")
            .is_none(),
        "empty means ABSENT, not null or []: the skip predicate must not invert"
    );
}

#[tokio::test]
async fn a_failed_or_empty_reconcile_never_warns() {
    let capture = install_warn_capture();
    let mut state = reconcile_state();
    state.on_reconcile_done(None);
    state.on_reconcile_done(Some(BTreeSet::new()));
    assert_eq!(
        stranded_warns(&capture, "r5-quiet"),
        0,
        "a failed probe and an empty stranded set are both silence"
    );
    let stranded: BTreeSet<String> = ["r5-quiet".to_string()].into_iter().collect();
    state.on_reconcile_done(Some(stranded.clone()));
    assert_eq!(stranded_warns(&capture, "r5-quiet"), 1);
    state.on_reconcile_done(Some(BTreeSet::new()));
    assert_eq!(
        stranded_warns(&capture, "r5-quiet"),
        1,
        "the stranded container dying is a changed set with nothing left to name"
    );
    state.on_reconcile_done(Some(stranded));
    assert_eq!(
        stranded_warns(&capture, "r5-quiet"),
        2,
        "reappearing is a changed set again: one fresh warn"
    );
}

#[tokio::test]
async fn a_rescan_reports_a_stranded_container_once_across_the_actor() {
    // End-to-end: the boot scan and every rescan trigger the reconcile; the
    // warn lands once for the first sighting, the unchanged rescan is quiet,
    // and a CHANGED set warns again — through the live actor loop, not a
    // hand-called handler.
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("r5-e2e");
    std::fs::write(
            dir.join("r5-model.yaml"),
            "name: r5-model\nengine: ninfer\ncategory: work\nvram_gb: 2\nninfer_image: ninfer:local\nninfer_container: r5-e2e-a\n",
        )
        .expect("seed ninfer yaml");
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    adapters.insert(
        "ninfer".to_string(),
        Arc::new(crate::engine::ninfer::NinferAdapter::new("Ubuntu", "ops"))
            as Arc<dyn EngineAdapter>,
    );
    let stranded_a: BTreeSet<String> = ["r5-e2e-a".to_string()].into_iter().collect();
    let stranded_b: BTreeSet<String> = ["r5-e2e-b".to_string()].into_iter().collect();
    let probe = Arc::new(ReconcileRuntime::new(vec![
        Some(stranded_a.clone()),
        Some(stranded_a.clone()),
        Some(stranded_b.clone()),
    ]));
    let handle = Scheduler::start(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        probe.clone() as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while stranded_warns(&capture, "r5-e2e-a") < 1 {
        assert!(
            Instant::now() < deadline,
            "the boot-scan reconcile never reported the stranded container"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        probe
            .asked
            .lock()
            .expect("asked lock")
            .iter()
            .any(|names| names.iter().any(|name| name == "r5-e2e-a")),
        "the probe must be asked with the catalog's declared `ninfer_container` names"
    );
    let status = handle.status().await.expect("actor alive");
    assert_eq!(status.stranded_containers, Some(stranded_a));
    handle.reload().await.expect("actor alive");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        stranded_warns(&capture, "r5-e2e-a"),
        1,
        "a rescan of an unchanged stranded set must not re-warn"
    );
    std::fs::write(
            dir.join("r5-model.yaml"),
            "name: r5-model\nengine: ninfer\ncategory: work\nvram_gb: 2\nninfer_image: ninfer:local\nninfer_container: r5-e2e-b\n",
        )
        .expect("edit container name");
    handle.reload().await.expect("actor alive");
    while stranded_warns(&capture, "r5-e2e-b") < 1 {
        assert!(
            Instant::now() < deadline,
            "the changed stranded set was never reported"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        stranded_warns(&capture, "r5-e2e-a"),
        1,
        "the departed name is not re-warned by the changed set"
    );
    let status = handle.status().await.expect("actor alive");
    assert_eq!(
        status.stranded_containers,
        Some(stranded_b),
        "the page rides the same replaced set the log warned about"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn actor_state_construction_never_scans_the_catalog() {
    // Boot-thread pin: the constructor that `Scheduler::start`
    // funnels through — including the `ActorState::empty` path it delegates to —
    // must return against a dir holding a valid model with the catalog still
    // empty. Re-introducing the scan into the constructor (the pre-bind stall)
    // fails this assertion. Deterministic: no task has run, so nothing can have
    // scanned yet.
    let dir = crate::testutil::temp_dir("bootscan");
    seed_one_model(&dir, "boot-stub");
    let wiring = boot_stub_wiring();
    let state = ActorState::new(
        crate::config::AppConfig::default(),
        dir.clone(),
        wiring.adapters,
        wiring.runtime,
        wiring.witness,
        wiring.tx,
    );
    assert!(
        state.catalog.is_empty(),
        "the constructor must not scan: a pre-bind catalog read is the \
             FINDINGS-14 [1] stall; {:?}",
        state
            .catalog
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    );
    let wiring = boot_stub_wiring();
    let state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        wiring.adapters,
        wiring.runtime,
        wiring.witness,
        wiring.tx,
    );
    assert!(
        state.catalog.is_empty(),
        "ActorState::empty must not scan either; {:?}",
        state
            .catalog
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn actor_task_performs_the_initial_scan() {
    // Actor-side pin: the scan `ActorState::new` no longer runs
    // must still happen — as the first thing the actor task does. A command sent
    // immediately after `start()` (here the catalog read) queues behind the scan
    // on the unbounded channel and observes its result; no sleep or timing
    // assumption is involved, only a bounded poll for the eventual occupancy of
    // the seeded model.
    let dir = crate::testutil::temp_dir("bootscan");
    seed_one_model(&dir, "boot-model");
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let handle = Scheduler::start(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let catalog = handle.catalog().await.expect("actor alive");
        if catalog
            .models
            .iter()
            .any(|entry| entry.name == "boot-model")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the actor-side initial scan never populated the catalog: {:?}",
            catalog
                .models
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_during_an_in_flight_rescan_is_answered() {
    // The reload scan parks on a test-only seam; while it is
    // in flight the actor must keep answering commands, and the reload reply
    // may resolve only when the new catalog is APPLIED (never when the scan
    // was dispatched — the /har/rescan contract). Pre-fix (the inline scan),
    // the parked scan holds the actor thread and the status poll below times
    // out — which is exactly why this test runs on a multi-thread runtime:
    // the timer has to survive a thread that a blocking scan has parked.
    let dir = crate::testutil::temp_dir("reloadresp");
    seed_one_model(&dir, "resident");
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let handle = Scheduler::start(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
    );
    // The boot scan must have landed before the seam is armed: the seam is
    // single-shot for this dir and belongs to the reload alone.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let catalog = handle.catalog().await.expect("actor alive");
        if catalog.models.iter().any(|entry| entry.name == "resident") {
            break;
        }
        assert!(Instant::now() < deadline, "the boot scan never landed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    std::fs::write(
        dir.join("late.yaml"),
        "engine: mock\ncategory: work\nvram_gb: 2\n",
    )
    .expect("seed late model");

    let (started, release) = crate::model::scan::arm_scan_pause(&dir);
    let client = handle.clone();
    let reload = tokio::spawn(async move { client.reload().await });
    let grabbed = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || started.recv().is_ok()),
    )
    .await;
    assert!(
        matches!(grabbed, Ok(Ok(true))),
        "the reload scan never parked on the seam (is scan_dir off-thread?)"
    );

    let answered = tokio::time::timeout(Duration::from_secs(2), handle.status()).await;
    assert!(
        matches!(answered, Ok(Ok(_))),
        "the actor stopped answering while a rescan was in flight"
    );
    assert!(
        !reload.is_finished(),
        "the reload reply resolved before the scan even completed"
    );

    drop(release);
    let reply = tokio::time::timeout(Duration::from_secs(10), reload)
        .await
        .expect("the reload resolves once released")
        .expect("join")
        .expect("reply");
    assert_eq!(reply, Reply::Done);
    let catalog = handle.catalog().await.expect("actor alive");
    assert!(
        catalog.models.iter().any(|entry| entry.name == "late"),
        "the reply meant applied: the new model must be in the catalog"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unavailable_engine_is_dropped_and_named() {
    // The audit verdict pin: no
    // fixture publishes an engine its adapter map lacks, and such an entry can
    // only ever die later as `no adapter registered` — so the actor refuses it
    // at apply time, DROPPING it with a typed, page-visible reason.
    let dir = crate::testutil::temp_dir("engineunavail");
    std::fs::write(
        dir.join("ghost.yaml"),
        "engine: phantom\ncategory: work\nvram_gb: 2\n",
    )
    .expect("seed ghost");
    seed_one_model(&dir, "resident");
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let handle = Scheduler::start(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let catalog = loop {
        let catalog = handle.catalog().await.expect("actor alive");
        let reported = catalog
            .errors
            .iter()
            .any(|e| e.kind == crate::model::scan::ScanKind::EngineUnavailable);
        let settled = catalog.models.iter().any(|entry| entry.name == "resident");
        assert!(
            Instant::now() < deadline,
            "scan never reported: {catalog:?}"
        );
        if reported && settled {
            break catalog;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        !catalog.models.iter().any(|entry| entry.name == "ghost"),
        "an engine with no adapter must be dropped from the catalog"
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == crate::model::scan::ScanKind::EngineUnavailable)
        .expect("the drop must be reported");
    assert_eq!(error.file, "ghost.yaml");
    assert!(
        error.reason == "engine \"phantom\" is not available to this harness (known: mock)",
        "{}",
        error.reason
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The fingerprint-change predicate table, including the
/// unreadable-stat windows the actor path cannot race into existence
/// deterministically (the scan that publishes the bytes rides the same read
/// the stat does), so the `None` legs are manufactured here directly: a
/// missing fingerprint — at registration or at rescan — is the absence of
/// evidence, never evidence of change. Falsifiers: treating `None` as change
/// fails every KEEP leg; disabling the invalidation fails both RETIRE legs;
/// an always-retire predicate fails KEEP 1 (whose actor-path twin is
/// `an_unchanged_rescan_keeps_the_queue_and_the_parked_load_still_loads`).
#[test]
fn a_rescan_keeps_a_queue_whose_fingerprint_cannot_show_a_change() {
    let dir = crate::testutil::temp_dir("rescan-fp");
    let yaml = dir.join("m.yaml");
    let entry_on_disk = |bytes: &str| -> ModelEntry {
        std::fs::write(&yaml, bytes).expect("seed");
        let (map, _) = crate::model::scan::parse_flat_yaml("engine: mock\nvram_gb: 4\n");
        let mut entry = ModelEntry::from_flat(&map, "m").expect("entry");
        entry.file = Some(yaml.clone());
        entry
    };
    let fresh_state = || -> ActorState {
        let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
        let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
        adapters.insert(
            "mock".to_string(),
            Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
        );
        ActorState::empty(
            crate::config::AppConfig::default(),
            dir.clone(),
            adapters,
            Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
            Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
            tx,
        )
    };
    let queue_on = |state: &mut ActorState, slot: usize, entry: &ModelEntry| -> JobId {
        state.slots = vec![Slot {
            category: CategoryId::Work,
            index: slot,
            engine_port: 18_100 + slot as u16,
            occupant: None,
            // The parked row does not own the slot: some other job is in
            // flight on it, exactly as parking requires.
            job: Some(JobId(9_999)),
        }];
        let id = state.register_queued_load(slot, entry, Vec::new());
        state.pending.insert(slot, id);
        id
    };
    let apply = |state: &mut ActorState, models: Vec<ModelEntry>| {
        let catalog = crate::model::scan::Catalog {
            models,
            errors: Vec::new(),
        };
        let drift = drift_stats_for(state, &catalog);
        state.apply_catalog(catalog, None, &drift);
    };
    let queued_alive = |state: &ActorState, id: JobId| {
        state.jobs.get(&id).is_some_and(|j| !j.terminal) && state.pending.values().any(|p| *p == id)
    };

    // KEEP 1: same bytes, same clock — the no-change rescan must not drop
    // the queue (budget kept intact, place kept, row non-terminal).
    let entry = entry_on_disk("vram_gb: 4\n");
    let mut state = fresh_state();
    let id = queue_on(&mut state, 0, &entry);
    apply(&mut state, vec![entry.clone()]);
    assert!(
        queued_alive(&state, id),
        "a no-change rescan must never drop a queue"
    );

    // KEEP 2: the file became unstat'able by apply time (bytes published
    // from the earlier read, the stat window lost the file) — a missing
    // rescan-side fingerprint is not evidence.
    let entry = entry_on_disk("vram_gb: 4\n");
    let mut state = fresh_state();
    let id = queue_on(&mut state, 0, &entry);
    std::fs::remove_file(&yaml).expect("delete after the scan read");
    apply(&mut state, vec![entry.clone()]);
    assert!(
        queued_alive(&state, id),
        "an unreadable rescan-time stat must not retire"
    );

    // KEEP 3: the row queued on an unreadable file (no registration
    // baseline at all) — even readable-and-changed bytes now prove nothing.
    let entry = entry_on_disk("vram_gb: 4\n");
    let mut state = fresh_state();
    let id = queue_on(&mut state, 0, &entry);
    state.jobs.get_mut(&id).expect("row").queue_fingerprint = None;
    std::fs::write(&yaml, "vram_gb: 400\n").expect("edit");
    apply(&mut state, vec![entry.clone()]);
    assert!(
        queued_alive(&state, id),
        "a row without a registration baseline survives the rescan"
    );

    // RETIRE: bytes moved against a real baseline — the named reason is on
    // the row and the queue place is released.
    let entry = entry_on_disk("vram_gb: 4\n");
    let mut state = fresh_state();
    let id = queue_on(&mut state, 0, &entry);
    std::fs::write(&yaml, "vram_gb: 400\n").expect("edit");
    apply(&mut state, vec![entry]);
    assert!(
        state.jobs[&id].terminal,
        "moved bytes must retire the intent"
    );
    assert!(state.pending.is_empty(), "and release the queue place");
    let error = state.jobs[&id]
        .error
        .clone()
        .expect("the reason rides the ledger row")
        .to_string();
    assert!(
        error.contains("superseded by rescan")
            && error.contains("m.yaml")
            && error.contains("changed since queued")
            && error.contains("re-queue to apply"),
        "{error}"
    );

    // RETIRE: the model left the catalog.
    let entry = entry_on_disk("vram_gb: 4\n");
    let mut state = fresh_state();
    let id = queue_on(&mut state, 0, &entry);
    apply(&mut state, Vec::new());
    assert!(
        state.jobs[&id].terminal,
        "a vanished model retires its intent"
    );
    assert!(
        state.jobs[&id].error.as_ref().is_some_and(|e| e
            .to_string()
            .contains("superseded by rescan (removed from the catalog)")),
        "{:?}",
        state.jobs[&id].error
    );

    // KEEP 4: the dispatch-to-apply corner:
    // row A parked M at dispatch (its file gathered pre-edit), the scan
    // held, an operator edit landed, and row B parked the same model before
    // the scan landed. B's fingerprint is newer than any stat gathered
    // before its birth — judging it could only invent "changed since
    // queued" — so rows registered after the dispatch are kept
    // unconditionally. Falsifier: drop the `queued_at_dispatch` gate and B
    // lands in the (Some(newer), Some(older)) arm and retires.
    let entry = entry_on_disk("vram_gb: 4\n");
    let mut state = fresh_state();
    let a = queue_on(&mut state, 0, &entry);
    let gathered = drift_stats_for(
        &state,
        &crate::model::scan::Catalog {
            models: vec![entry.clone()],
            errors: Vec::new(),
        },
    );
    let edited = entry_on_disk("vram_gb: 400\n");
    let b = queue_on(&mut state, 1, &edited);
    state.apply_catalog(
        crate::model::scan::Catalog {
            models: vec![edited],
            errors: Vec::new(),
        },
        None,
        &gathered,
    );
    assert!(
        queued_alive(&state, b),
        "a row registered after the reload was dispatched must survive it"
    );
    assert!(
        queued_alive(&state, a),
        "and the gathered evidence shows no change for the row it did see"
    );
}

/// P4: a row whose engine lists this host publishes without a platform
/// warning. No shipped engine is host-unsupported today (ninfer moved to both
/// hosts with the in-distro container); the refusal helper itself is pinned by
/// `every_row_lists_a_host_and_ninfer_lists_both_hosts`.
#[test]
fn a_row_that_lists_this_host_does_not_warn_and_still_publishes() {
    let dir = crate::testutil::temp_dir("platform-scan");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "ninfer".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir,
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let (map, problems) =
        crate::model::scan::parse_flat_yaml("engine: ninfer\ncategory: work\nvram_gb: 2\n");
    assert!(problems.is_empty(), "{problems:?}");
    let entry = ModelEntry::from_flat(&map, "both-host-row").expect("entry");
    state.apply_catalog(
        crate::model::scan::Catalog {
            models: vec![entry],
            errors: Vec::new(),
        },
        None,
        &DriftStats::default(),
    );
    assert!(
        state.catalog.iter().any(|m| m.name == "both-host-row"),
        "the row still publishes: {:?}",
        state.catalog.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    assert!(
        !state
            .catalog_errors
            .iter()
            .any(|e| e.reason.contains("runs on")),
        "a listed host never warns: {:?}",
        state.catalog_errors
    );
}

/// P4: a row whose engine lists this host reaches the adapter lookup, never the
/// platform refusal.
#[test]
fn a_row_that_lists_this_host_reaches_the_adapter_lookup() {
    let dir = crate::testutil::temp_dir("platform-load");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir,
        BTreeMap::new(),
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let (map, problems) =
        crate::model::scan::parse_flat_yaml("engine: ninfer\ncategory: work\nvram_gb: 2\n");
    assert!(problems.is_empty(), "{problems:?}");
    let entry = ModelEntry::from_flat(&map, "both-host-row").expect("entry");
    let job = state.register_queued_load(0, &entry, Vec::new());
    match state.preflight_spec(job) {
        Err(crate::scheduler::SchedulerError::Spawn(reason)) => {
            assert!(
                reason.contains("no adapter registered"),
                "the fixture has no adapter, so the lookup owns this answer: {reason}"
            );
        }
        other => panic!("expected the adapter lookup to speak, got {other:?}"),
    }
}
