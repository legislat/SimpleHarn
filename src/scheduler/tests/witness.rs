use super::*;

/// A persist the disk refuses must not leave memory booking what disk denied
/// (closing the warn-only tail): the warn fires AND the
/// catalog entry's measured triple drains to its none state. The entry is a
/// `llama` row so the persist really carries the stamped triple — non-offload
/// engines never mint the stamp (`only_offload_engines_stamp_measured_offload`).
/// The wedged-drive handle is Windows-only, so the test is gated like its
/// writeback sibling.
#[cfg(windows)]
#[tokio::test]
async fn a_refused_measured_persist_drains_the_triple_from_memory() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("measured-persist");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("drift.yaml");
    std::fs::write(&yaml, "engine: llama\ncategory: work\nvram_gb: 2\n").expect("write");

    // Refuse the validated rename the way a wedged drive does: hold the target
    // open with delete-sharing off. Reads still succeed (the batch must be
    // able to read the file), the replace cannot.
    let _lock = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x3) // FILE_SHARE_READ | FILE_SHARE_WRITE (no DELETE)
            .open(&yaml)
            .expect("exclusive handle")
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let (map, problems) =
        crate::model::scan::parse_flat_yaml("engine: llama\ncategory: work\nvram_gb: 2\n");
    assert!(problems.is_empty(), "{problems:?}");
    let mut entry = ModelEntry::from_flat(&map, "drift").expect("entry");
    entry.file = Some(yaml.clone());
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port: 18_991,
    };
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 18_991);

    state.witness("drift", &ticket, 3.7);
    assert_eq!(
        state.catalog[0].measured_gb,
        Some(3.7),
        "the reading books in memory first"
    );

    let msg = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("the persist failure reaches the actor")
        .expect("channel open");
    let ActorMsg::MeasuredPersistFailed { model, data, file } = msg else {
        panic!("expected the persist-failed reconcile message");
    };
    assert_eq!(model, "drift");
    assert_eq!((data.high, data.low, data.offload), (3.7, 3.7, Some(false)));
    assert_eq!(
        data.attribution,
        Some(crate::model::MeasuredAttribution::PerProcess),
        "concrete bytes for the engine's own pid stamp the reading per_process"
    );
    assert_eq!(
        file.as_deref(),
        Some(yaml.as_path()),
        "the reconcile names the spawn-era row it refused to write"
    );

    state.fail_persist(&model, data, file);
    let entry = &state.catalog[0];
    assert_eq!(entry.measured_gb, None);
    assert_eq!(entry.measured_min_gb, None);
    assert_eq!(entry.measured_offload, None);
    assert_eq!(
        entry.measured_attribution, None,
        "a range the disk refused must not keep the provenance that would book it"
    );

    let warns = capture.lock().expect("capture lock");
    assert!(
        warns.iter().any(|(message, fields)| {
            message.contains("cannot persist the measured range")
                && fields.get("model").is_some_and(|m| m.contains("drift"))
        }),
        "the warn must have fired: {warns:?}"
    );
    // And the file really never gained the keys the memory had booked.
    let text = std::fs::read_to_string(&yaml).expect("read back");
    assert!(
        !text.contains("measured_gb"),
        "disk refused the triple: {text}"
    );

    drop(_lock);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The fold books the row that owns the spawn-era FILE, while the
/// drain keyed `name && file`. Wherever the file's row answers to a different name
/// than the spawn-era string, the drain missed the very row it exists to clear:
/// memory kept booking a footprint the disk refused for the whole serve window
/// (later samples reproduce the same high/low against the updated anchor range, so
/// `changed` stays false and no retry persist ever fires — it healed only on
/// rescan). Scan derives identity from the file stem today, so the state is built by
/// hand: it is the vocabulary the DEFERRED `name:`-key hard-reject opens up, and the
/// drain must not be the line that turns it into a leak. Falsifier: restore the
/// `name && file` key and the renamed row keeps `Some(3.7)` after the drain.
#[cfg(windows)]
#[tokio::test]
async fn the_refused_persist_drain_follows_the_file_the_fold_booked() {
    let _capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("measured-drain-file");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("renamed.yaml");
    std::fs::write(&yaml, "engine: llama\ncategory: work\nvram_gb: 2\n").expect("write");
    let _lock = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x3)
            .open(&yaml)
            .expect("exclusive handle")
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let (map, problems) =
        crate::model::scan::parse_flat_yaml("engine: llama\ncategory: work\nvram_gb: 2\n");
    assert!(problems.is_empty(), "{problems:?}");
    let mut spawn_era = ModelEntry::from_flat(&map, "renamed").expect("entry");
    spawn_era.file = Some(yaml.clone());
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4243,
        port: 18_997,
    };
    state.catalog = vec![spawn_era.clone()];
    seat_spawn(&mut state, &spawn_era, &ticket, 18_997);

    // The reload the finding names: the same bytes on disk, a row that answers to a
    // different name. The engine is still serving the spawn-era string.
    state.catalog = vec![ModelEntry {
        name: "moved".to_string(),
        ..spawn_era.clone()
    }];

    state.witness("renamed", &ticket, 3.7);
    assert_eq!(
        state.catalog[0].measured_gb,
        Some(3.7),
        "the fold reaches the row by file, whatever it is called now"
    );

    let msg = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("the persist failure reaches the actor")
        .expect("channel open");
    let ActorMsg::MeasuredPersistFailed { model, data, file } = msg else {
        panic!("expected the persist-failed reconcile message");
    };
    assert_eq!(
        model, "renamed",
        "the message carries the spawn-era name, which is what used to defeat the drain"
    );
    assert_eq!(file.as_deref(), Some(yaml.as_path()));

    state.fail_persist(&model, data, file);
    let entry = &state.catalog[0];
    assert_eq!(
        entry.measured_gb, None,
        "memory may not book what the disk refused, whichever name the row answers to"
    );
    assert_eq!(entry.measured_min_gb, None);
    assert_eq!(entry.measured_offload, None);

    drop(_lock);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A spawn whose anchor carries `file: None` passed the serving
