use super::*;

/// The yaml fingerprint alone carries the `stale_config` verdict; the dormant
/// profile leg is retired: an unchanged row never
/// accuses, and an edit to the resident's own row ambers on the refresh
/// turn with no other capture anywhere — the same amber, the same
/// unload + reload gesture. Falsifier: dropping the yaml leg from
/// `refresh_stale_markers` (amber never fires). The no-capture posture is
/// pinned separately by the L3 leg, which has no capture at all.
#[test]
fn the_yaml_fingerprint_alone_ambers_the_resident_on_a_changed_row() {
    let (mut state, yaml, _capture) = resident_on_a_file();
    state.refresh_stale_markers(&gather_drift_stats(std::slice::from_ref(&yaml)));
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(false),
        "an unchanged fingerprint must not accuse"
    );
    std::fs::write(&yaml, "engine: mock\n").expect("edit row");
    state.refresh_stale_markers(&gather_drift_stats(std::slice::from_ref(&yaml)));
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(true),
        "a changed yaml fingerprint ambers on its own"
    );
}

/// Reload leg: an edit only ambers when a rescan
/// lands — between the edit and the rescan the status path must not notice
/// (one-stat-per-event: the snapshot never stats). Falsifier: removing
/// `refresh_stale_markers` from `apply_catalog` fails the amber assert; moving
/// the verdict computation into the snapshot (or any status read) fails the
/// pre-rescan assert.
#[test]
fn a_resident_ambers_on_the_reload_that_finds_newer_bytes_than_its_launch() {
    let (mut state, yaml, _) = resident_on_a_file();
    let stamp = state.snapshot().slots[0]
        .launched_at
        .clone()
        .expect("a resident carries its wall-clock launch");
    assert_eq!(state.snapshot().slots[0].stale_config, Some(false));
    let original = std::fs::read_to_string(&yaml).expect("read");
    std::fs::write(&yaml, format!("{original}# operator note\n")).expect("operator edit");
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(false),
        "an edit without a rescan must not move the verdict from a status read: \
             the snapshot never stats"
    );
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].stale_config,
        Some(true),
        "newer bytes than the spawn capture must amber on the reload turn"
    );
    assert_eq!(
        after.slots[0].launched_at.as_deref(),
        Some(stamp.as_str()),
        "the launch stamp rides the install, not the rescan"
    );
}

/// Interlock (4), stated plainly: the comparison rides the SPAWN capture
/// (mtime + size), so re-saving byte-identical content does NOT clear the
/// verdict — the clock moved. The apply gesture is unload + reload, and a
/// respawned engine re-captures cleanly. Falsifier: a verdict that compares
/// content, or that clears on size-equality, takes the final assert from true
/// to false here (the size/mtime probes prove the leg is real either way).
#[test]
fn restoring_identical_bytes_does_not_clear_the_stale_verdict() {
    let (mut state, yaml, capture) = resident_on_a_file();
    let original = std::fs::read(&yaml).expect("read");
    std::fs::write(
        &yaml,
        // The `cpu_offload` flag key (elastic booking): a PURE cost edit is now
        // re-booked without a reload, so an amber pin must edit a flag —
        // the launch config the running engine never read — to earn the
        // verdict this pin then defends against a byte-identical restore.
        "engine: mock\ncost: fixed\nmeasured_gb: 444.0\nmeasured_gb_min: 8.0\ncpu_offload: true\n",
    )
    .expect("diverge (size moves: no dependence on the clock tick)");
    rescan(&mut state, &yaml, "m");
    assert_eq!(state.snapshot().slots[0].stale_config, Some(true));
    std::fs::write(&yaml, &original).expect("restore the identical bytes");
    // A same-tick rewrite can leave the recorded mtime unmoved (Windows clock
    // granularity), so force the clock forward: the restore is byte-identical,
    // the timestamp factually newer — the state a re-save really produces.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&yaml)
        .expect("open for the clock bump");
    file.set_times(
        std::fs::FileTimes::new().set_modified(
            capture
                .mtime
                .checked_add(Duration::from_millis(60_000))
                .expect("mtime in range"),
        ),
    )
    .expect("force the mtime forward");
    drop(file);
    let now = crate::model::fingerprint(&yaml).expect("stat");
    assert_eq!(
        now.size, capture.size,
        "byte-identical restore: the SIZE cannot tell it apart"
    );
    assert_ne!(
        now.mtime, capture.mtime,
        "and the clock moved (pre {:?} vs post {:?}) — this is the leg size-only would miss",
        capture.mtime, now.mtime
    );
    rescan(&mut state, &yaml, "m");
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(true),
        "byte-identical content restored later is STILL stale: honest conservatism, \
             the operator applies with unload + reload"
    );
}

