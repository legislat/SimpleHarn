use super::*;

/// The seat with a back-dated turn: the idle sweep compares
/// `state_last_used` against `loaded_at`, so a swept candidate must look
/// older than its category's `idle_timeout_s`.
fn stale_seat(model: &str, need: f64, ram_need: Option<f64>, port: u16) -> Occupant {
    let mut seat = ram_settle_seat(model, need, ram_need, port);
    seat.loaded_at = Instant::now() - Duration::from_secs(30);
    seat
}

/// A queued expert-offload
/// reclaim victim is committed to release from the moment it is planned —
/// exactly like a deficit victim under the same release-commit rule. With AUX, RP and EMB all
/// opted into `idle_timeout_s`, a tick after the admit must sweep NONE of
/// the committed rows: the dequeue-in-flight victim (covered by the `victim`
/// arm already) and the still-queued reclaim victim (the arm added
/// for it) both stay installed, while the idle sweep demonstrably still runs —
/// the uninvolved small row it also covers gets its unload job. Without two
/// causes of death for one model.
/// Falsifier: remove the `reclaim.victims` arm of
/// `is_committed_release_target` and the sweep mints its own unload of
/// `mid-cpu` behind the queued reclaim release — the leg below goes red
/// while the innocent-row leg stays green (the sweep did run, the predicate
/// missed the queued victim).
#[tokio::test]
async fn the_idle_sweep_skips_a_queued_expert_offload_reclaim_victim() {
    let dir = crate::testutil::temp_dir("expert-reclaim-sweep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = expert_state(&dir);
    for cat in state.cfg.categories.iter_mut() {
        if matches!(cat.id, CategoryId::Aux | CategoryId::Rp | CategoryId::Emb) {
            cat.idle_timeout_s = 1;
        }
    }
    let entry = expert_row(&dir, "expert-sweep", "maya", true);
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_156,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_157,
            occupant: Some(stale_seat("big-cpu", 0.0, Some(12.0), 18_157)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 2,
            engine_port: 18_158,
            occupant: Some(stale_seat("mid-cpu", 0.0, Some(8.0), 18_158)),
            job: None,
        },
        Slot {
            category: CategoryId::Emb,
            index: 3,
            engine_port: 18_159,
            occupant: Some(stale_seat("innocent-cpu", 0.0, Some(2.0), 18_159)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(job_id, Ok(free_card()));
    let job = &state.jobs[&job_id];
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    assert_eq!(
        job.victim.as_ref().expect("dequeued first").model,
        "big-cpu"
    );
    assert_eq!(
        job.reclaim.victims[0].model, "mid-cpu",
        "the queued victim under test"
    );
    // Past the 1 s idle threshold: the sweep runs on the actor's tick.
    state.on_tick();
    let unloads: Vec<&str> = state
        .jobs
        .values()
        .filter(|job| job.kind == JobKind::Unload)
        .map(|job| job.model.as_str())
        .collect();
    assert!(
        !unloads.contains(&"mid-cpu"),
        "M-R-01 falsifier: the idle sweep minted its own unload behind the \
         queued reclaim release — two causes of death for one model: {unloads:?}"
    );
    assert!(
        !unloads.contains(&"big-cpu"),
        "the dequeue-in-flight victim was already covered (R2-20-01): {unloads:?}"
    );
    assert!(
        unloads.contains(&"innocent-cpu"),
        "the sweep itself still works — without this leg the pin above proves nothing: {unloads:?}"
    );
    // Both committed rows are still installed, awaiting the load's own release path.
    assert_eq!(
        state.slots[1].occupant.as_ref().expect("stays").model,
        "big-cpu"
    );
    assert_eq!(
        state.slots[2].occupant.as_ref().expect("stays").model,
        "mid-cpu"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Both victim queues on one
/// load. A greedy expert admit that is short on the card drains in the
/// mandated order — the same-slot holder first, then the deficit queue, and
/// only after the card gate is honest again does the reclaim queue run —
/// each release confirmed, every skip sparing its row, the settle window
/// closing before the single spawn. The early-stop branch ("release
/// sufficient") fires while a deficit victim is still queued, and it must
/// continue into the reclaim queue, never settle past it: the leg below
/// stands the gate figure where the released credits already cover it, which
/// is exactly what that branch means, and asserts the reclaim victim is the
/// next kill, not a spawn.
/// Falsifier: let the early stop settle (pre-fix shape) and `mid-cpu` is
/// never killed, `spawns` moves to 1 while the reclaim row is still
/// installed, and the victims-released order goes red; drain reclaim before
/// the deficit queue and the first releases name the wrong rows.
#[tokio::test]
async fn a_mixed_drain_runs_holder_then_deficit_then_reclaim_then_spawn() {
    let dir = crate::testutil::temp_dir("expert-reclaim-mixed");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "maya".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let runtime = Arc::new(SettleRuntime {
        confirm: true,
        ..SettleRuntime::default()
    });
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.to_path_buf(),
        adapters,
        Arc::clone(&runtime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = expert_row(&dir, "expert-mix", "maya", true);
    state.catalog = vec![entry.clone()];
    let mut gpu_a = ram_settle_seat("gpu-a", 1.5, None, 18_161);
    gpu_a.loaded_at = Instant::now() - Duration::from_secs(10); // LRU first within AUX
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_160,
            occupant: Some(ram_settle_seat("old-holder", 0.0, Some(6.0), 18_160)),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_161,
            occupant: Some(gpu_a),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 2,
            engine_port: 18_162,
            occupant: Some(ram_settle_seat("gpu-b", 1.5, None, 18_162)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 3,
            engine_port: 18_163,
            occupant: Some(ram_settle_seat("mid-cpu", 0.0, Some(8.0), 18_163)),
            job: None,
        },
        Slot {
            category: CategoryId::Emb,
            index: 4,
            engine_port: 18_164,
            occupant: Some(ram_settle_seat("small-cpu", 0.0, Some(3.0), 18_164)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    // Card used 68.5 of 72: free = 72 - 2 reserve - 68.5 = 1.5 < the 4.0
    // greedy launch floor — a 2.5 GiB deficit the two 1.5 GiB AUX residents
    // cover only as a pair, so the deficit queue is genuinely two deep at
    // plan time.
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 68.5,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    // The dispatched-reading twin the manual card leg stands in for; the
    // state is past Admit, applying it is the documented stale-sample no-op.
    match next_ram_event(&mut rx, job_id, "the dispatched reading").await {
        super::Event::Admitted(card, ram) => {
            state.apply_event(job_id, super::Event::Admitted(card, ram));
        }
        other => panic!("the admit task sends its sample first: {other:?}"),
    }
    {
        let job = &state.jobs[&job_id];
        assert!(job.error.is_none(), "{:?}", job.error);
        assert_eq!(job.path, Some(AdmissionPath::Drain));
        assert_eq!(
            job.victim.as_ref().expect("holder first").model,
            "old-holder"
        );
        let queued: Vec<&str> = job.victims.iter().map(|v| v.model.as_str()).collect();
        assert_eq!(queued, vec!["gpu-a", "gpu-b"], "deficit pair, LRU first");
        let reclaim: Vec<&str> = job
            .reclaim
            .victims
            .iter()
            .map(|v| v.model.as_str())
            .collect();
        assert_eq!(
            reclaim,
            vec!["mid-cpu"],
            "the 8 GiB row; the 3 GiB row stays"
        );
        // Stand the gate figure where the first confirmed release's credit
        // already covers it — the exact situation the "release sufficient"
        // early stop exists for (see the leg after the second release).
        state.jobs.get_mut(&job_id).expect("row").gate_target = Some(2.8);
    }
    // Release one: the same-slot holder. The next dequeue arms gpu-a; the
    // re-check at 1.5 (holder cost rides neither leg's ledger credit) is
    // below even the stood-down 2.8 gate, so the deficit queue keeps running.
    assert!(drive_ram_release(&mut state, &mut rx).await);
    assert_eq!(
        state.jobs[&job_id]
            .victim
            .as_ref()
            .expect("deficit next")
            .model,
        "gpu-a"
    );
    assert_eq!(
        runtime.kills(),
        1,
        "the holder only — gpu-a's kill runs after its turn"
    );
    assert_eq!(runtime.spawns(), 0);
    // Release two: gpu-a's 1.5 GiB lands the honest ledger at 3.0 against
    // the 2.8 gate — the early stop fires with gpu-b STILL queued. The
    // review's point: it must clear the deficit queue and continue into the
    // reclaim queue, never settle past it.
    assert!(drive_ram_release(&mut state, &mut rx).await);
    assert_eq!(
        runtime.kills(),
        2,
        "the holder and gpu-a, each on its own turn"
    );
    let job = &state.jobs[&job_id];
    assert!(
        job.victims.is_empty(),
        "the early stop cleared the deficit queue"
    );
    assert!(
        job.reclaim.victims.is_empty(),
        "the reclaim queue drained forward, it did not settle"
    );
    assert_eq!(
        job.victim
            .as_ref()
            .expect("reclaim next, not a spawn")
            .model,
        "mid-cpu",
        "the early stop continues into the reclaim queue"
    );
    assert_eq!(runtime.spawns(), 0, "no spawn past the queue");
    assert_eq!(
        state.slots[2].occupant.as_ref().expect("spared").model,
        "gpu-b"
    );
    // Release three: the reclaim row. Queue empty on both ladders — now the
    // settle window, then the one spawn.
    assert!(drive_ram_release(&mut state, &mut rx).await);
    assert_eq!(
        runtime.kills(),
        3,
        "holder, gpu-a, mid-cpu — gpu-b is spared alive"
    );
    match next_ram_event(&mut rx, job_id, "the settle verdict").await {
        super::Event::Settled(Ok(())) => state.apply_event(job_id, super::Event::Settled(Ok(()))),
        other => panic!("the ledger-only witness waves the window: {other:?}"),
    }
    match next_ram_event(&mut rx, job_id, "the spawn").await {
        super::Event::Spawned(Ok(spawned)) => {
            assert_eq!(spawned.port, 18_160);
            state.apply_event(job_id, super::Event::Spawned(Ok(spawned)));
        }
        other => panic!("the spawn answers: {other:?}"),
    }
    state.apply_event(
        job_id,
        super::Event::Probed(crate::engine::Probe::Ready, None),
    );
    let job = &state.jobs[&job_id];
    assert_eq!(
        job.victims_released,
        vec!["gpu-a".to_string(), "mid-cpu".to_string()],
        "the release ORDER: deficit before reclaim, holder never listed"
    );
    assert_eq!(
        runtime.kills(),
        3,
        "exactly three releases, one per dead engine"
    );
    assert_eq!(
        runtime.spawns(),
        1,
        "one engine, after both queues and the window"
    );
    assert_eq!(
        state.slots[0].occupant.as_ref().expect("installs").model,
        "expert-mix"
    );
    assert!(state.slots[1].occupant.is_none());
    assert_eq!(
        state.slots[2].occupant.as_ref().expect("spared").model,
        "gpu-b"
    );
    assert!(state.slots[3].occupant.is_none(), "the reclaim row is gone");
    assert_eq!(
        state.slots[4].occupant.as_ref().expect("stays").model,
        "small-cpu"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// F11 (W2): `min_turn` applies unchanged to both planners. A young Ram
/// holder is a named blocker of a RAM shortage, in the card leg's own
/// wording class; and the guard never walls off a same-slot RAM
/// replacement — releasing the named target of the request is the request
/// itself, so the deficit that remains is covered from another slot.
/// Falsifier: on the pre-fix tree the RAM refusal never had the `min_turn`
/// wording (leg one red) and never admitted a drain at all (leg two red).
/// A regression that guards the same-slot credit with the young resident's
/// clock (subtracting the credit instead of filtering candidates) turns leg
/// two into a refusal — red.
#[tokio::test]
async fn a_young_ram_holder_is_a_min_turn_blocker_never_a_same_slot_wall() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("ram-min-turn");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let seat = |model: &str, ram: f64, port: u16| Occupant {
        model: model.to_string(),
        need: Gibibytes(0.0),
        ram_need: Some(Gibibytes(ram)),
        port,
        ticket: fold_ticket(port),
        loaded_at: Instant::now(),
        launched_at_wall: std::time::SystemTime::now(),
        spawn_config: None,
        stale_config: false,
        serve_id: None,
        measure: None,
        live_measured_gb: None,
    };
    let write_row = |name: &str, ram_gb: &str, min_turn: Option<&str>| {
        let yaml = dir.join(format!("{name}.yaml"));
        let guarded = min_turn.map_or(String::new(), |m| format!("min_turn: {m}\n"));
        std::fs::write(
            &yaml,
            format!(
                "engine: llama\ncategory: work\nram_gb: {ram_gb}\ncpu_offload: true\n{guarded}"
            ),
        )
        .expect("seed");
        rescan_entry(&yaml, name)
    };
    // Leg one: the young cross-slot Ram holder blocks, named in the
    // card leg's min_turn wording class; nothing else yields.
    {
        let load = write_row("load", "20", None);
        let young = write_row("aux-cpu", "10", Some("3600"));
        let mut state = fold_state(&dir);
        state.catalog = vec![load.clone(), young];
        state.slots = vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: None,
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: Some(seat("aux-cpu", 10.0, 18_131)),
                job: None,
            },
        ];
        let job_id = state.register_queued_load(0, &load, Vec::new());
        state.start_queued_load(job_id, 0, load.clone());
        state.finish_admit(
            job_id,
            None,
            Some(crate::witness::RamSample {
                total_gib: 64.0,
                used_gib: 44.0,
            }),
        );
        let job = &state.jobs[&job_id];
        let error = job
            .error
            .as_ref()
            .expect("the young guard leaves no victim");
        let SchedulerError::Admission(AdmissionError::InsufficientRam {
            victims_considered,
            blockers,
            ..
        }) = error
        else {
            panic!("a RAM shortage stays the RAM class: {error:?}");
        };
        assert!(
            victims_considered.is_empty(),
            "a holder inside its guard is a blocker, never a candidate: {victims_considered:?}"
        );
        assert!(
            blockers
                .iter()
                .any(|b| b.model == "aux-cpu" && b.reason.contains("min_turn guard")),
            "the young Ram holder is a named blocker: {blockers:?}"
        );
        assert!(job.terminal);
        assert!(
            state.slots[1].occupant.is_some(),
            "nothing killed: the guarded holder stays seated"
        );
    }
    // Leg two: the young guard never blocks the same-slot replacement. The
    // same-slot credit still stands (the guard protects a reload cost the
    // replacement itself IS paying), and the small remaining deficit is
    // covered by the unguarded Ram resident in AUX — the load drains.
    {
        let load = write_row("load30", "30", None);
        let young = write_row("work-cpu", "10", Some("3600"));
        let mut state = fold_state(&dir);
        state.adapters.insert(
            "llama".to_string(),
            Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
        );
        state.catalog = vec![load.clone(), young];
        state.slots = vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(seat("work-cpu", 10.0, 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: Some(seat("aux-cpu", 10.0, 18_131)),
                job: None,
            },
        ];
        let job_id = state.register_queued_load(0, &load, Vec::new());
        state.start_queued_load(job_id, 0, load.clone());
        // free = 64 - 8 - 40 + 10 (own-slot credit) = 26 < need 30: the
        // 4 GiB deficit comes from AUX; the young own-slot resident is the
        // replacement, not a blocker and not a candidate.
        state.finish_admit(
            job_id,
            None,
            Some(crate::witness::RamSample {
                total_gib: 64.0,
                used_gib: 40.0,
            }),
        );
        let job = &state.jobs[&job_id];
        assert!(job.error.is_none(), "{:?}", job.error);
        assert!(!job.terminal, "the young guard never walls the same slot");
        assert_eq!(job.path, Some(AdmissionPath::Drain));
        let victim = job.victim.as_ref().expect("the replacement rides the row");
        assert_eq!(victim.model, "work-cpu", "the named target first");
        assert!(!victim.evicted, "a same-slot release, not an eviction");
        assert!(
            job.victims.iter().any(|v| v.model == "aux-cpu"),
            "the covering victim stays queued: {:?}",
            job.victims
                .iter()
                .map(|v| v.model.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(job.state, SwapState::Dispose);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The RAM ladder's no-basis rule: a CPU row with neither `ram_gb` nor a
/// GGUF size is under-specified, not guessed at (the estimate exists only
/// where the weights file's size is known).
#[test]
fn a_ram_row_without_any_basis_is_under_specified() {
    use super::AdmissionError;
    let entry = ModelEntry::from_flat(
        &BTreeMap::from([
            ("engine".to_string(), "llama".to_string()),
            ("cpu_offload".to_string(), "true".to_string()),
        ]),
        "cpu",
    )
    .expect("valid");
    assert_eq!(entry.resource(), crate::model::Resource::Ram);
    assert!(
        matches!(
            super::ActorState::ram_booking(&entry),
            Err(AdmissionError::UnderSpecified)
        ),
        "no `ram_gb`, no GGUF size: the gate refuses rather than guess"
    );
    let sized = ModelEntry {
        gguf_size_gb: Some(20.0),
        ..entry
    };
    assert_eq!(
        super::ActorState::ram_booking(&sized).expect("the estimate has a basis"),
        Gibibytes(22.0),
        "the weights page in, plus the working set: size x 1.0 + 2"
    );
}

// --- The expert-offload RAM reclaim

/// The greedy expert-offload row: `cost: greedy` on an engine carrying the
/// `expert_offload` policy cell. Written as the scan writes it, read back
/// through the real parser like every other pin.
fn expert_row(dir: &std::path::Path, name: &str, engine: &str, greedy: bool) -> ModelEntry {
    let yaml = dir.join(format!("{name}.yaml"));
    std::fs::write(
        &yaml,
        format!(
            "engine: {engine}\ncategory: work\n{}\n",
            if greedy {
                "cost: greedy"
            } else {
                "vram_gb: 12"
            }
        ),
    )
    .expect("seed");
    rescan_entry(&yaml, name)
}

/// A card reading with the whole 72 GiB free: the reclaim rule is not a
/// deficit answer, so every pin drives a card the gate was never short on.
fn free_card() -> crate::budget::MemorySample {
    crate::budget::MemorySample {
        total_gib: 72.0,
        used_gib: 0.0,
        processes: Vec::new(),
        attribution: crate::budget::AttributionQuality::Unavailable,
    }
}

/// State armed for the reclaim pins: adapters for every engine a pin's row
/// may name, so `preflight_spec` never confuses the leg under test.
fn expert_state(dir: &std::path::Path) -> ActorState {
    let mut state = fold_state(dir);
    for engine in ["maya", "llama"] {
        state.adapters.insert(
            engine.to_string(),
            Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
        );
    }
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state
}

/// The expert-offload RAM reclaim: a `cost: greedy` admit on
/// an expert-offload engine queues every CPU resident STRICTLY above the
/// configured threshold, largest booked footprint first, through the drain
/// rung — while the card gate passes as it always did (free 70 of 72, no
/// deficit, no host-RAM gate invented). The first queued victim rides the
/// release slot the moment the admit lands.
/// Falsifier: make the comparison `>=` and `small-cpu` joins the queue; drop
/// the descending sort and `victim` reads `mid-cpu`; drop the `ram_need`
/// filter and the GPU resident joins. Each fails here alone.
#[tokio::test]
async fn a_greedy_expert_offload_admit_queues_large_cpu_residents_largest_first() {
    let dir = crate::testutil::temp_dir("expert-reclaim-plan");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = expert_state(&dir);
    let entry = expert_row(&dir, "expert-m", "maya", true);
    assert_eq!(entry.cost, crate::model::CostClass::Greedy);
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_140,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_141,
            occupant: Some(ram_settle_seat("mid-cpu", 0.0, Some(8.0), 18_141)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 2,
            engine_port: 18_142,
            occupant: Some(ram_settle_seat("big-cpu", 0.0, Some(12.0), 18_142)),
            job: None,
        },
        Slot {
            category: CategoryId::Emb,
            index: 3,
            engine_port: 18_143,
            occupant: Some(ram_settle_seat("small-cpu", 0.0, Some(3.0), 18_143)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(job_id, Ok(free_card()));
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert!(
        !job.terminal,
        "the gate passed; the load drains, it does not refuse"
    );
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    let names: Vec<&str> = job
        .reclaim
        .victims
        .iter()
        .map(|v| v.model.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["mid-cpu"],
        "big-cpu already left the queue for the release slot, largest first"
    );
    assert_eq!(
        job.reclaim.gib, 20.0,
        "8 + 12 booked host GiB, never card GiB"
    );
    assert!(job.reclaim.skipped.is_empty());
    let victim = job
        .victim
        .as_ref()
        .expect("the first victim rides the slot");
    assert_eq!(victim.model, "big-cpu");
    assert_eq!(victim.slot, 2);
    assert!(victim.evicted, "a cross-category eviction");
    assert_eq!(
        victim.cost, 0.0,
        "a CPU release books zero card bytes — the drain re-check must never credit host GiB"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reclaim rule's own boundaries:
/// `0` disables the whole rule; a llama row never triggers it whatever it
/// costs; a `fixed` row on the same expert engine never triggers it; and a
/// holder sitting EXACTLY on the threshold stays (the comparison is strictly
/// greater). Every leg reads `FitsNow` with an empty reclaim queue — the
/// posture, not the numbers, is what differs from the pin above.
/// Falsifier: flip the boundary to `>=` and leg four queues; read the engine
/// name instead of the policy cell and leg two drains rows for llama; drop
/// the cost guard and leg three does.
#[tokio::test]
async fn a_reclaim_never_plans_below_zero_at_threshold_or_for_another_row() {
    let dir = crate::testutil::temp_dir("expert-reclaim-bounds");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let build = |state: &mut ActorState, entry: &ModelEntry, holder_gib: f64| {
        state.catalog = vec![entry.clone()];
        state.slots = vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_144,
                occupant: None,
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_145,
                occupant: Some(ram_settle_seat("holder-cpu", 0.0, Some(holder_gib), 18_145)),
                job: None,
            },
        ];
        let job_id = state.register_queued_load(0, entry, Vec::new());
        state.start_queued_load(job_id, 0, entry.clone());
        state.finish_admit_card(job_id, Ok(free_card()));
        job_id
    };
    // Leg one: the threshold reads `0` — disabled, even for a 40 GiB holder.
    let mut state = expert_state(&dir);
    state.cfg.expert_offload_evict_cpu_above_gb = 0.0;
    let entry = expert_row(&dir, "off-m", "maya", true);
    let job_id = build(&mut state, &entry, 40.0);
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    assert!(job.reclaim.victims.is_empty() && job.victim.is_none());
    // Leg two: the same greedy row on llama — no policy cell, no rule.
    let mut state = expert_state(&dir);
    let entry = expert_row(&dir, "greedy-l", "llama", true);
    let job_id = build(&mut state, &entry, 40.0);
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    assert!(job.reclaim.victims.is_empty() && job.victim.is_none());
    // Leg three: the expert engine with a `fixed` row — the rule is greedy-only.
    let mut state = expert_state(&dir);
    let entry = expert_row(&dir, "fixed-m", "maya", false);
    let job_id = build(&mut state, &entry, 40.0);
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    assert!(job.reclaim.victims.is_empty() && job.victim.is_none());
    // Leg four: exactly the default 4.0 GiB threshold — `>` means stays.
    let mut state = expert_state(&dir);
    let entry = expert_row(&dir, "edge-m", "maya", true);
    let job_id = build(&mut state, &entry, 4.0);
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    assert!(job.reclaim.victims.is_empty() && job.reclaim.skipped.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// DOCTRINE's CPU posture honored: a GPU resident is never a reclaim victim, however huge its
/// footprint — it holds no booked host RAM, so the resource filter drops it
/// before any other question, and it is not even named as skipped. The CPU
/// resident beside it is queued normally.
/// Falsifier: source candidates from `need` (the card figure) instead of
/// `ram_need` and `gpu-resident` joins the queue red here.
#[tokio::test]
async fn a_gpu_resident_is_never_an_expert_offload_reclaim_victim() {
    let dir = crate::testutil::temp_dir("expert-reclaim-gpu-stays");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = expert_state(&dir);
    let entry = expert_row(&dir, "expert-g", "maya", true);
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_146,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_147,
            occupant: Some(ram_settle_seat("gpu-resident", 40.0, None, 18_147)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 2,
            engine_port: 18_148,
            occupant: Some(ram_settle_seat("big-cpu", 0.0, Some(25.0), 18_148)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(job_id, Ok(free_card()));
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    let all = format!(
        "{:?} {:?} {:?}",
        job.reclaim
            .victims
            .iter()
            .map(|v| &v.model)
            .collect::<Vec<_>>(),
        job.reclaim.skipped,
        job.victim.as_ref().map(|v| v.model.clone()),
    );
    assert!(
        !all.contains("gpu-resident"),
        "a GPU resident is never named by the reclaim, victim or skipped: {all}"
    );
    assert!(
        job.reclaim.victims.is_empty(),
        "the one candidate drained to the release slot"
    );
    assert_eq!(
        job.victim
            .as_ref()
            .expect("the queued victim rides the slot")
            .model,
        "big-cpu"
    );
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The "skip it rather than fail the load" rule: a holder the `evictable_by` matrix protects
/// and a holder inside its `min_turn` guard are SKIPPED with a named reason —
/// the load still admits `FitsNow` and kills nothing — and one log line names
/// the threshold, the (empty) evicted set, and every skipped row with its
/// reason.
/// Falsifier: treat a matrix miss as a blocker and the load refuses red here;
/// treat a young holder as eligible and it joins the queue; drop the
/// skipped field from the log and the leg below finds no reason text.
#[tokio::test]
async fn a_blocked_or_young_cpu_holder_is_skipped_with_its_reason_logged() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("expert-reclaim-skips");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = expert_state(&dir);
    // The shipped matrix opens RP to WORK; close it — the operator's file is
    // king and the skip must follow the effective config, not a remembered one.
    if let Some(rp) = state
        .cfg
        .categories
        .iter_mut()
        .find(|cat| cat.id == CategoryId::Rp)
    {
        rp.evictable_by = Vec::new();
    }
    let entry = expert_row(&dir, "expert-log", "maya", true);
    let (map, problems) = crate::model::scan::parse_flat_yaml(
        "engine: llama\ncategory: aux\nram_gb: 8\ncpu_offload: true\nmin_turn: 3600\n",
    );
    assert!(problems.is_empty(), "{problems:?}");
    let guarded = ModelEntry::from_flat(&map, "young-cpu").expect("entry");
    state.catalog = vec![entry.clone(), guarded];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_149,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_150,
            occupant: Some(ram_settle_seat("young-cpu", 0.0, Some(8.0), 18_150)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 2,
            engine_port: 18_151,
            occupant: Some(ram_settle_seat("blocked-cpu", 0.0, Some(20.0), 18_151)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(job_id, Ok(free_card()));
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(
        job.path,
        Some(AdmissionPath::FitsNow),
        "skips drain nothing: no victim, no rung change"
    );
    assert!(job.reclaim.victims.is_empty());
    let skipped: Vec<&str> = job
        .reclaim
        .skipped
        .iter()
        .map(|(_, reason)| reason.as_str())
        .collect();
    assert_eq!(job.reclaim.skipped.len(), 2, "{skipped:?}");
    assert!(skipped.iter().any(|r| r.contains("min_turn")));
    assert!(skipped.iter().any(|r| r.contains("evict")));
    let guard = capture.lock().expect("capture lock");
    let logged = guard
        .iter()
        .filter(|(message, _)| message.contains("expert-offload admit"))
        .filter(|(_, fields)| {
            fields
                .get("model")
                .is_some_and(|m| m.contains("expert-log"))
        })
        .collect::<Vec<_>>();
    assert_eq!(logged.len(), 1, "one decision, one line: {logged:?}");
    let (_, fields) = logged[0];
    assert!(
        fields
            .get("skipped")
            .is_some_and(|s| s.contains("young-cpu") && s.contains("blocked-cpu")),
        "every skipped row is named with its reason: {fields:?}"
    );
    assert!(
        fields.get("evicted").is_some_and(|e| e == "[]"),
        "nothing was evicted, and the log says so: {fields:?}"
    );
    assert!(fields
        .get("threshold_gib")
        .is_some_and(|t| t.starts_with('4')));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The drain end-to-end: with
/// the card gate passing on a free card, the greedy expert admit drains its
/// CPU queue one release at a time — largest first — each victim killed only
/// after the previous release is confirmed, the GPU resident never touched,
/// the settle window closing before the single spawn lands. No card GiB is
/// ever credited to the drain re-check, and the load installs.
/// Falsifier: settle instead of draining the reclaim queue and the second
/// kill never happens (kills == 1); spawn before the settle and the spawn
/// counter moves while the slots still hold; kill both up front and the
/// mid-drain occupant assert goes red.
#[tokio::test]
async fn the_expert_offload_drain_kills_only_large_cpu_residents_before_the_spawn() {
    let dir = crate::testutil::temp_dir("expert-reclaim-drain");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "maya".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let runtime = Arc::new(SettleRuntime {
        confirm: true,
        ..SettleRuntime::default()
    });
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.to_path_buf(),
        adapters,
        Arc::clone(&runtime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = expert_row(&dir, "expert-go", "maya", true);
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_152,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_153,
            occupant: Some(ram_settle_seat("big-cpu", 0.0, Some(12.0), 18_153)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 2,
            engine_port: 18_154,
            occupant: Some(ram_settle_seat("small-cpu", 0.0, Some(8.0), 18_154)),
            job: None,
        },
        Slot {
            category: CategoryId::Emb,
            index: 3,
            engine_port: 18_155,
            occupant: Some(ram_settle_seat("gpu-stays", 40.0, None, 18_155)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    // The dispatched reading arrives on the actor's channel — ledger-only, so
    // the gate budgets the ledger, the same shape the real dispatch carries.
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the admit task dispatches its reading")
        .expect("the actor channel is open");
    match msg {
        ActorMsg::Event { job, event } => match event {
            super::Event::Admitted(card, ram) => {
                assert!(ram.is_none(), "a VRAM row never samples the host");
                state.apply_event(job, super::Event::Admitted(card, ram));
            }
            other => panic!("the admit task sends its sample: {other:?}"),
        },
        _ => panic!("expected an event"),
    }
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    assert_eq!(job.reclaim.gib, 20.0);
    assert_eq!(job.victim.as_ref().expect("largest first").model, "big-cpu");
    // Release one: the big holder goes, the small one waits its turn, and
    // the GPU resident is untouched by design.
    assert!(drive_ram_release(&mut state, &mut rx).await);
    assert!(
        state.slots[1].occupant.is_none(),
        "the confirmed release cleared its slot"
    );
    assert_eq!(
        state.slots[2].occupant.as_ref().expect("waits").model,
        "small-cpu"
    );
    assert_eq!(
        runtime.kills(),
        1,
        "one kill while the first release drained"
    );
    assert_eq!(
        runtime.spawns(),
        0,
        "the spawn waits behind the whole queue"
    );
    // Release two: the queue is empty, so the settle window opens next.
    assert!(drive_ram_release(&mut state, &mut rx).await);
    assert!(state.slots[2].occupant.is_none());
    assert_eq!(runtime.kills(), 2, "every queued victim, exactly once");
    match next_ram_event(&mut rx, job_id, "the settle verdict").await {
        super::Event::Settled(Ok(())) => state.apply_event(job_id, super::Event::Settled(Ok(()))),
        other => panic!("the ledger-only witness waves the window: {other:?}"),
    }
    match next_ram_event(&mut rx, job_id, "the spawn").await {
        super::Event::Spawned(Ok(spawned)) => {
            assert_eq!(spawned.port, 18_152);
            state.apply_event(job_id, super::Event::Spawned(Ok(spawned)));
        }
        other => panic!("the spawn answers: {other:?}"),
    }
    state.apply_event(
        job_id,
        super::Event::Probed(crate::engine::Probe::Ready, None),
    );
    assert_eq!(
        runtime.spawns(),
        1,
        "exactly one engine, after the queue and the window"
    );
    assert_eq!(runtime.kills(), 2, "nothing else was killed");
    let occupant = state.slots[0].occupant.as_ref().expect("the load installs");
    assert_eq!(occupant.model, "expert-go");
    assert!(
        state.slots[3]
            .occupant
            .as_ref()
            .expect("the GPU resident stays")
            .model
            == "gpu-stays",
        "a GPU resident outlives every expert-offload drain"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