/// and anchor checks and returned early WITHOUT retiring the sampler — the one
/// reading class that left the task walking the witness every `measured_sample_s`
/// until its 24 h deadline while the actor discarded every reading it produced.
/// `apply_catalog` keeps file-None rows whose engine has an adapter, so the door is
/// config-inert, not type-inert. Falsifier: delete the `retire_sampler` call from
/// the file-None arm and the sampler is still attached.
#[tokio::test]
async fn a_synthetic_anchor_retires_the_sampler_that_has_nowhere_to_write() {
    let capture = install_warn_capture();
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        crate::testutil::temp_dir("synthetic-anchor"),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let (map, problems) = crate::model::scan::parse_flat_yaml("engine: mock\n");
    assert!(problems.is_empty(), "{problems:?}");
    // `from_flat` is the only producer of a `file: None` row: the synthetic shape.
    let entry = ModelEntry::from_flat(&map, "synthetic").expect("entry");
    assert_eq!(entry.file, None, "the shape under test");
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4343,
        port: 18_998,
    };
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 18_998);
    let sampler = tokio::spawn(tokio::time::sleep(Duration::from_secs(3600)));
    state
        .slots
        .first_mut()
        .and_then(|slot| slot.occupant.as_mut())
        .expect("a seated occupant")
        .measure = Some(sampler);

    state.witness("synthetic", &ticket, 3.7);

    let measure = &state
        .slots
        .first()
        .and_then(|slot| slot.occupant.as_ref())
        .expect("a seated occupant")
        .measure;
    assert!(
        measure.is_none(),
        "a reading with no operator file may not leave a sampler polling for a day"
    );
    let warns = capture.lock().expect("capture lock");
    assert!(
        warns
            .iter()
            .any(|(message, _)| message.contains("no operator file")),
        "the retirement has to be visible: {warns:?}"
    );
}

/// The install-time `serve_model` verdict read the
/// LIVE catalog by name, minutes after `begin_spawn`. Under a mid-serve reload that
/// rewrites the row's `serve_model`, the warning was computed against yaml the
/// running process was never launched from — a false warning, or a missed one — and
/// under same-name twins it could quote the other twin's id. The verdict now rides
/// the spawn-era anchor. Falsifier: judge from the live row instead of the anchor
/// and the `declared-A`/`declared-B` legs fail.
#[test]
fn the_serve_model_verdict_is_judged_against_the_spawn_era_row() {
    let (map, problems) =
        crate::model::scan::parse_flat_yaml("engine: mock\nserve_model: declared-A\n");
    assert!(problems.is_empty(), "{problems:?}");
    let spawn_era = ModelEntry::from_flat(&map, "served").expect("entry");
    let anchor = MeasuredAnchor::at_spawn(&spawn_era);

    let warning = serve_model_warning(Some(&anchor), Some("actual"))
        .expect("the yaml claims an id the engine does not report");
    assert!(warning.contains("declared-A"), "{warning}");

    // The reload the finding names: the same model, a different id on disk now.
    let (reloaded_map, _) =
        crate::model::scan::parse_flat_yaml("engine: mock\nserve_model: declared-B\n");
    let reloaded = ModelEntry::from_flat(&reloaded_map, "served").expect("entry");
    assert_eq!(reloaded.serve_model.as_deref(), Some("declared-B"));
    assert!(
        !warning.contains("declared-B"),
        "yaml that arrived after the launch may not rewrite what counted as a \
             mismatch: {warning}"
    );

    assert!(
        serve_model_warning(Some(&anchor), Some("declared-A")).is_none(),
        "an id the engine really reports is not a mismatch"
    );
    assert!(
        serve_model_warning(Some(&anchor), None).is_none(),
        "no discovered id is nothing to compare against"
    );
    assert!(
        serve_model_warning(None, Some("actual")).is_none(),
        "a job that never spawned holds no spawn-era claim"
    );

    // And the site may not quietly go back to describing the process from live
    // catalog bytes.
    let source = include_str!("../events.rs");
    let site = source
        .split("let serve_id = discovered_id;")
        .nth(1)
        .expect("the install verdict site is still here")
        .split("if let Some(slot) = slot")
        .next()
        .expect("the site ends where the slot work begins");
    assert!(
        !site.contains("self.catalog"),
        "the install verdict must come from the anchor, not the live catalog: {site}"
    );
}