/// Interlock (1): stale is INFO, not policy. An amber resident keeps its
/// measured pair, its cost basis and its slot exactly as a clean one does —
/// the marker never touches what admission books or who holds the card.
/// Falsifier: gating `cost_source`/measured validity (or the slot) on
/// `stale_config` fails this while the verdict pins stay green.
#[test]
fn a_stale_resident_keeps_serving_its_measured_range() {
    let (mut state, yaml, _) = resident_on_a_file();
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 10.0\nmeasured_gb_min: 8.0\n# operator note\n",
    )
    .expect("operator edit that keeps the measured keys intact");
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(after.slots[0].stale_config, Some(true), "amber first");
    let entry = state
        .catalog
        .iter()
        .find(|m| m.name == "m")
        .expect("resident's row");
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(10.0), Some(8.0)),
        "the measured pair is witness truth: an amber marker does not void it"
    );
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Measured,
        "and admission still books from it — stale never demotes the cost basis"
    );
    let occupant = after.slots[0].clone();
    assert_eq!(occupant.model.as_deref(), Some("m"));
    assert_eq!(occupant.port, Some(18_431), "the resident keeps its slot");
}

/// Interlock (5), actor side: a harness-owned write (the measured fold, the API
/// flag edit) re-baselines the spawn capture — never through the verdict, never
/// over a true one. Falsifiers: no-op-ing `on_model_file_written` makes the
/// unchanged rescan after the write amber (L1 fails); re-baselining regardless
/// of the verdict erases the operator's earned amber (L2's final assert fails).
#[test]
fn the_harness_own_write_rebaselines_a_clean_resident_captured_spawn_side() {
    // L1: clean resident + harness write + carried fingerprint -> the verdict
    // stays false AND survives the next unchanged rescan (the capture moved).
    let (mut state, yaml, _) = resident_on_a_file();
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 12.0\nmeasured_gb_min: 8.0\n",
    )
    .expect("fold write");
    let written = crate::model::fingerprint(&yaml).expect("stat");
    state.on_model_file_written("m", &yaml, written);
    rescan(&mut state, &yaml, "m");
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(false),
        "the harness's own write never ambers its own model"
    );

    // L2: an already-true verdict is never erased: neither by the message nor
    // by the unchanged rescan that follows it (which would go false had the
    // capture been moved under an earned amber).
    std::fs::write(
        &yaml,
        // The `cpu_offload` flag key (elastic booking): a pure cost edit is now
        // re-booked without a reload; this leg needs a real amber, earned
        // by a config move the running engine never read.
        "engine: mock\ncost: fixed\nmeasured_gb: 120.0\nmeasured_gb_min: 8.0\ncpu_offload: true\n",
    )
    .expect("operator edit (size moves)");
    rescan(&mut state, &yaml, "m");
    assert_eq!(state.snapshot().slots[0].stale_config, Some(true));
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 120.0\nmeasured_gb_min: 8.0\ncpu_offload: true\n",
    )
    .expect("fold write over an amber resident");
    let written = crate::model::fingerprint(&yaml).expect("stat");
    state.on_model_file_written("m", &yaml, written);
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(true),
        "a harness write must not clear a verdict the operator's edit earned"
    );
    rescan(&mut state, &yaml, "m");
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(true),
        "and must not re-baseline the capture under it: the unchanged rescan would \
             go false if it had"
    );

    // L3: a resident with no capture stays unaccused through the same message.
    if let Some(occupant) = state.slots[0].occupant.as_mut() {
        occupant.spawn_config = None;
        occupant.stale_config = false;
    }
    state.on_model_file_written("m", &yaml, written);
    rescan(&mut state, &yaml, "m");
    assert_eq!(
        state.snapshot().slots[0].stale_config,
        Some(false),
        "absence of a capture is absence of evidence, never an accusation"
    );
}

