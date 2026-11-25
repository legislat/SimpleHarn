use super::*;

/// A runtime the readmission pins can safely touch: the kill and the
/// release proof answer immediately (the pins judge only the admit
/// decision and never drive the swap to the end), while everything the
/// pins must never reach keeps the never-called posture.
struct ReadmitRuntime;

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for ReadmitRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        Ok(crate::engine::Spawned {
            port: 18_999,
            pid: 0,
        })
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        Ok(())
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        Ok(true)
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the readmission pin never reaches readiness");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the readmission pin never reaches readiness");
    }
    async fn engine_status_json(&self, _port: u16, _path: &str) -> Option<serde_json::Value> {
        None
    }
    async fn descendant_pids(&self, _pid: u32) -> Vec<u32> {
        Vec::new()
    }
}

/// The 2026-11-04 readmission shape, parameterised: one WORK slot holding
/// a resident booked at `paper_gib` while the scripted 72 GiB card
/// physically carries `held_gib` (under-booked resident, the incident),
/// and no other occupancy — the same-slot replacement is the only release
/// the gate can ever count.
fn readmit_state(held_gib: f64) -> (ActorState, mpsc::UnboundedReceiver<ActorMsg>) {
    let (tx, rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    for engine in ["mock", "basalt", "strata"] {
        adapters.insert(
            engine.to_string(),
            Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
        );
    }
    let witness = Arc::new(ScriptedWitness(crate::budget::MemorySample {
        total_gib: 72.0,
        used_gib: held_gib,
        processes: Vec::new(),
        attribution: crate::budget::attribution_from_processes(&[]),
    })) as Arc<dyn crate::budget::VramWitness>;
    let state = ActorState::empty(
        crate::config::AppConfig::default(),
        crate::testutil::temp_dir("readmit"),
        adapters,
        Arc::new(ReadmitRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        witness,
        tx,
    );
    (state, rx)
}

fn incident_sample(used_gib: f64) -> crate::budget::MemorySample {
    crate::budget::MemorySample {
        total_gib: 72.0,
        used_gib,
        processes: Vec::new(),
        attribution: crate::budget::attribution_from_processes(&[]),
    }
}

/// Arms `entry` on slot 0 and hands the gate the card reading — the two
/// synchronous halves of the load path (`start_queued_load` also samples a
/// scripted witness on a task; its message stays unread in the pin's rx).
fn admit_round(state: &mut ActorState, entry: &ModelEntry, used_gib: f64) -> super::JobId {
    let id = state.register_queued_load(0, entry, Vec::new());
    state.start_queued_load(id, 0, entry.clone());
    state.finish_admit_card(id, Ok(incident_sample(used_gib)));
    id
}

/// T1 (the incident, Strata→Basalt): while the release credit books the
/// row's paper 55 and the card physically holds the resident's 70.4, the
/// 67.4 replacement with a 65 GiB launch floor must be REFUSED with the
/// incident's own verdict — no release the harness may perform reaches the
/// floor (54.6 credited free vs a 63 floor after tolerance). Once the
/// engine's own status figure has folded (the elastic seam), the SAME load on
/// the SAME card names the resident as its victim and arms. The red leg is
/// the falsifier for the whole feature: a booking policy that ignores the
/// engine's word keeps this refusal forever.
#[tokio::test]
async fn a_strata_resident_releases_for_basalt_only_once_its_measured_footprint_is_booked() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-sb");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("strata-v1.yaml");
    std::fs::write(&resident_yaml, "engine: strata\ncost: fixed\nvram_gb: 55\n").expect("seed");
    let incoming_yaml = dir.join("basalt-v2.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: basalt\ncost: fixed\nvram_gb: 67.4\nlaunch_only_with_at_least_vram_free: 65\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(70.4);
    let resident = rescan_entry(&resident_yaml, "strata-v1");
    let ticket = fold_ticket(18_109);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_109);
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(55.0);
    let incoming = rescan_entry(&incoming_yaml, "basalt-v2");
    // The gate preflights the launch before it evicts, so the replacement's
    // row must be in the catalog like it would be on a serving box.
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 70.4);
    let job = &state.jobs[&red];
    assert!(job.terminal, "the paper-credit gate must refuse, not arm");
    assert!(
        matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "the incident's verdict: {:?}",
        job.error
    );

    state.process_measurement(
        "strata-v1",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 70.4,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(70.4),
        "the fold re-books the slot before any replacement is judged"
    );

    let green = admit_round(&mut state, &incoming, 70.4);
    let job = &state.jobs[&green];
    assert!(
        !job.terminal && job.error.is_none(),
        "the footprint credit covers the need and the floor: {:?}",
        job.error
    );
    assert_eq!(
        job.victim.as_ref().expect("victim").model,
        "strata-v1",
        "the release-first swap names the resident it is freeing"
    );
    await_yaml_keys(&resident_yaml, &["measured_gb: 70.4"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// T2 (the mirror, Basalt→Strata): paper 60 held 67.4, the incoming Strata
/// row needs 70.0 with the same 65 floor — red the need deficit (7.4) is
/// even LARGER than the floor deficit (2.4), and the guard still shapes
/// the refusal as the floor verdict, exactly as the ruling
/// demands. After the fold (67.4 credited, free 70.0) it admits.
#[tokio::test]
async fn a_basalt_resident_releases_for_strata_only_once_its_measured_footprint_is_booked() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-bs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("basalt-v1.yaml");
    std::fs::write(&resident_yaml, "engine: basalt\ncost: fixed\nvram_gb: 60\n").expect("seed");
    let incoming_yaml = dir.join("strata-v2.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: strata\ncost: fixed\nvram_gb: 70.0\nlaunch_only_with_at_least_vram_free: 65\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(67.4);
    let resident = rescan_entry(&resident_yaml, "basalt-v1");
    let ticket = fold_ticket(18_110);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_110);
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(60.0);
    let incoming = rescan_entry(&incoming_yaml, "strata-v2");
    // The gate preflights the launch before it evicts: the row rides.
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 67.4);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "the incident's verdict: {:?}",
        state.jobs[&red].error
    );

    state.process_measurement(
        "basalt-v1",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 67.4,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    assert_eq!(state.snapshot().slots[0].need_gib, Some(67.4));

    let green = admit_round(&mut state, &incoming, 67.4);
    let job = &state.jobs[&green];
    assert!(
        !job.terminal && job.error.is_none(),
        "free 70.0 books the 70.0 need and clears the floor: {:?}",
        job.error
    );
    await_yaml_keys(&resident_yaml, &["measured_gb: 67.4"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// T3 (the row-cost half, Strata side): no measurement at all — the
/// operator moves the row's declared cost to the footprint the engine was
/// seen holding (55 -> 70.4) and reloads; the rescan re-books the loaded
/// resident green, and the replacement admits against the SAME card that
/// refused it minutes before. This is the trap's manual gesture made
/// automatic: no unload, no reload of the resident.
#[tokio::test]
async fn a_rebooked_row_lets_the_basalt_replacement_in_without_touching_the_resident() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-row-sb");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("strata-v1.yaml");
    std::fs::write(&resident_yaml, "engine: strata\ncost: fixed\nvram_gb: 55\n").expect("seed");
    let incoming_yaml = dir.join("basalt-v2.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: basalt\ncost: fixed\nvram_gb: 67.4\nlaunch_only_with_at_least_vram_free: 65\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(70.4);
    let resident = rescan_entry(&resident_yaml, "strata-v1");
    let ticket = fold_ticket(18_111);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_111);
    let capture = crate::model::fingerprint(&resident_yaml).expect("stat");
    if let Some(occupant) = state.slots[0].occupant.as_mut() {
        occupant.need = Gibibytes(55.0);
        // The launch came from this yaml: install captures it, and only a
        // captured resident can have its bytes re-judged by a rescan.
        occupant.spawn_config = Some((resident_yaml.clone(), capture));
    }
    let incoming = rescan_entry(&incoming_yaml, "basalt-v2");
    // The gate preflights the launch before it evicts: the row rides.
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 70.4);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "the red control before the row moved: {:?}",
        state.jobs[&red].error
    );

    std::fs::write(
        &resident_yaml,
        "engine: strata\ncost: fixed\nvram_gb: 70.4\n",
    )
    .expect("operator moves the declared cost to the footprint");
    rescan(&mut state, &resident_yaml, "strata-v1");
    // `rescan` lands a one-row catalog; the replacement's row rides again.
    state.catalog.push(incoming.clone());
    let after = state.snapshot();
    assert_eq!(
        after.slots[0].need_gib,
        Some(70.4),
        "the rescan re-books the loaded resident's credit from the new row"
    );
    assert_eq!(
        after.slots[0].stale_config,
        Some(false),
        "a cost-only move is applied, not accused: no reload trap"
    );

    let green = admit_round(&mut state, &incoming, 70.4);
    let job = &state.jobs[&green];
    assert!(
        !job.terminal && job.error.is_none(),
        "the same card that refused now releases: {:?}",
        job.error
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// T4 (the row-cost half, Basalt side, mirror of T3): declared 60 -> 67.4
/// re-books the resident; the 70.0 Strata replacement that the stale row
/// refused (free 62.6 under a 63 floor-after-tolerance) admits against
/// free 70.0.
#[tokio::test]
async fn a_rebooked_row_lets_the_strata_replacement_in_without_touching_the_resident() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-row-bs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("basalt-v1.yaml");
    std::fs::write(&resident_yaml, "engine: basalt\ncost: fixed\nvram_gb: 60\n").expect("seed");
    let incoming_yaml = dir.join("strata-v2.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: strata\ncost: fixed\nvram_gb: 70.0\nlaunch_only_with_at_least_vram_free: 65\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(67.4);
    let resident = rescan_entry(&resident_yaml, "basalt-v1");
    let ticket = fold_ticket(18_112);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_112);
    let capture = crate::model::fingerprint(&resident_yaml).expect("stat");
    if let Some(occupant) = state.slots[0].occupant.as_mut() {
        occupant.need = Gibibytes(60.0);
        // The launch came from this yaml: install captures it, and only a
        // captured resident can have its bytes re-judged by a rescan.
        occupant.spawn_config = Some((resident_yaml.clone(), capture));
    }
    let incoming = rescan_entry(&incoming_yaml, "strata-v2");
    // The gate preflights the launch before it evicts: the row rides.
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 67.4);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "the red control before the row moved: {:?}",
        state.jobs[&red].error
    );

    std::fs::write(
        &resident_yaml,
        "engine: basalt\ncost: fixed\nvram_gb: 67.4\n",
    )
    .expect("operator moves the declared cost to the footprint");
    rescan(&mut state, &resident_yaml, "basalt-v1");
    // `rescan` lands a one-row catalog; the replacement's row rides again.
    state.catalog.push(incoming.clone());
    let after = state.snapshot();
    assert_eq!(after.slots[0].need_gib, Some(67.4));
    assert_eq!(after.slots[0].stale_config, Some(false));

    let green = admit_round(&mut state, &incoming, 67.4);
    let job = &state.jobs[&green];
    assert!(
        !job.terminal && job.error.is_none(),
        "free 70.0 books the 70.0 need and clears the floor: {:?}",
        job.error
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The greedy resident: a `cost: greedy`
/// Basalt occupant holds the card (70.4) while its booking stays the room
/// it grabbed at admit (50) - a measured pair may never re-book a greedy
/// row (DOCTRINE 12), so this stale-looking credit is the honest one. A
/// fixed Strata replacement with a 65 floor is judged against the booked
/// room: red while free+credit misses the floor-after-tolerance (49.6 vs
/// 63) with the floor verdict `no_releases_reach`, exactly as a fixed
/// resident would take; a 70.4 fold changes nothing on the greedy slot;
/// and once the card reading lets the booked room alone carry the floor
/// (free 30 without the victim credit, 80 with it) the replacement admits
/// and names the greedy resident as the victim it frees. Falsifier: a
/// greedy fold that re-books the slot, or a floor guard that stops
/// crediting the same-slot victim, flips the red or the green arm.
#[tokio::test]
async fn a_greedy_basalt_resident_credits_only_its_room_at_admit() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-greedy");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("basalt-greedy.yaml");
    std::fs::write(
        &resident_yaml,
        "engine: basalt\ncost: greedy\nlaunch_only_with_at_least_vram_free: 60\n",
    )
    .expect("seed");
    let incoming_yaml = dir.join("strata-v2.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: strata\ncost: fixed\nvram_gb: 60\nlaunch_only_with_at_least_vram_free: 65\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(70.4);
    let resident = rescan_entry(&resident_yaml, "basalt-greedy");
    let ticket = fold_ticket(18_113);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_113);
    // The greedy admit booked the room it grabbed (50), not the 70.4 the
    // card physically carries - the clamp `min(nominal, planned_room)`.
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(50.0);
    let incoming = rescan_entry(&incoming_yaml, "strata-v2");
    // The gate preflights the launch before it evicts: the row rides.
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 70.4);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "free 49.6 with the booked room credited misses the 63 floor: {:?}",
        state.jobs[&red].error
    );

    state.process_measurement(
        "basalt-greedy",
        &ticket,
        crate::engine::EngineMeasurement {
            gpu_gib: 70.4,
            ram_gib: None,
            source: crate::engine::MeasurementSource::EngineStatus,
            confidence: crate::engine::MeasurementConfidence::High,
        },
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(50.0),
        "a measured pair never books or re-books a greedy row (DOCTRINE 12)"
    );

    // The same replacement on a card whose own occupancy released: free 30
    // never reaches the 65 floor, free 80 does - the greedy victim's booked
    // room alone is what admits the load.
    let green = admit_round(&mut state, &incoming, 40.0);
    let job = &state.jobs[&green];
    assert!(
        !job.terminal && job.error.is_none(),
        "the booked room credits the replacement over the floor: {:?}",
        job.error
    );
    assert_eq!(
        job.victim.as_ref().expect("victim").model,
        "basalt-greedy",
        "the replacement names the greedy resident it is freeing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The greedy swap-credit falsifier: a greedy
/// Strata books its nominal 12.0 while ITS OWN spawn's fold witnessed 68.4 GiB
/// on the card (driven here through the real `fold` entry with a Vram FoldPlan
/// for the installed ticket). The card holds 71.1 of the helper's 72 GiB
/// (field: 71.66) and a greedy Work replacement declared 67.4 with a 60 GiB
/// floor arrives — the release frees the footprint, not the booking, so the
/// swap must ADMIT on the projected release instead of dying with the field's
/// verdict ("59.1 GiB unreleasable … No releases allow for the VRAM floor").
/// The booking itself never moves: the projection is a credit for this one
/// gate, and the settle re-gate stays the physical truth. Falsifier: crediting
/// the victim at its booking alone (the pre-ruling shape) flips the green arm
/// back to `no_releases_reach` — red first, green with the ruling.
#[tokio::test]
async fn a_greedy_residents_folded_footprint_credits_the_same_slot_release() {
    use super::measured::{FoldPlan, MeasuredPair};
    let dir = crate::testutil::temp_dir("readmit-live-credit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("strata-greedy.yaml");
    std::fs::write(
        &resident_yaml,
        "engine: strata\ncost: greedy\nvram_gb: 12\n",
    )
    .expect("seed");
    let incoming_yaml = dir.join("basalt-live.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: basalt\ncost: greedy\nvram_gb: 67.4\nlaunch_only_with_at_least_vram_free: 60\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(71.1);
    let resident = rescan_entry(&resident_yaml, "strata-greedy");
    let ticket = fold_ticket(18_120);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_120);
    // The greedy admit left the nominal booking behind, not the 68.4 the card
    // physically carries (the same clamp T5 pins).
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(12.0);
    let incoming = rescan_entry(&incoming_yaml, "basalt-live");
    // The gate preflights the launch before it evicts: the row rides.
    state.catalog.push(incoming.clone());

    state.fold(&FoldPlan {
        model: "strata-greedy".to_string(),
        ticket: ticket.clone(),
        high: 68.4,
        low: 68.4,
        attribution: crate::model::MeasuredAttribution::EngineStatus,
        profile: None,
        fold_range: true,
        pair: MeasuredPair::Vram,
    });
    assert_eq!(
        state.slots[0]
            .occupant
            .as_ref()
            .expect("resident")
            .live_measured_gb,
        Some(68.4),
        "the spawn's own fold is what stands behind the release credit"
    );
    assert_eq!(
        state.snapshot().slots[0].need_gib,
        Some(12.0),
        "the booking stays the greedy nominal (DOCTRINE 12): a projection is not a booking"
    );

    let green = admit_round(&mut state, &incoming, 71.1);
    let job = &state.jobs[&green];
    assert!(
        !job.terminal && job.error.is_none(),
        "the projected release carries the 60 floor over: {:?}",
        job.error
    );
    assert_eq!(
        job.path,
        Some(AdmissionPath::FitsNow),
        "with the footprint credited the numbers already reach the floor — no drain is queued"
    );
    let victim = job.victim.as_ref().expect("victim");
    assert_eq!(victim.model, "strata-greedy");
    assert_eq!(
        victim.ticket, ticket,
        "the committed release names THIS spawn's resident"
    );
    await_yaml_keys(&resident_yaml, &["measured_gb: 68.4"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stale-credit falsifier: the same shape, but THIS spawn never
/// folded — and the on-disk row already carries the previous launch's 68.4
/// pair. A disk figure is not this process's evidence (`live_measured_gb` is
/// spawn-ticket-scoped and never seeded from disk), so the credit stays the
/// booking and the 2026-11-01 guard keeps its verdict: no release reaches the
/// floor. Falsifier: seeding `live_measured_gb` from the catalog's measured
/// pair — the stale pair the ruling forbids — turns this red.
#[tokio::test]
async fn a_greedy_resident_without_this_spawns_fold_keeps_the_paper_refusal() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-live-credit-nofold");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("strata-greedy.yaml");
    std::fs::write(
        &resident_yaml,
        "engine: strata\ncost: greedy\nvram_gb: 12\nmeasured_gb: 68.4\nmeasured_gb_min: 40\n",
    )
    .expect("seed");
    let incoming_yaml = dir.join("basalt-live.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: basalt\ncost: greedy\nvram_gb: 67.4\nlaunch_only_with_at_least_vram_free: 60\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(71.1);
    let resident = rescan_entry(&resident_yaml, "strata-greedy");
    let ticket = fold_ticket(18_121);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_121);
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(12.0);
    assert_eq!(
        state.slots[0]
            .occupant
            .as_ref()
            .expect("resident")
            .live_measured_gb,
        None,
        "the seat never seeds the live figure from the row's on-disk pair"
    );
    let incoming = rescan_entry(&incoming_yaml, "basalt-live");
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 71.1);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "without this spawn's fold the refusal stands on the booked 12: {:?}",
        state.jobs[&red].error
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The cross-slot falsifier: the projection never crosses a slot. The
/// card holds a folded greedy Work resident on ANOTHER slot while the load's
/// own slot is empty; the fold still finds its ticket by spawn match, but the
/// same-slot lookup finds no victim, nothing the harness may release reaches
/// the floor (Work is evicted by nobody), and the refusal keeps the guard's
/// verdict. Falsifier: crediting `live_measured_gb` from any occupant instead
/// of the job's own slot turns this green on a foreign resident's bytes.
#[tokio::test]
async fn a_folded_footprint_never_credits_a_cross_slot_replacement() {
    use super::measured::{FoldPlan, MeasuredPair};
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-live-credit-cross");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("strata-greedy.yaml");
    std::fs::write(
        &resident_yaml,
        "engine: strata\ncost: greedy\nvram_gb: 12\n",
    )
    .expect("seed");
    let incoming_yaml = dir.join("basalt-live.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: basalt\ncost: greedy\nvram_gb: 67.4\nlaunch_only_with_at_least_vram_free: 60\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(71.1);
    let resident = rescan_entry(&resident_yaml, "strata-greedy");
    let ticket = fold_ticket(18_122);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_122);
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(12.0);
    let seated = JobId(state.next_job - 1);
    // Stand the resident on slot 1 and leave the incoming's slot 0 empty: the
    // fold must still find its ticket there, but the same-slot credit must not.
    state.slots.push(Slot {
        category: CategoryId::Work,
        index: 1,
        engine_port: 18_123,
        occupant: None,
        job: None,
    });
    state.slots[1].occupant = state.slots[0].occupant.take();
    state.slots[1].job = state.slots[0].job.take();
    if let Some(job) = state.jobs.get_mut(&seated) {
        job.slot = Some(1);
    }
    state.fold(&FoldPlan {
        model: "strata-greedy".to_string(),
        ticket: ticket.clone(),
        high: 68.4,
        low: 68.4,
        attribution: crate::model::MeasuredAttribution::EngineStatus,
        profile: None,
        fold_range: true,
        pair: MeasuredPair::Vram,
    });
    assert_eq!(
        state.slots[1]
            .occupant
            .as_ref()
            .expect("resident")
            .live_measured_gb,
        Some(68.4),
        "the fold credits the slot its ticket actually stands on"
    );
    let incoming = rescan_entry(&incoming_yaml, "basalt-live");
    state.catalog.push(incoming.clone());

    let red = admit_round(&mut state, &incoming, 71.1);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "a folded footprint on a foreign slot is not this swap's release: {:?}",
        state.jobs[&red].error
    );
    await_yaml_keys(&resident_yaml, &["measured_gb: 68.4"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The credit is the largest of THIS SPAWN's
/// own readings, never the range-merge result. The spawn starts on a row that
/// already carries the previous launch's persisted pair (68.4 / 68.3 — the
/// `MeasuredAnchor` seeds its range from it), then lands ONE small witness
/// fold of 40.0 through the real `fold`. The merged pair is what the yaml and
/// the catalog keep (history), but `live_measured_gb` may only hold the spawn's
/// own 40.0 — a stale on-disk figure must never be credited. A greedy
/// replacement with a 60 floor therefore still refuses conservatively
/// (booked 12, own reading 40: no release reaches the floor). Falsifier on the
/// merge-folded shape: crediting the merged high (68.4, seeded from disk) flips
/// the live figure and admits the load on the previous launch's bytes.
#[tokio::test]
async fn a_stale_persisted_pair_never_rides_the_release_credit() {
    use super::measured::{FoldPlan, MeasuredPair};
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("readmit-live-credit-stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let resident_yaml = dir.join("strata-greedy.yaml");
    std::fs::write(
        &resident_yaml,
        "engine: strata\ncost: greedy\nvram_gb: 12\nmeasured_gb: 68.4\nmeasured_gb_min: 68.3\n",
    )
    .expect("seed");
    let incoming_yaml = dir.join("basalt-live.yaml");
    std::fs::write(
        &incoming_yaml,
        "engine: basalt\ncost: greedy\nvram_gb: 67.4\nlaunch_only_with_at_least_vram_free: 60\n",
    )
    .expect("seed");
    let (mut state, _rx) = readmit_state(71.1);
    let resident = rescan_entry(&resident_yaml, "strata-greedy");
    let ticket = fold_ticket(18_124);
    state.catalog = vec![resident.clone()];
    seat_spawn(&mut state, &resident, &ticket, 18_124);
    state.slots[0].occupant.as_mut().expect("resident").need = Gibibytes(12.0);
    let incoming = rescan_entry(&incoming_yaml, "basalt-live");
    state.catalog.push(incoming.clone());

    state.fold(&FoldPlan {
        model: "strata-greedy".to_string(),
        ticket: ticket.clone(),
        high: 40.0,
        low: 40.0,
        attribution: crate::model::MeasuredAttribution::EngineStatus,
        profile: None,
        fold_range: true,
        pair: MeasuredPair::Vram,
    });
    assert_eq!(
        state.slots[0]
            .occupant
            .as_ref()
            .expect("resident")
            .live_measured_gb,
        Some(40.0),
        "the credit is the spawn's own reading; the persisted 68.4 that seeded the range merge is excluded"
    );

    let red = admit_round(&mut state, &incoming, 71.1);
    assert!(
        matches!(
            state.jobs[&red].error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "booked 12 + own 40 reaches nothing near the 60 floor: {:?}",
        state.jobs[&red].error
    );
    await_yaml_keys(&resident_yaml, &["measured_gb: 68.4"]).await;
    let _ = std::fs::remove_dir_all(&dir);
}