/// `apply_catalog` ran `self.cfg.llama_server_exe.exists()` inside
/// the single writer's turn — the last synchronous `std::fs` touch on the actor.
/// The exe path is operator config, so that stat can park the actor behind a
/// sleeping USB or a disconnected NAS volume with the listener already bound, and
/// every command, `/har/status` poll and tray tick queues behind it (the A1-class
/// stall moved the scans off-thread for). The verdict now
/// travels in from the blocking leg. Falsifier: restore the stat inside
/// `apply_catalog` and the first leg fails (an absent binary reports itself even
/// when the scan leg said nothing), and the file-wide count leg fails.
#[test]
fn the_actor_applies_the_binary_verdict_it_is_handed_and_never_stats_for_one() {
    let source = include_str!("../catalog.rs");
    // Counted over the production half only, and with a needle built at compile
    // time so this assertion's own text cannot feed on itself.
    let production = source.split("mod tests").next().unwrap_or(source);
    let needle = concat!(".exists", "()");
    assert_eq!(
        production.match_indices(needle).count(),
        1,
        "the actor half must hold exactly one synchronous stat — the one inside \
             `missing_binary_error`, which runs on a blocking thread"
    );

    let dir = crate::testutil::temp_dir("binary-verdict");
    let absent = dir.join("llama-server.exe");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let config = crate::config::AppConfig {
        llama_server_exe: Some(absent.clone()),
        ..crate::config::AppConfig::default()
    };
    let mut state = ActorState::empty(
        config,
        dir.clone(),
        BTreeMap::new(),
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );

    assert!(
        missing_binary_error(Some(&absent)).is_some(),
        "the fixture binary really is absent"
    );
    state.apply_catalog(
        crate::model::scan::Catalog::empty(),
        None,
        &DriftStats::default(),
    );
    assert!(
        !state
            .catalog_errors
            .iter()
            .any(|error| error.kind == ScanKind::EngineBinaryMissing),
        "the actor may not answer from a stat of its own: {:?}",
        state.catalog_errors
    );

    state.apply_catalog(
        crate::model::scan::Catalog::empty(),
        missing_binary_error(Some(&absent)),
        &DriftStats::default(),
    );
    let report = state
        .catalog_errors
        .iter()
        .find(|error| error.kind == ScanKind::EngineBinaryMissing)
        .expect("the carried verdict is published");
    assert!(
        report.file == "config"
            && report.reason.contains(&absent.display().to_string())
            && report
                .reason
                .contains("without a row `llama_server_exe` will fail"),
        "the page-visible sentence keeps naming the path and the consequence: {:?}",
        report.reason
    );

    // The other polarity: a binary that is there is nothing to report.
    let present = dir.join("present.exe");
    std::fs::write(&present, b"").expect("fixture binary");
    assert!(missing_binary_error(Some(&present)).is_none());
    assert!(
        missing_binary_error(None).is_none(),
        "no global binary is no verdict: the unset posture belongs to the rows, \
         not to a page error (FR-13)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reconcile must not drain a triple that a newer reading already replaced
/// (the stale-failure race: two persist tasks land out of order), and the
/// exact-match guard covers the `Option<bool>` shape dimension itself:
/// equal `(high, low)` with a mismatched
/// stamp shape drains nothing, in either direction. Falsifier: neutering the
/// `entry.measured_offload == offload` comparison fails the shape legs below.
#[test]
fn a_stale_persist_failure_never_drains_a_superseding_triple() {
    let dir = crate::testutil::temp_dir("measured-stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let (map, _) = crate::model::scan::parse_flat_yaml("engine: llama\nvram_gb: 2\n");
    let mut entry = ModelEntry::from_flat(&map, "fresh").expect("entry");
    entry.measured_gb = Some(6.0);
    entry.measured_min_gb = Some(4.0);
    entry.measured_offload = Some(false);
    let fresh_yaml = dir.join("fresh.yaml");
    entry.file = Some(fresh_yaml.clone());
    state.catalog = vec![entry];

    state.fail_persist(
        "fresh",
        super::MeasuredData {
            high: 5.0,
            low: 5.0,
            offload: Some(false),
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(fresh_yaml.clone()),
    );
    assert_eq!(
        state.catalog[0].measured_gb,
        Some(6.0),
        "the newer (possibly persisted) triple survives a stale failure"
    );
    state.fail_persist(
        "fresh",
        super::MeasuredData {
            high: 6.0,
            low: 4.0,
            offload: None,
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(fresh_yaml.clone()),
    );
    assert_eq!(
        state.catalog[0].measured_gb,
        Some(6.0),
        "a None-shaped (strata-style) failure must not drain a stamped \
             (llama-style) triple: equal pair values with the wrong stamp shape are \
             not the triple that failed"
    );
    state.fail_persist(
        "fresh",
        super::MeasuredData {
            high: 6.0,
            low: 4.0,
            offload: Some(false),
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(fresh_yaml.clone()),
    );
    assert_eq!(
        state.catalog[0].measured_gb, None,
        "the matching triple drains"
    );

    let (map2, _) = crate::model::scan::parse_flat_yaml("engine: strata\nvram_gb: 2\n");
    let mut paironly = ModelEntry::from_flat(&map2, "paironly").expect("entry");
    paironly.measured_gb = Some(6.0);
    paironly.measured_min_gb = Some(4.0);
    // measured_offload stays None: the shape a non-offload engine's persist carries.
    let paironly_yaml = dir.join("paironly.yaml");
    paironly.file = Some(paironly_yaml.clone());
    state.catalog.push(paironly);
    state.fail_persist(
        "paironly",
        super::MeasuredData {
            high: 6.0,
            low: 4.0,
            offload: Some(false),
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(paironly_yaml.clone()),
    );
    assert_eq!(
        state.catalog[1].measured_gb,
        Some(6.0),
        "the reverse leg: a Some-shaped (llama-style) failure must not drain a \
             pair-only (strata-style) triple"
    );
    state.fail_persist(
        "paironly",
        super::MeasuredData {
            high: 6.0,
            low: 4.0,
            offload: None,
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(paironly_yaml.clone()),
    );
    assert_eq!(
        state.catalog[1].measured_gb, None,
        "the shape-matching failure drains the pair"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// D of the `cpu_offload` scoping ruling: the measured PAIR mints for every
/// engine (witness truth is engine-agnostic), but the `measured_offload` stamp
/// is minted only for engines carrying the `cpu_offload` knob — in memory AND on disk: both
/// entries carry a real backing yaml, so the persist runs to completion and
/// the bytes are read back. Falsifier (disk leg): deleting the stamp push in
/// the persist edits makes this the only failing test — a llama yaml that
/// silently stops carrying `measured_offload` (killing the flip-void rule's
/// stamp source) cannot ship unnoticed; the mock leg then stays green (its
/// contract is pair-only). Also catches: dropping the `offload_live` gate in
/// `on_measured` (stamps the mock row) or tightening it past llama (unmints).
#[tokio::test]
async fn only_offload_engines_stamp_measured_offload() {
    let dir = crate::testutil::temp_dir("measured-stamp");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port: 18_992,
    };
    let entry = |name: &str, engine: &str| -> ModelEntry {
        let (map, _) = crate::model::scan::parse_flat_yaml(&format!(
            "engine: {engine}\ncpu_offload: true\nvram_gb: 2\n"
        ));
        ModelEntry::from_flat(&map, name).expect("entry")
    };
    let mut stampy = entry("stampy", "llama");
    // `on` is in the engine truth set: the stamp must follow it, not a
    // third reading of the token.
    stampy
        .raw
        .insert("cpu_offload".to_string(), "on".to_string());
    let mut nostamp = entry("nostamp", "mock");
    // Real backing files: the persist runs to completion (NOT the wedged-drive
    // sibling) so the stamped triple is pinned as written bytes, not just as
    // in-memory fields.
    let stampy_path = dir.join("stampy.yaml");
    std::fs::write(&stampy_path, "engine: llama\nvram_gb: 2\ncpu_offload: on\n").expect("seed");
    let nostamp_path = dir.join("nostamp.yaml");
    std::fs::write(
        &nostamp_path,
        "engine: mock\nvram_gb: 2\ncpu_offload: true\n",
    )
    .expect("seed");
    stampy.file = Some(stampy_path.clone());
    nostamp.file = Some(nostamp_path.clone());
    state.catalog = vec![stampy.clone(), nostamp.clone()];

    seat_spawn(&mut state, &stampy, &ticket, 18_992);
    state.witness("stampy", &ticket, 4.2);
    assert_eq!(
        (
            state.catalog[0].measured_gb,
            state.catalog[0].measured_min_gb
        ),
        (Some(4.2), Some(4.2)),
        "the pair mints on the offload engine"
    );
    assert_eq!(
        state.catalog[0].measured_offload,
        Some(true),
        "the stamp mints on the engine that reads the key"
    );
    let written = await_yaml_keys(
        &stampy_path,
        &[
            "measured_gb: 4.2",
            "measured_gb_min: 4.2",
            "measured_offload: true",
        ],
    )
    .await;
    assert!(
        written.contains("measured_offload: true"),
        "a genuine llama yaml must gain all three stamped keys on disk — if the \
             persist stops pushing the stamp, this is the only failing test: {written}"
    );

    seat_spawn(&mut state, &nostamp, &ticket, 18_992);
    state.witness("nostamp", &ticket, 4.3);
    assert_eq!(
        (
            state.catalog[1].measured_gb,
            state.catalog[1].measured_min_gb
        ),
        (Some(4.3), Some(4.3)),
        "witness truth is engine-agnostic: the pair still mints on mock"
    );
    assert_eq!(
        state.catalog[1].measured_offload, None,
        "no stamp for an engine whose adapter never reads cpu_offload"
    );
    let written =
        await_yaml_keys(&nostamp_path, &["measured_gb: 4.3", "measured_gb_min: 4.3"]).await;
    assert!(
        !written.contains("measured_offload"),
        "a non-offload yaml must not gain the meaningless stamp on disk: {written}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `cpu_offload` flip that starts this whole investigation:
/// the knob changes
/// in the yaml while the engine still serves under the old argv. The stamp must
/// keep describing the launch that ran — a stamp rewritten to agree with the new
/// bytes silently disarms the flip-void rule, which compares the range with those
/// same live bytes — and the accumulated range must survive the rescan that drops
/// the in-memory pair. Falsifiers: computing `offload` from `entry.raw` inside the
/// fold (the pre-fix shape) flips the stamp leg to `Some(true)`; seeding the fold
/// from the live entry's pair instead of the spawn's drops the low end to `6.0`;
/// each fails here and nowhere else in the suite.
#[tokio::test]
async fn a_mid_serve_offload_flip_stamps_the_launch_that_ran() {
    let dir = crate::testutil::temp_dir("fold-flip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("flip.yaml");
    std::fs::write(&yaml, "engine: llama\nvram_gb: 20\ncpu_offload: false\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "flip");
    let ticket = fold_ticket(19_001);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_001);

    state.witness("flip", &ticket, 5.0);
    await_yaml_keys(&yaml, &["measured_gb: 5.0", "measured_offload: false"]).await;

    // The operator flips the knob (one line, the measured keys stay) and the
    // catalog is rescanned underneath the serving engine.
    let flipped = std::fs::read_to_string(&yaml)
        .expect("read")
        .replace("cpu_offload: false", "cpu_offload: true");
    std::fs::write(&yaml, flipped).expect("flip");
    state.catalog = vec![rescan_entry(&yaml, "flip")];
    assert_eq!(
        state.catalog[0].measured_gb, None,
        "the rescan's void rule must have fired, or the leg below proves nothing"
    );

    state.witness("flip", &ticket, 6.0);
    let entry = &state.catalog[0];
    assert_eq!(
        entry.measured_offload,
        Some(false),
        "the stamp describes the spawn (`-ngl 99`), not the yaml that arrived after it"
    );
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(6.0), Some(5.0)),
        "the range keeps the history this model built, not a re-seed from the one live process"
    );
    let written = await_yaml_keys(
        &yaml,
        &[
            "measured_gb: 6.0",
            "measured_gb_min: 5.0",
            "measured_offload: false",
        ],
    )
    .await;
    assert!(
        written.contains("cpu_offload: true"),
        "the flip really is on disk, so the contradiction below is real: {written}"
    );
    // And the rule that was supposed to catch it still works: a stamp that cannot
    // match its knob voids the pair, instead of agreeing with it forever.
    let after = rescan_entry(&yaml, "flip");
    assert_eq!(
        (after.measured_gb, after.measured_min_gb),
        (None, None),
        "the flip-void rule is restored, not disarmed by the fold"
    );
    assert_eq!(after.measured_offload, Some(false));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The container's own reserved-footprint report books for an
/// `exact` row that declared nothing — the one class the GGUF fallback must never
/// guess for — fixes BOTH ends of the pair (`measured_gb_min` never understates the
/// declared KV), carries the launch's profile stamp, and outranks every later
/// witness reading: the reserved figure is static, so a noisy total-diff could only
/// corrupt it. Falsifiers: dropping the anchor's `EngineReport` guard lets the
/// witness clobber the range; stamping without the profile leaves the scan's
/// profile-void rule without an anchor to compare.
#[tokio::test]
async fn an_engine_report_books_an_exact_row_and_the_witness_never_clobbers_it() {
    let dir = crate::testutil::temp_dir("fold-engine-report");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("er.yaml");
    std::fs::write(&yaml, "engine: ninfer\ncost: exact\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "er");
    assert_eq!(
        spawned.cost_source(),
        crate::model::CostSource::Undeclared,
        "exact without its declared precision claim starts refused, never estimated"
    );
    let ticket = fold_ticket(19_010);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_010);

    state.engine_report("er", &ticket, 31.0);
    let entry = &state.catalog[0];
    assert_eq!(entry.measured_gb, Some(31.0));
    assert_eq!(
        entry.measured_min_gb,
        Some(31.0),
        "the reserved figure is static: both ends are the reservation"
    );
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::EngineReport)
    );
    assert_eq!(entry.measured_profile.as_deref(), Some("b"));
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Measured,
        "the engine's own reserved budget is a source: exact may book it"
    );

    state.witness("er", &ticket, 9.0);
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(31.0), Some(31.0)),
        "the total-diff cannot widen what the engine itself reported"
    );
    await_yaml_keys(
        &yaml,
        &[
            "measured_gb: 31.0",
            "measured_attribution: engine_report",
            "measured_profile: b",
        ],
    )
    .await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The per-slot carrier behind the API's memory-bar sum: the figure reaches
/// the snapshot ONLY through the spawn anchor of the very job that produced this
/// occupant — engine ninfer, container-backed, attribution `engine_report` — so
/// `/har/status` and the SSE push quote the identical number (review
/// 2026-10-31: threading the sum only through the SSE build made the annotation
/// dead on the poll path). Falsifier: loosening any one of the three anchor
/// gates (the engine check, `anchor.container`, the attribution filter — the
/// very filter with zero coverage before this pin) makes a total-diff free-memory
/// delta or a child-process figure reach the line the operator reads as
/// container-reserved.
#[tokio::test]
async fn only_an_engine_report_anchor_reaches_the_snapshot_slot() {
    let dir = crate::testutil::temp_dir("fold-slot-report");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::create_dir_all(dir.join("logs")).expect("logs dir");
    let yaml = dir.join("sb.yaml");
    std::fs::write(&yaml, "engine: ninfer\ncost: exact\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "sb");
    let ticket = fold_ticket(19_015);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_015);

    // Witness first: a container total-diff must NOT populate the slot carrier.
    state.on_measured(
        "sb",
        &ticket,
        super::Reading {
            total_diff: 12.0,
            own: None,
            total_gib: 72.0,
            used_gib: 13.0,
        },
    );
    assert_eq!(
        state.snapshot().slots[0].container_reported_gb,
        None,
        "a free-memory delta is not a container reservation"
    );

    // The engine's own record: the same fold that books is what the slot carries.
    state.engine_report("sb", &ticket, 31.0);
    assert_eq!(
        state.snapshot().slots[0].container_reported_gb,
        Some(31.0),
        "the engine_report fold is the ONLY route to the bar's reserved line"
    );

    // A twin under a non-ninfer engine, anchored like a report but with the
    // engine gate's answer changed: the engine check must keep it out.
    let twin = ModelEntry {
        engine: "llama".to_string(),
        ..spawned.clone()
    };
    let twin_ticket = fold_ticket(19_016);
    state.catalog = vec![twin.clone()];
    seat_spawn(&mut state, &twin, &twin_ticket, 19_016);
    state.engine_report("sb", &twin_ticket, 31.0);
    assert_eq!(
        state.snapshot().slots[0].container_reported_gb,
        None,
        "a child-process spawn never reserves container memory, whoever reported it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A report equal to the freshly-persisted stamp must still anchor THIS launch:
/// the persisted pair proves an earlier launch spoke, not this one. The fold's
/// row-idempotence used to return before the anchor promotion, so the live
/// container never displayed as self-reported (a card run polled
/// `container_reported_gb` for 150 s through a ninfer serve whose 43.8 GiB
/// report matched the stamp `ninfer-swift-fp8.yaml` already carried).
/// Falsifier: the pre-fix row-only idempotence guard in `fold`.
#[tokio::test]
async fn a_report_equal_to_the_persisted_stamp_still_promotes_the_anchor() {
    let dir = crate::testutil::temp_dir("fold-stamp-promote");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::create_dir_all(dir.join("logs")).expect("logs dir");
    let yaml = dir.join("sb.yaml");
    std::fs::write(
        &yaml,
        "engine: ninfer\ncost: exact\nmeasured_gb: 31.0\nmeasured_gb_min: 31.0\n\
             measured_attribution: engine_report\nmeasured_profile: b\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "sb");
    assert_eq!(
        spawned.measured_attribution,
        Some(crate::model::MeasuredAttribution::EngineReport),
        "the scanner kept the provenance stamp the live row carries"
    );
    let ticket = fold_ticket(19_017);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_017);
    assert_eq!(
        state.snapshot().slots[0].container_reported_gb,
        None,
        "the persisted stamp is display, not this launch's anchor"
    );

    state.engine_report("sb", &ticket, 31.0);
    assert_eq!(
        state.snapshot().slots[0].container_reported_gb,
        Some(31.0),
        "a fresh report matching the persisted stamp still anchors this launch"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A container that never yielded a parsable memory record
/// still gets measured — but the reading is stamped `total_diff`, and a
/// `total_diff` displays without ever booking. When the report does land (a later
/// startup), it supersedes the witness figure outright. Falsifier: minting the
/// witness stamp as `None` (the pre-report shape) books the 12.0 GiB total-diff as if
/// it were a footprint.
#[tokio::test]
async fn a_total_diff_fold_displays_but_never_books_until_the_report_lands() {
    let dir = crate::testutil::temp_dir("fold-total-diff");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("td.yaml");
    std::fs::write(&yaml, "engine: ninfer\ncost: exact\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "td");
    let ticket = fold_ticket(19_011);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_011);

    state.on_measured(
        "td",
        &ticket,
        super::Reading {
            total_diff: 12.0,
            own: None,
            total_gib: 24.0,
            used_gib: 13.0,
        },
    );
    let entry = &state.catalog[0];
    assert_eq!(
        entry.measured_gb,
        Some(12.0),
        "the page still shows the reading"
    );
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::TotalDiff)
    );
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Undeclared,
        "exact never books a total-diff: absence of a source is still absence"
    );

    state.engine_report("td", &ticket, 31.0);
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(31.0), Some(31.0)),
        "the engine's own figure replaces the noisy stand-in rather than widening it"
    );
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::EngineReport)
    );
    assert_eq!(entry.cost_source(), crate::model::CostSource::Measured);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fake engine metrics JSON on an ephemeral port, served at the path the
/// sampler's path map reads (`/metrics`; the 2026-11-04 capture proved
/// the status routes carry no footprint). The SE fake-server rule, same
/// posture as `serve_health` in `engine/mod.rs`.
async fn serve_status_json(body: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let port = listener.local_addr().expect("addr").port();
    let app =
        axum::Router::new().route("/metrics", axum::routing::get(move || async move { body }));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    port
}

/// Pin, kept honest by the metric parse: the fake metrics server answers with a body the
/// Basalt parse MUST refuse — `vram` is a scalar riding a foreign
/// wrapper, and the parse requires `engine.expert_cache_mib` from the
/// captured `/metrics` engine block (pinned in
/// `engine::basalt::tests::the_scalar_vram_body_stays_refused`) — so the
/// one-shot must NOT consume the sampler: the process-tree fallback has
/// no per-process bytes to sum here either, `report_measurement` returns
/// `None`, and the witness loop keeps standing in. A body the parse
/// ACCEPTS is pinned elsewhere (basalt fake-server chain); this pin
/// exists only for the fallback leg and must never be weakened.
#[tokio::test]
async fn an_unparsable_status_shape_falls_back_to_the_witness_loop() {
    let port = serve_status_json(r#"{"engine":{"info":{"vram":67.4}}}"#).await;
    let runtime =
        Arc::new(crate::process::ProcessRuntime::new()) as Arc<dyn crate::engine::EngineRuntime>;
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let witness = Arc::new(scripted_sample(Vec::new())) as Arc<dyn crate::budget::VramWitness>;
    let ctx = sampler_ctx(
        "stub",
        "basalt",
        SpawnTicket {
            generation: 1,
            kind: ResourceKind::ChildProcess,
            pid: 4242,
            port,
        },
    );
    assert_eq!(
        ActorState::report_measurement(&runtime, &tx, &ctx, &witness).await,
        None,
        "the parse refuses the scalar-vram JSON, and the unattributable witness \
             gives the fallback nothing: the sampler must stay alive for the witness loop"
    );
    assert!(
        rx.try_recv().is_err(),
        "a refused one-shot sends no MeasuredProcessReport"
    );
}

/// The engine gate and the refused-figure gate of the same one-shot: a
/// non-process engine is never touched, and figures
/// `measurable_figure` would drop (sub-quantum, larger than the card)
/// return `None` WITHOUT a message — the M1 doctrine that a consumed
/// one-shot silently skips the witness stand-in.
#[tokio::test]
async fn a_refused_measurement_never_consumes_the_sampler() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(StatusRuntime {
        status: None,
        children: Vec::new(),
    }) as Arc<dyn crate::engine::EngineRuntime>;
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port: 19_020,
    };
    // A llama row never gets a process-engine one-shot at all.
    let llama_witness =
        Arc::new(scripted_sample(vec![(4242, 30.0)])) as Arc<dyn crate::budget::VramWitness>;
    let llama_ctx = sampler_ctx("plain", "llama", ticket.clone());
    assert_eq!(
        ActorState::report_measurement(&runtime, &tx, &llama_ctx, &llama_witness).await,
        None,
        "the one-shot belongs to basalt/strata only"
    );
    // A sub-quantum tree sum: absence, never a fake number and never a
    // consumed sampler.
    let tiny = Arc::new(scripted_sample(vec![(4242, 0.02)])) as Arc<dyn crate::budget::VramWitness>;
    let strata_ctx = sampler_ctx("tiny", "strata", ticket.clone());
    assert_eq!(
        ActorState::report_measurement(&runtime, &tx, &strata_ctx, &tiny).await,
        None,
        "a sub-quantum figure is refused here so the witness loop keeps standing in"
    );
    // A figure larger than the card: same fall-through.
    let giant =
        Arc::new(scripted_sample(vec![(4242, 80.0)])) as Arc<dyn crate::budget::VramWitness>;
    let giant_ctx = sampler_ctx("giant", "strata", ticket.clone());
    assert_eq!(
        ActorState::report_measurement(&runtime, &tx, &giant_ctx, &giant).await,
        None,
        "a footprint beyond the card is nonsense, not a booking"
    );
    assert!(
        rx.try_recv().is_err(),
        "no refused figure may reach the actor as a MeasuredProcessReport"
    );
}

/// When the status endpoint stays silent but the driver attributes concrete
/// bytes, the one-shot sums the engine pid plus its DIRECT children only
/// (the 9999 twin belongs to nobody here) and sends the `ProcessTreeNvml`
/// measurement ONCE with the live card total the witness just read
/// (doctrine 4).
#[tokio::test]
async fn the_tree_fallback_sums_the_engine_and_its_direct_children() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(StatusRuntime {
        status: None,
        children: vec![4243],
    }) as Arc<dyn crate::engine::EngineRuntime>;
    let witness = Arc::new(scripted_sample(vec![
        (4242, 20.0),
        (4243, 10.0),
        (9999, 5.0),
    ])) as Arc<dyn crate::budget::VramWitness>;
    let ctx = sampler_ctx(
        "tree",
        "strata",
        SpawnTicket {
            generation: 3,
            kind: ResourceKind::ChildProcess,
            pid: 4242,
            port: 19_021,
        },
    );
    assert_eq!(
        ActorState::report_measurement(&runtime, &tx, &ctx, &witness).await,
        // The elastic booking: the landing now reports the booked figure it seeded the
        // actor with (the same rounded value the fold will land), so the
        // elastic refresh can judge its hysteresis against the truth — the
        // tree sum of the engine plus its direct children, 20 + 10.
        Some(30.0)
    );
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the measurement reaches the actor")
        .expect("channel open");
    let ActorMsg::MeasuredProcessReport {
        model,
        ticket,
        measurement,
        total_gib,
    } = msg
    else {
        panic!("expected the process-measurement message");
    };
    assert_eq!(model, "tree");
    assert_eq!(ticket.generation, 3);
    assert_eq!(
        measurement.gpu_gib, 30.0,
        "20.0 + 10.0, the foreign twin excluded"
    );
    assert_eq!(
        measurement.source,
        crate::engine::MeasurementSource::ProcessTreeNvml
    );
    assert_eq!(
        measurement.confidence,
        crate::engine::MeasurementConfidence::Low,
        "the tree sum is a footprint read from outside the engine: Low confidence"
    );
    assert_eq!(
        total_gib, 72.0,
        "the message carries the live witness card total"
    );
}

/// A process engine's
/// `EngineStatus` figure books with `EngineReport`-class trust — it REPLACES
/// the spawn's persisted range (no averaging: a status figure is static for
/// the launch) and, as max truth, overrides the declared `vram_gb` it
/// exceeds. After it lands, witness readings can no longer fold into the
/// range (the guard `engine_report` always had, extended). Falsifiers: the
/// old single-variant witness guard (the 9.0 total-diff would clobber the
/// 67.0), and `engine_report_outranks_declared` without `EngineStatus`
/// (the declared 55 would keep booking).
#[tokio::test]
async fn an_engine_status_measurement_books_over_the_declared_value() {
    let dir = crate::testutil::temp_dir("fold-engine-status");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("es.yaml");
    std::fs::write(
        &yaml,
        "engine: strata\ncost: fixed\nvram_gb: 55\nmeasured_gb: 50.0\nmeasured_gb_min: 40.0\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "es");
    let ticket = fold_ticket(19_022);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_022);

    state.process_measurement(
        "es",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 67.0,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(67.0), Some(67.0)),
        "both ends are the footprint: the persisted 40.0 low end is replaced, not averaged"
    );
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::EngineStatus)
    );
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Measured,
        "the engine's own status figure above the declared floor books instead of it"
    );
    assert!(
        entry.measured_profile.is_none(),
        "no ninfer profile on a strata row"
    );

    state.witness("es", &ticket, 9.0);
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(67.0), Some(67.0)),
        "a total-diff cannot widen what the engine itself reported"
    );
    let written = await_yaml_keys(
        &yaml,
        &[
            "measured_gb: 67.0",
            "measured_gb_min: 67.0",
            "measured_attribution: engine_status",
        ],
    )
    .await;
    assert!(
        !written.contains("measured_profile"),
        "the status fold mints no profile stamp: {written}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `ProcessTreeNvml` source folds as the existing `PerProcess`
/// footprint — persisted under the old token (no new yaml
/// keys), replacing the spawn's range outright, and NOT overriding the
/// declared value (only the engine-spoken reports get max truth).
#[tokio::test]
async fn a_process_tree_measurement_lands_as_per_process_and_replaces_the_range() {
    let dir = crate::testutil::temp_dir("fold-tree-sum");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("ts.yaml");
    std::fs::write(
        &yaml,
        "engine: strata\nvram_gb: 55\nmeasured_gb: 50.0\nmeasured_gb_min: 40.0\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "ts");
    let ticket = fold_ticket(19_023);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_023);

    state.process_measurement(
        "ts",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 30.0,
            ram_gib: None,
            source: crate::engine::MeasurementSource::ProcessTreeNvml,
            confidence: crate::engine::MeasurementConfidence::Low,
        },
    );
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(30.0), Some(30.0)),
        "a tree sum is this launch's footprint: it fixes both ends instead of averaging"
    );
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::PerProcess),
        "the tree sum persists as the existing per_process provenance"
    );
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Declared,
        "below the declared floor the operator's figure stays the booking"
    );
    await_yaml_keys(
        &yaml,
        &["measured_gb: 30.0", "measured_attribution: per_process"],
    )
    .await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The actor-side twin of the sampler's M1 gate: a figure