// ---------------------------------------------------------------------
// The booking policy that removes
// the manual-reload trap. Two seams: a process measurement
// that folds re-books its slot occupant, and a rescan that moved ONLY
// cost figures re-books (or invalidates) the resident without a reload.
// ---------------------------------------------------------------------

/// The `on_process_measurement` seam: the fold moves the row's max
/// truth, and the slot booking must follow in the same turn — otherwise
/// the release credit and the no-release floor guard keep billing the
/// paper figure (the Strata 55 / 70.4 incident). Falsifiers: dropping the
/// `rebook_after_measurement` call (the first assert stays at the paper
/// 55), and booking the raw measurement instead of the ONE decision
/// (the second fold to 30.0 must floor back UP to the declared 55, not
/// book 30).
#[tokio::test]
async fn a_folded_measurement_rebooks_the_slots_resident_to_the_max_truth() {
    let dir = crate::testutil::temp_dir("fold-rebook");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("rb.yaml");
    std::fs::write(&yaml, "engine: strata\ncost: fixed\nvram_gb: 55\n").expect("seed");
    let mut state = fold_state(&dir);
    let resident = rescan_entry(&yaml, "rb");
    let ticket = fold_ticket(19_040);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 19_040);
    // The incident's shape: the booking rode the paper row.
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(55.0);

    state.process_measurement(
        "rb",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 67.8,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(67.8),
        "the engine's own word above the declared floor re-books the slot"
    );
    // The two persist writes race for the disk: land the first before the
    // second fold, so the final file provably carries the later reading.
    await_yaml_keys(&yaml, &["measured_gb: 67.8"]).await;

    state.process_measurement(
        "rb",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 30.0,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(55.0),
        "the re-book is the ONE decision, not the raw figure: a footprint under \
             the declared floor floors back at the operator's number"
    );
    await_yaml_keys(&yaml, &["measured_gb: 30.0"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scripted status answer for the elastic-refresh pin: one body per poll
/// (`None` = silence), and silence forever once the sequence is spent.
struct SequencedStatusRuntime {
    answers: std::sync::Mutex<std::collections::VecDeque<Option<serde_json::Value>>>,
    children: Vec<u32>,
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for SequencedStatusRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the elastic refresh pin must never load");
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        panic!("the elastic refresh pin must never load");
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        panic!("the elastic refresh pin must never load");
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the elastic refresh pin must never load");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the elastic refresh pin must never load");
    }
    async fn engine_status_json(&self, _port: u16, _path: &str) -> Option<serde_json::Value> {
        let mut answers = self.answers.lock().expect("lock");
        answers.pop_front().flatten()
    }
    async fn descendant_pids(&self, _pid: u32) -> Vec<u32> {
        self.children.clone()
    }
}

/// A Strata-shaped `/metrics` engine block booking the component sum
/// `expert_cache_mib + conversation_cache_mib + arena_mib`
/// (`measurement_from_metrics`; the conversation and arena components
/// are zeroed so the fixture figure follows the expert cache alone
/// (the captured body's own `arena_mib` is zero too).
fn strata_vram_status(expert_cache_mib: u64) -> serde_json::Value {
    serde_json::json!({
        "engine": {
            "expert_cache_mib": expert_cache_mib,
            "conversation_cache_mib": 0,
            "arena_mib": 0,
        }
    })
}

/// The elastic refresh (`ActorState::elastic_refresh`), the asymmetric
/// band: starting booked at 62.0 on the scripted 72 GiB card, the polls
/// answer equal (62.0 — nothing moves), rise (62.5 — re-books at once,
/// under-booking is over-commitment), small fall (62.0 — inside the
/// 1 GiB band, the booking stays), big fall (60.0 — 2.5 GiB past the
/// band, the credit moves down). After the sequence, silence: absence is
/// never a guess, and nothing further ever books. Falsifiers: a symmetric
/// band (the rise skipped), no band (the small fall lands), and consuming
/// the sampler on the first fold (no message after tick one).
#[tokio::test]
async fn the_elastic_refresh_rebooks_rises_at_once_and_falls_only_past_the_band() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(SequencedStatusRuntime {
        answers: std::sync::Mutex::new(
            vec![
                Some(strata_vram_status(63_488)), // 62.0: equal — silence
                Some(strata_vram_status(64_000)), // 62.5: rise — re-books
                Some(strata_vram_status(63_488)), // 62.0: 0.5 fall — band
                Some(strata_vram_status(61_440)), // 60.0: 2.5 fall — lands
            ]
            .into(),
        ),
        children: Vec::new(),
    }) as Arc<dyn crate::engine::EngineRuntime>;
    let witness = Arc::new(scripted_sample(Vec::new())) as Arc<dyn crate::budget::VramWitness>;
    let ctx = sampler_ctx("elastic", "strata", fold_ticket(19_041));
    let sampler = tokio::spawn(async move {
        ActorState::elastic_refresh(
            &runtime,
            &tx,
            &ctx,
            &witness,
            62.0,
            Duration::from_millis(40),
        )
        .await;
    });
    async fn next_figure(rx: &mut mpsc::UnboundedReceiver<ActorMsg>) -> f64 {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the expected figure reaches the actor")
            .expect("actor channel open");
        let ActorMsg::MeasuredProcessReport { measurement, .. } = msg else {
            panic!("expected a process-measurement re-book");
        };
        measurement.gpu_gib
    }
    let rise = next_figure(&mut rx).await;
    assert!(
        (rise - 62.5).abs() < 1e-6,
        "the rise re-books at once, band only ever guards down: {rise}"
    );
    let fall = next_figure(&mut rx).await;
    assert!(
        (fall - 60.0).abs() < 1e-6,
        "the small fall stays inside the band; only the 2.5 GiB fall lands: {fall}"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .is_err(),
        "after the sequence, silence re-books nothing (and the loop keeps polling)"
    );
    sampler.abort();
}

/// Review F-EM-403: a resident the TREE landed (the status endpoint silent
/// at readiness — the 2026-11-04 evidence line records exactly this Strata
/// serve lag) stays open to the engine's own word: the refresh loop polls
/// the endpoint every tick regardless of what landed first, and when it
/// answers, that answer is the truth — above the booked tree figure it
/// re-books at once. The landing message stays `ProcessTreeNvml`; the
/// upgrade carries `EngineStatus`. Falsifier: ending the sampler after a
/// tree landing (the upgrade never arrives).
#[tokio::test]
async fn a_tree_landed_resident_upgrades_to_the_engines_own_status_word() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(SequencedStatusRuntime {
        answers: std::sync::Mutex::new(
            // pop 1: the landing — silent, so the tree sum books;
            // pop 2: the first refresh tick — the endpoint came up with
            // 34.0 GiB above the booked 30.0: the rise re-books at once.
            vec![None, Some(strata_vram_status(34_816))].into(),
        ),
        children: vec![4243],
    }) as Arc<dyn crate::engine::EngineRuntime>;
    let witness = Arc::new(scripted_sample(vec![
        (4242, 20.0),
        (4243, 10.0),
        (9999, 5.0),
    ])) as Arc<dyn crate::budget::VramWitness>;
    let ticket = SpawnTicket {
        generation: 7,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port: 19_042,
    };
    let ctx = sampler_ctx("treeup", "strata", ticket.clone());
    assert_eq!(
        ActorState::report_measurement(&runtime, &tx, &ctx, &witness).await,
        Some(30.0),
        "the status endpoint is silent at readiness: the engine plus all its \
             descendants land the booking, and the sampler is not consumed"
    );
    let sampler = tokio::spawn(async move {
        ActorState::elastic_refresh(
            &runtime,
            &tx,
            &ctx,
            &witness,
            30.0,
            Duration::from_millis(40),
        )
        .await;
    });
    async fn next_report(
        rx: &mut mpsc::UnboundedReceiver<ActorMsg>,
    ) -> crate::engine::EngineMeasurement {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a measurement reaches the actor")
            .expect("actor channel open");
        let ActorMsg::MeasuredProcessReport { measurement, .. } = msg else {
            panic!("expected a process-measurement report");
        };
        measurement
    }
    let landing = next_report(&mut rx).await;
    assert_eq!(
        (landing.gpu_gib, landing.source, landing.confidence),
        (
            30.0,
            crate::engine::MeasurementSource::ProcessTreeNvml,
            crate::engine::MeasurementConfidence::Low,
        ),
        "the landing is the tree sum: a footprint read, not the engine's word"
    );
    let upgrade = next_report(&mut rx).await;
    assert!(
        (upgrade.gpu_gib - 34.0).abs() < 1e-6
            && upgrade.source == crate::engine::MeasurementSource::EngineStatus
            && upgrade.confidence == crate::engine::MeasurementConfidence::High,
        "the first live status answer supersedes the stand-in reading: {upgrade:?}"
    );
    sampler.abort();
}