/// `measurable_figure` refuses (negative, beyond the card) and a reading
/// that lost its uncontested window are both visible drops that book
/// nothing — the one-shot does not retry, so silence here would be a lost
/// measurement — doctrine.
#[tokio::test]
async fn a_nonsense_process_measurement_books_nothing_and_names_itself() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fold-nonsense-meas");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("nm.yaml");
    std::fs::write(&yaml, "engine: strata\nvram_gb: 20\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "nm");
    let ticket = fold_ticket(19_024);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_024);

    let measurement = |gpu_gib: f64| crate::engine::EngineMeasurement {
        gpu_gib,
        ram_gib: None,
        source: crate::engine::MeasurementSource::EngineStatus,
        confidence: crate::engine::MeasurementConfidence::High,
    };
    state.process_measurement("nm", &ticket, measurement(-5.0));
    state.process_measurement("nm", &ticket, measurement(80.0));
    // A reading for a ticket that is not the live occupant's: dropped, and named.
    state.process_measurement("nm", &fold_ticket(19_099), measurement(12.0));
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_attribution),
        (None, None),
        "nonsense never books and never persists"
    );

    let warns = capture.lock().expect("capture lock");
    assert!(
        warns
            .iter()
            .any(|(message, _)| message.contains("process measurement refused")),
        "the refused figures must be named once: {warns:?}"
    );
    assert!(
        warns
            .iter()
            .any(|(message, _)| message.contains("process measurement dropped")),
        "the race-loss drop must be named once: {warns:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A total-diff read on a foreign-dominated card mints nothing.
/// The card holds far more than this engine plus the other occupants can
/// account for, so the delta is someone else's story — absence, labelled by
/// the warn, never a 23.8–59.3 stand-in. Falsifier: deleting the
/// `ForeignDominated` guard persists the 12.0 GiB reading to the yaml.
#[tokio::test]
async fn a_foreign_dominated_card_mints_no_total_diff() {
    let dir = crate::testutil::temp_dir("fold-foreign-dominated");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("fd.yaml");
    std::fs::write(&yaml, "engine: ninfer\nvram_gb: 20\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "fd");
    let ticket = fold_ticket(19_021);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_021);
    // The admit baseline: the card was near-empty when this load was admitted,
    // so everything it holds beyond this engine at sample time is memory that
    // MOVED during the window — 26 GiB of foreign story against a 12 GiB delta.
    if let Some((_, job)) = state
        .jobs
        .iter_mut()
        .find(|(_, j)| j.spawned.as_ref() == Some(&ticket))
    {
        job.observed_at_admit = Some(50.0);
    }

    state.on_measured(
        "fd",
        &ticket,
        super::Reading {
            total_diff: 12.0,
            own: None,
            total_gib: 72.0,
            used_gib: 60.0,
        },
    );
    assert_eq!(
        state.catalog[0].measured_gb, spawned.measured_gb,
        "a foreign-dominated reading must not mint a range"
    );
    assert_eq!(state.catalog[0].measured_attribution, None);
    let written = std::fs::read_to_string(&yaml).expect("file readable");
    assert!(
        !written.contains("measured_gb"),
        "nothing reaches the yaml: {written}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A detached container's ticket pid is the dead `wsl.exe` client, so a
/// container spawn may NEVER claim `per_process` — even a witness reading that
/// carries concrete bytes for that pid is a lie waiting to be stamped. The
/// falsifier is minting the stamp from `own` alone: the container row would book
/// 12.0 GiB as its own from a number the card never attributed to the engine.
#[tokio::test]
async fn a_container_reading_may_never_claim_per_process_attribution() {
    let dir = crate::testutil::temp_dir("fold-container-own");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::create_dir_all(dir.join("logs")).expect("logs dir");
    let yaml = dir.join("cn.yaml");
    std::fs::write(&yaml, "engine: ninfer\nvram_gb: 20\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "cn");
    let ticket = fold_ticket(19_012);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_012);

    state.witness("cn", &ticket, 12.0);
    let entry = &state.catalog[0];
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::TotalDiff),
        "a container's pid is the dead client: concrete bytes for it attribute nothing"
    );
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Declared,
        "the declared number still books; the total-diff never competes"
    );
    // The persist must carry the honest stamp: a file left with a bare
    // `measured_gb` would grandfather the stand-in back into booking on the next
    // scan, which is exactly the lie this pin exists to keep out.
    let written = await_yaml_keys(&yaml, &["measured_attribution: total_diff"]).await;
    assert!(
        !written.contains("measured_profile"),
        "a witness range carries no profile stamp: {written}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The counterpart leg: a child-process engine whose card DID report concrete
/// bytes for the engine's own pid earns `per_process` — a provenance that books,
/// written to the file alongside the pair. Falsifier: stamping every witnessed
/// fold `total_diff` breaks the booking leg here while the container leg above
/// stays green.
#[tokio::test]
async fn a_per_process_fold_books_and_stamps_the_file() {
    let dir = crate::testutil::temp_dir("fold-per-process");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::create_dir_all(dir.join("logs")).expect("logs dir");
    let yaml = dir.join("pp.yaml");
    std::fs::write(&yaml, "engine: llama\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "pp");
    let ticket = fold_ticket(19_013);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_013);

    state.witness("pp", &ticket, 7.4);
    let entry = &state.catalog[0];
    assert_eq!(
        entry.measured_attribution,
        Some(crate::model::MeasuredAttribution::PerProcess)
    );
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Measured,
        "a driver-attributed footprint is a real booking source for a fixed row"
    );
    await_yaml_keys(
        &yaml,
        &["measured_gb: 7.4", "measured_attribution: per_process"],
    )
    .await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fork C extended to the provenance stamps: a range the disk refused must not
/// keep the stamp that would book it. With the grandfathering rule (an UNSTAMPED
/// measured pair books), a drain that cleared only the pair and left
/// `measured_attribution` behind would resurrect the refused figure as bookable on
/// the next rescan — this pin is the falsifier for exactly that half-drain.
#[test]
fn a_refused_persist_drains_the_provenance_stamps_with_the_pair() {
    let dir = crate::testutil::temp_dir("fold-drain-stamps");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("ds.yaml");
    std::fs::write(&yaml, "engine: ninfer\nvram_gb: 20\n").expect("seed");
    let mut state = fold_state(&dir);
    let mut entry = rescan_entry(&yaml, "ds");
    entry.measured_gb = Some(30.0);
    entry.measured_min_gb = Some(30.0);
    entry.measured_attribution = Some(crate::model::MeasuredAttribution::EngineReport);
    entry.measured_profile = Some("b".to_string());
    state.catalog = vec![entry];

    state.fail_persist(
        "ds",
        super::MeasuredData {
            high: 30.0,
            low: 30.0,
            offload: None,
            attribution: Some(crate::model::MeasuredAttribution::EngineReport),
            profile: Some("b".to_string()),
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(yaml.clone()),
    );
    let entry = &state.catalog[0];
    assert_eq!(entry.measured_gb, None);
    assert_eq!(
        entry.measured_attribution, None,
        "an orphaned provenance stamp would grandfather the refused pair back to bookable"
    );
    assert_eq!(entry.measured_profile, None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sampler's fallback shape: with no container report seeded, the
/// one-shot finds nothing, the witness loop still runs, and what lands is stamped
/// `total_diff` — displayed, never booked. A regression that swallows the report
/// path entirely looks exactly like this pin from the outside, which is why the
/// report-present legs live in `tests/r9_container_report.rs` against the faked
/// runtime seam rather than being asserted only here.
#[tokio::test]
async fn a_container_without_a_report_falls_back_to_the_witness_loop() {
    let dir = crate::testutil::temp_dir("fold-report-fallback");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("fb.yaml");
    std::fs::write(&yaml, "engine: ninfer\nvram_gb: 20\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "fb");
    let ticket = fold_ticket(19_014);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_014);

    state.on_measured(
        "fb",
        &ticket,
        super::Reading {
            total_diff: 18.0,
            own: None,
            total_gib: 24.0,
            used_gib: 19.0,
        },
    );
    assert_eq!(
        state.catalog[0].measured_attribution,
        Some(crate::model::MeasuredAttribution::TotalDiff),
        "the stand-in keeps its honest provenance"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The fold half: two rows answering to one identity. The reading
/// belongs to the file the engine was spawned from, and the reconcile of a refused
/// persist may only drain that same row — never the twin whose bytes the refused
/// write never touched. The catalog order puts the twin FIRST so any name-keyed
/// target (the pre-fix `find(|m| m.name == model)`, in the fold and in the drain)
/// picks the wrong file and both legs below fail.
#[tokio::test]
async fn a_twin_name_folds_only_into_the_spawn_era_file() {
    let dir = crate::testutil::temp_dir("fold-twin");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mine = dir.join("twin.yaml");
    let twin = dir.join("twin .yaml");
    std::fs::write(&mine, "engine: llama\nvram_gb: 20\n").expect("seed mine");
    std::fs::write(&twin, "engine: llama\nvram_gb: 20\n").expect("seed twin");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&mine, "twin");
    let ticket = fold_ticket(19_002);
    // Catalog order deliberately the other way round: the twin owns the first
    // slot, so a name lookup answers with the row this engine never came from.
    state.catalog = vec![rescan_entry(&twin, "twin"), spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_002);

    state.witness("twin", &ticket, 4.4);
    let folded = await_yaml_keys(&mine, &["measured_gb: 4.4"]).await;
    assert!(
        folded.contains("measured_gb_min: 4.4"),
        "the spawn-era file gains the pair: {folded}"
    );
    assert_eq!(
        (
            state.catalog[0].measured_gb,
            state.catalog[0].measured_min_gb,
            state.catalog[0].measured_offload
        ),
        (None, None, None),
        "the twin row books nothing for a launch it did not describe"
    );
    let untouched = std::fs::read_to_string(&twin).expect("read twin");
    assert!(
        !untouched.contains("measured_gb"),
        "the twin's bytes are not the resident's measurement sheet: {untouched}"
    );

    // The refused-persist reconcile is held to the same target: equal values on
    // both rows may not make the twin drainable. Both rows wear exactly what the
    // resident's own fold wrote — only the row whose FILE the refused write
    // targeted may drain.
    for index in 0..2 {
        state.catalog[index].measured_gb = Some(4.4);
        state.catalog[index].measured_min_gb = Some(4.4);
        state.catalog[index].measured_offload = Some(false);
        state.catalog[index].measured_attribution =
            Some(crate::model::MeasuredAttribution::PerProcess);
    }
    state.fail_persist(
        "twin",
        super::MeasuredData {
            high: 4.4,
            low: 4.4,
            offload: Some(false),
            attribution: Some(crate::model::MeasuredAttribution::PerProcess),
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(mine.clone()),
    );
    assert_eq!(
        state.catalog[1].measured_gb, None,
        "the row the refused write targeted drains"
    );
    assert_eq!(
        state.catalog[0].measured_gb,
        Some(4.4),
        "the same-name twin keeps the values the refused write never touched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The row that owned a serving engine vanishes mid-serve (renamed away, turned
/// into a problem line, given an engine with no adapter). Nothing may be minted or
/// extended without it, and the sampler that produced the reading is retired —
/// left alone it walks the witness every `measured_sample_s` for a day with
/// nowhere to put the answer, and says nothing. Falsifiers: dropping the detach
/// `return` (the fold would carry on against whatever row the name still reaches)
/// leaves the sampler armed and the reading booked; dropping the `anchor.file`
/// guard writes a triple to no file at all and clears nothing.
#[tokio::test]
async fn a_vanished_row_fails_the_fold_closed_and_retires_the_sampler() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fold-vanished");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("gone.yaml");
    std::fs::write(&yaml, "engine: llama\nvram_gb: 20\n").expect("seed");
    let mut state = fold_state(&dir);
    let spawned = rescan_entry(&yaml, "gone");
    let ticket = fold_ticket(19_003);
    state.catalog = vec![spawned.clone()];
    seat_spawn(&mut state, &spawned, &ticket, 19_003);
    let sampler = tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
    state.slots[0]
        .occupant
        .as_mut()
        .expect("seated occupant")
        .measure = Some(sampler);

    // The operator turns the file into a problem line; Fork C never publishes it,
    // so the rescan leaves the serving engine with no row at all.
    std::fs::write(&yaml, "engine: llama\nbad key: x\n").expect("break the row");
    state.catalog = Vec::new();

    state.witness("gone", &ticket, 9.0);
    assert!(
        state.catalog.is_empty(),
        "no row may be booked, created or extended for a vanished launch"
    );
    assert!(
        state.slots[0]
            .occupant
            .as_ref()
            .and_then(|o| o.measure.as_ref())
            .is_none(),
        "the orphaned sampler is retired, not left polling the card"
    );
    let text = std::fs::read_to_string(&yaml).expect("read back");
    assert!(
        !text.contains("measured_gb"),
        "nothing is persisted for the reading that had no row: {text}"
    );
    let warns = capture.lock().expect("capture lock");
    assert!(
        warns.iter().any(|(message, fields)| {
            message.contains("measured reading has no catalog row")
                && fields.get("model").is_some_and(|m| m.contains("gone"))
                && fields.get("file").is_some_and(|f| f.contains("gone.yaml"))
        }),
        "the silence is the bug this leg pins: {warns:?}"
    );

    // Second leg: a spawn with no anchor at all (no `begin_spawn` ever ran for it)
    // folds nothing either — absence, never a number with no provenance.
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut bare = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    bare.catalog = vec![spawned];
    let unanchored = SpawnTicket {
        generation: 8,
        kind: ResourceKind::ChildProcess,
        pid: 4243,
        port: 19_004,
    };
    bare.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 19_004,
        occupant: Some(Occupant {
            model: "gone".to_string(),
            need: Gibibytes(2.0),
            ram_need: None,
            port: 19_004,
            ticket: unanchored.clone(),
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: None,
            stale_config: false,
            serve_id: None,
            measure: None,
            live_measured_gb: None,
        }),
        job: None,
    }];
    bare.witness("gone", &unanchored, 9.0);
    assert_eq!(
        bare.catalog[0].measured_gb, None,
        "an unanchored spawn has no stamp to mint and no file to write"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