/// Review F-EM-401, amber half: a resident the rescan marked `stale_config`
/// keeps its booking through a fold. The fold still records what the
/// process actually holds (the measured pair persists — honest data, the
/// same ruling as accepted F-EM-402), but a row that no longer describes
/// the process may not drive the booking: only unload + reload re-applies,
/// exactly the posture `rebook_cost_only_moves` declares. Falsifier: no
/// `stale_config` leg in `rebook_after_measurement` (the 67.8 walks onto
/// the amber resident's slot).
#[tokio::test]
async fn an_amber_residents_fold_records_the_pair_but_never_rebooks() {
    let dir = crate::testutil::temp_dir("fold-rebook-amber");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("amb.yaml");
    std::fs::write(&yaml, "engine: strata\ncost: fixed\nvram_gb: 55\n").expect("seed");
    let mut state = fold_state(&dir);
    let resident = rescan_entry(&yaml, "amb");
    let ticket = fold_ticket(19_043);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 19_043);
    if let Some(occupant) = state.slots[0].occupant.as_mut() {
        occupant.need = Gibibytes(55.0);
        occupant.stale_config = true;
    }

    state.process_measurement(
        "amb",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 67.8,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    let entry = &state.catalog[0];
    assert_eq!(
        (entry.measured_gb, entry.measured_attribution),
        (
            Some(67.8),
            Some(crate::model::MeasuredAttribution::EngineStatus)
        ),
        "the fold still records what the process holds"
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(55.0),
        "but an amber resident's booking stands: the apply gesture is unload + reload"
    );
    await_yaml_keys(&yaml, &["measured_gb: 67.8"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review F-EM-401, greedy half: the refusal reads the SPAWN-TIME cost
/// class the job carries, not the possibly-edited row — an engine launched
/// greedy keeps its card-driven clamped booking whatever the yaml says
/// now (the rescan seam refuses the same resident from its own side).
/// Falsifier: reading only `entry.cost` (the flipped `fixed` row would
/// book the 67.8 onto the greedy launch).
#[tokio::test]
async fn a_greedy_launched_residents_fold_never_rebooks_even_on_a_fixed_row() {
    let dir = crate::testutil::temp_dir("fold-rebook-greedy");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("gdy.yaml");
    std::fs::write(&yaml, "engine: strata\ncost: greedy\nvram_gb: 40\n").expect("seed");
    let mut state = fold_state(&dir);
    let resident = rescan_entry(&yaml, "gdy");
    let ticket = fold_ticket(19_044);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 19_044);
    // The card-driven clamp the greedy admit left behind.
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(40.0);
    // The row reads `fixed` now (an operator flipped the class; the rescan
    // seam has its own greedy legs) — the JOB still carries the spawn-era
    // class, and that is what refuses.
    state.catalog[0].cost = crate::model::CostClass::Fixed;

    state.process_measurement(
        "gdy",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 67.8,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(40.0),
        "a greedy launch is never re-booked by a measurement, whatever the row says now"
    );
    await_yaml_keys(&yaml, &["measured_gb: 67.8"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Forces the file's mtime one minute forward. A same-size edit is then a real
/// byte move even on a coarse volume: the Windows clock can collapse a sub-tick
/// rewrite, and the fingerprint is (mtime, size) (TRR-01).
fn bump_mtime(path: &std::path::Path) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for the clock bump");
    file.set_times(
        std::fs::FileTimes::new().set_modified(
            std::time::SystemTime::now()
                .checked_add(std::time::Duration::from_secs(60))
                .expect("mtime in range"),
        ),
    )
    .expect("force the mtime forward");
}

/// A cost-only rescan (elastic booking, `rebook_cost_only_moves`): the loaded
/// resident's booking follows the new bytes WITHOUT a reload, the capture
/// rides with it so the marker round stays green, and the next unchanged
/// rescan confirms the re-baseline. Falsifiers: no re-book (need stays at
/// the fixture's 4.0), no re-baseline (the unchanged rescan ambers), and
/// laundering a flag move (see the sibling pin below).
#[test]
fn a_cost_only_edit_rebooks_the_loaded_resident_without_a_reload() {
    let (mut state, yaml, _) = resident_on_a_file();
    // The seed and the edit below have the same byte length: force the stamp
    // forward so the edit is a real byte move on every machine.
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 12.0\nmeasured_gb_min: 8.0\n",
    )
    .expect("operator cost edit");
    bump_mtime(&yaml);
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].need_gib,
        Some(12.0),
        "the moved cost figure re-books the resident's release credit"
    );
    assert_eq!(
        after.slots[0].stale_config,
        Some(false),
        "a booking the new bytes justify is applied, not accused"
    );
    rescan(&mut state, &yaml, "m");
    let again = state.snapshot();
    assert_eq!(again.slots[0].stale_config, Some(false));
    assert_eq!(again.slots[0].need_gib, Some(12.0));
}

/// The boundary stays a boundary: an edit that moves a flag (here together
/// with a cost figure) is a config change the running engine never read —
/// it ambers exactly as before and re-books NOTHING, even though the cost
/// figure alone would have been adopted. Falsifier: classifying any
/// byte-move as cost-only (this goes green with an unearned 99.0).
#[test]
fn a_flag_edit_still_ambers_the_resident_and_never_rebooks_it() {
    let (mut state, yaml, _) = resident_on_a_file();
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 99.0\nmeasured_gb_min: 8.0\ncpu_offload: true\n",
    )
    .expect("operator edit moving a flag alongside the cost figure");
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].stale_config,
        Some(true),
        "the launch no longer matches the row: the amber gesture (unload + reload) stands"
    );
    assert_eq!(
        after.slots[0].need_gib,
        Some(4.0),
        "a resident whose engine never read the bytes never re-books on them"
    );
}

/// Honesty control for the re-book classifier: bytes moved with NO cost
/// field actually different (a comment note) is not a cost move — the
/// marker round's conservatism keeps the amber and the old booking.
#[test]
fn a_comment_only_edit_is_not_a_cost_move() {
    let (mut state, yaml, _) = resident_on_a_file();
    let original = std::fs::read_to_string(&yaml).expect("read");
    std::fs::write(&yaml, format!("{original}# operator note\n")).expect("comment edit");
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(after.slots[0].stale_config, Some(true));
    assert_eq!(
        after.slots[0].need_gib,
        Some(4.0),
        "nothing moved that a booking could follow"
    );
}

/// A cost move that leaves the row unable to answer the gate (an `exact`
/// stripped of every size) is INVALIDATED, never silently kept: the
/// marker round ambers it (the acceptance criterion's other half — update
/// the booking or invalidate it, never keep a number the bytes deny).
#[test]
fn a_cost_move_to_an_unbookable_row_invalidates_instead_of_rebooking() {
    let (mut state, yaml, _) = resident_on_a_file();
    std::fs::write(&yaml, "engine: mock\ncost: exact\n").expect("strip every size");
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].stale_config,
        Some(true),
        "the row no longer justifies any booking: unload + reload is the gesture"
    );
    assert_eq!(after.slots[0].need_gib, Some(4.0));
}

/// An amber the operator's flag edit earned is never laundered by a later
/// cost edit: the resident whose launch no longer matches its row may not
/// re-book against bytes its engine never read. Falsifier: dropping the
/// `stale_config` guard in `rebook_cost_only_moves` (the cost edit below
/// would turn the resident green and re-booked).
#[test]
fn an_earned_amber_survives_a_later_cost_only_edit() {
    let (mut state, yaml, _) = resident_on_a_file();
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 10.0\nmeasured_gb_min: 8.0\ncpu_offload: true\n",
    )
    .expect("flag edit earns the amber");
    rescan(&mut state, &yaml, "m");
    assert_eq!(state.snapshot().slots[0].stale_config, Some(true));
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 99.0\nmeasured_gb_min: 8.0\ncpu_offload: true\n",
    )
    .expect("later cost-only edit over the amber");
    rescan(&mut state, &yaml, "m");
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].stale_config,
        Some(true),
        "the amber stays until the unload + reload applies it"
    );
    assert_eq!(after.slots[0].need_gib, Some(4.0));
}

/// The ledger-boundary pin: a cost-only rescan of a
/// live CPU-resident row must not move it between the ledgers. Adding
/// `vram_gb` to the loaded row is a pure COST_KEYS move, so the re-book
/// classifier fires — and must find nothing to re-book: the VRAM ladder's
/// figure may never be written into a RAM resident's `need`, where the
/// same-slot release credit would bill a GPU replacement GiB the CPU engine
/// never held (the stranger's-paper-number class). The bytes are
/// still adopted (a cost edit neither re-books nor ambers a healthy
/// resident), and a later flag edit still earns the amber.
/// Falsifier: with the guard dropped, `booking_need` answers the VRAM ladder
/// for the RAM row and the snapshot shows `need_gib` 12.0 — a booking into
/// the card ledger — and the phantom credit follows into the next
/// replacement.
#[test]
fn a_cost_only_edit_never_moves_a_ram_resident_into_the_vram_ledger() {
    let dir = crate::testutil::temp_dir("rebook-ram-resident");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let yaml = dir.join("cpu.yaml");
    std::fs::write(&yaml, "engine: llama\ncpu_offload: true\nram_gb: 10\n").expect("seed");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
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
    let capture = crate::model::fingerprint(&yaml).expect("seeded file fingerprints");
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port: 18_431,
    };
    state.catalog = vec![entry_from(&yaml, "cpu")];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_431,
        occupant: Some(Occupant {
            model: "cpu".to_string(),
            need: Gibibytes(0.0),
            ram_need: Some(Gibibytes(10.0)),
            port: 18_431,
            ticket,
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: Some((yaml.clone(), capture)),
            stale_config: false,
            serve_id: None,
            measure: None,
            live_measured_gb: None,
        }),
        job: None,
    }];
    // Same-size cost edit: force the stamp forward (the GPU re-book pin's
    // posture).
    std::fs::write(
        &yaml,
        "engine: llama\ncpu_offload: true\nram_gb: 10\nvram_gb: 12\n",
    )
    .expect("operator cost edit");
    bump_mtime(&yaml);
    rescan(&mut state, &yaml, "cpu");
    let after = state.snapshot();
    assert_eq!(
        state.catalog[0].resource(),
        crate::model::Resource::Ram,
        "a cost edit cannot re-tag the row's resource"
    );
    assert_eq!(
        after.slots[0].need_gib,
        Some(0.0),
        "the VRAM ladder must not book a RAM resident (a pre-fix rebook shows 12.0 here)"
    );
    assert_eq!(
        after.slots[0].stale_config,
        Some(false),
        "a cost move the resident cannot see is adopted, not accused"
    );
    let occupant = state.slots[0].occupant.as_ref().expect("resident");
    assert_eq!(occupant.need, Gibibytes(0.0));
    assert_eq!(
        occupant.ram_need,
        Some(Gibibytes(10.0)),
        "the RAM booking stands: only a reload moves it (wave 2 owns the measured rung)"
    );
    assert_eq!(
        state.ledger_sum(),
        Gibibytes(0.0),
        "the card ledger gained nothing"
    );
    assert_eq!(
        state.ram_ledger_sum(),
        Gibibytes(10.0),
        "the RAM ledger lost nothing"
    );
    // The amber half: a later flag edit still earns the amber — adopting
    // cost bytes launders nothing.
    // Same-size flag edit: force the stamp forward so the amber is a byte
    // move on every machine.
    std::fs::write(
        &yaml,
        "engine: llama\ncpu_offload: false\nram_gb: 10\nvram_gb: 12\n",
    )
    .expect("operator flag edit");
    bump_mtime(&yaml);
    rescan(&mut state, &yaml, "cpu");
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].stale_config,
        Some(true),
        "the launch no longer matches the row: the unload + reload gesture stands"
    );
    assert_eq!(
        after.slots[0].need_gib,
        Some(0.0),
        "and still nothing re-books"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
