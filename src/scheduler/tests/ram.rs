use super::*;

// ---------------------------------------------------------------------------
// The RAM ladder: CPU-resident rows admit on
// host RAM and never touch the card; the two ledgers keep their rows apart.
// ---------------------------------------------------------------------------

/// The never-answers host witness, the RAM mirror of `LedgerOnlyWitness` in
/// shape — but not in privilege: RAM has no ledger mode, its `None` refuses
/// fail-closed (DOCTRINE 4).
struct SilentRamWitness;

impl crate::witness::RamWitness for SilentRamWitness {
    fn sample(&self) -> Option<crate::witness::RamSample> {
        None
    }
}

/// The garbage host witness: a finite negative `used` (the Windows reading is
/// a float subtraction of the reported figures, and the witness trait is a
/// public injection seam whose honesty rule is `None`, not a figure range),
/// so the decision point — not the open set of drivers — must fail closed.
struct NegativeRamWitness;

impl crate::witness::RamWitness for NegativeRamWitness {
    fn sample(&self) -> Option<crate::witness::RamSample> {
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: -5.0,
        })
    }
}

/// Sane for its first reading (the admit dispatch), garbage for every reading
/// after — the mid-window flip the RAM settle window must ride out.
#[derive(Default)]
struct NegativeAfterFirstRamWitness {
    reads: std::sync::atomic::AtomicUsize,
}

impl crate::witness::RamWitness for NegativeAfterFirstRamWitness {
    fn sample(&self) -> Option<crate::witness::RamSample> {
        let n = self
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: if n == 0 { 44.0 } else { -5.0 },
        })
    }
}

/// Slots for the RAM pins: an empty WORK slot and an empty AUX slot.
fn ram_slots() -> Vec<Slot> {
    vec![
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
            occupant: None,
            job: None,
        },
    ]
}

/// The CPU-resident fixture row: llama (the one engine whose knob table reads
/// `cpu_offload`), the truth-set word, and a declared footprint — read back
/// through the real parser like every other pin.
fn write_cpu_row_yaml(dir: &std::path::Path, name: &str, ram_gb: &str) -> std::path::PathBuf {
    let yaml = dir.join(format!("{name}.yaml"));
    std::fs::write(
        &yaml,
        format!("engine: llama\ncategory: work\nram_gb: {ram_gb}\ncpu_offload: true\n"),
    )
    .expect("seed");
    yaml
}

/// Falsifier (1): a CPU-resident row with ample host RAM admits on its own
/// ladder, books ZERO VRAM, and the next GPU admit sees the card exactly as
/// if it had never been asked.
/// Falsifier: route a Ram row through the VRAM leg (need 10, booked as VRAM)
/// and the second leg's `free_at_admit` drops by 10 — pin goes red.
#[tokio::test]
async fn a_cpu_row_admits_on_ram_and_books_no_vram() {
    let dir = crate::testutil::temp_dir("ram-admit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "10");
    let gpu_yaml = dir.join("gpu.yaml");
    std::fs::write(&gpu_yaml, "engine: mock\ncategory: aux\nvram_gb: 12\n").expect("seed");
    let mut state = fold_state(&dir);
    state.adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    let gpu = rescan_entry(&gpu_yaml, "gpu");
    assert_eq!(cpu.resource(), crate::model::Resource::Ram);
    state.catalog = vec![cpu.clone(), gpu.clone()];
    state.slots = ram_slots();
    let job_id = state.register_queued_load(0, &cpu, Vec::new());
    state.start_queued_load(job_id, 0, cpu.clone());
    assert_eq!(
        state.jobs[&job_id].need,
        Some(Gibibytes(0.0)),
        "a RAM row books zero VRAM"
    );
    assert_eq!(state.jobs[&job_id].ram_need, Some(Gibibytes(10.0)));
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 20.0,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    assert_eq!(
        job.state,
        SwapState::Spawn,
        "the admitted tail ran to the spawn commit"
    );
    assert_eq!(
        state.ledger_sum(),
        Gibibytes(0.0),
        "no VRAM was ever debited"
    );
    // The next GPU admit sees the card untouched: the same sample, the same
    // figures — 72 total, 2 reserve, 30 used, nothing else on the ledger or
    // in flight (the RAM row spends none of the card's bytes).
    let gpu_id = state.register_queued_load(1, &gpu, Vec::new());
    state.start_queued_load(gpu_id, 1, gpu.clone());
    state.finish_admit_card(
        gpu_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 30.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let gpu_job = &state.jobs[&gpu_id];
    assert!(gpu_job.error.is_none(), "{:?}", gpu_job.error);
    assert_eq!(
        gpu_job.free_at_admit,
        Some(Gibibytes(40.0)),
        "the CPU row must not have spent a byte of the card"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (2): a CPU row over the RAM floor refuses naming its need,
/// free, reserve and blockers with the standard wait fields — the same
/// contract `Insufficient` has, judged on host RAM (operator ruling).
/// The named blocker here is a GPU resident: its host RAM
/// rides the witness and is never under the RAM gate's control.
#[tokio::test]
async fn a_cpu_row_over_the_ram_floor_refuses_naming_its_numbers() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("ram-refuse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "10");
    let mut state = fold_state(&dir);
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    state.catalog = vec![cpu.clone()];
    let gpu_ticket = fold_ticket(18_131);
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
            occupant: Some(Occupant {
                model: "gpu-resident".to_string(),
                need: Gibibytes(6.0),
                ram_need: None,
                port: 18_131,
                ticket: gpu_ticket.clone(),
                loaded_at: Instant::now(),
                launched_at_wall: std::time::SystemTime::now(),
                spawn_config: None,
                stale_config: false,
                serve_id: None,
                measure: None,
                live_measured_gb: None,
            }),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &cpu, Vec::new());
    state.start_queued_load(job_id, 0, cpu.clone());
    // free = 64 - 8 (reserve) - 50 (used) = 6 < need 10.
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 50.0,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.terminal, "the refusal terminalizes the row");
    let error = job.error.as_ref().expect("the refusal rides the row");
    let SchedulerError::Admission(AdmissionError::InsufficientRam {
        need,
        free,
        reserve,
        blockers,
        wait_hint,
        victims_considered,
        ..
    }) = error
    else {
        panic!("the RAM refusal has its own class: {error:?}");
    };
    assert_eq!(
        (*need, *free, *reserve),
        (Gibibytes(10.0), Gibibytes(6.0), Gibibytes(8.0))
    );
    let msg = error.to_string();
    assert!(
        msg.contains("need 10.0 GiB")
            && msg.contains("free 6.0 GiB")
            && msg.contains("reserve 8.0 GiB"),
        "the refusal quotes the numbers it compared: {msg}"
    );
    assert!(
        blockers
            .iter()
            .any(|b| b.model == "gpu-resident" && b.reason.contains("GPU")),
        "the GPU resident's host RAM is a named blocker: {blockers:?}"
    );
    assert!(
        victims_considered.is_empty(),
        "the RAM gate plans no eviction: {victims_considered:?}"
    );
    assert_eq!(wait_hint.queue_position, None, "{wait_hint:?}");
    assert_eq!(wait_hint.expected_release_s, None, "{wait_hint:?}");
    assert!(
        !msg.contains("card holds"),
        "a RAM refusal never blames the card: {msg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (3/4, the VRAM side): the card's victim planner never considers
/// — and never even blames — a RAM resident, even when its category would
/// be evictable by the requesting one. A RAM occupant holds no VRAM to take;
/// the refusal is a plain shortage with an empty candidate set.
#[tokio::test]
async fn the_vram_planner_never_considers_or_blames_a_ram_occupant() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("vram-planner-skip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let gpu_yaml = dir.join("gpu.yaml");
    std::fs::write(&gpu_yaml, "engine: mock\ncategory: work\nvram_gb: 40\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let gpu = rescan_entry(&gpu_yaml, "gpu");
    state.catalog = vec![gpu.clone()];
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
            occupant: Some(Occupant {
                model: "aux-cpu".to_string(),
                need: Gibibytes(0.0),
                ram_need: Some(Gibibytes(10.0)),
                port: 18_131,
                ticket: fold_ticket(18_131),
                loaded_at: Instant::now(),
                launched_at_wall: std::time::SystemTime::now(),
                spawn_config: None,
                stale_config: false,
                serve_id: None,
                measure: None,
                live_measured_gb: None,
            }),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &gpu, Vec::new());
    state.start_queued_load(job_id, 0, gpu.clone());
    // free = 72 - 2 - 40 = 30 < need 40: a 10 GiB deficit, and the only
    // other holder is the RAM resident — the planner must find nothing.
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 40.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    let error = job.error.as_ref().expect("the shortage refuses");
    assert!(
        matches!(
            error,
            SchedulerError::Admission(AdmissionError::Insufficient { .. })
        ),
        "a VRAM shortage stays the VRAM class: {error:?}"
    );
    let msg = error.to_string();
    assert!(
        !msg.contains("aux-cpu"),
        "the RAM resident is neither victim nor blocker: {msg}"
    );
    match error {
        SchedulerError::Admission(AdmissionError::Insufficient {
            victims_considered,
            blockers,
            ..
        }) => {
            assert!(
                victims_considered.is_empty(),
                "no VRAM candidate existed: {victims_considered:?}"
            );
            assert!(
                blockers.is_empty(),
                "no VRAM holder existed to blame: {blockers:?}"
            );
        }
        _ => unreachable!("checked above"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (5): a `whole` GPU request is never blocked by a RAM resident,
/// and a RAM resident never enters its holder list — the whole-machine gate
/// is a card posture. The closure legs
/// add the in-flight half: an admitted-not-installed RAM row is likewise
/// neither a blocker nor a holder — and when a real card holder forces the
/// refusal, the holder list names the GPU resident only, in-flight RAM row
/// and seated RAM resident both absent.
/// Falsifier: delete the `job.ram_need.is_some()` row filter in
/// `whole_machine_holders` and the in-flight RAM row joins the holders —
/// leg one refuses an admit that must pass, leg two names `cpu` beside
/// `gpu-holder`. Delete the occupant-leg filter and the seated `aux-cpu`
/// does the same in both legs.
#[tokio::test]
async fn a_whole_gpu_row_never_sees_a_ram_resident_as_a_holder() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("whole-ram-holder");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("whole.yaml");
    std::fs::write(
        &yaml,
        "engine: mock\ncategory: work\nvram_gb: 12\nlaunch_only_with_at_least_vram_free: whole\n",
    )
    .expect("seed");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "10");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "whole");
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    state.catalog = vec![entry.clone(), cpu.clone()];
    let seat = |model: &str, need: f64, ram_need: Option<f64>, port: u16| Occupant {
        model: model.to_string(),
        need: Gibibytes(need),
        ram_need: ram_need.map(Gibibytes),
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
            occupant: Some(seat("aux-cpu", 0.0, Some(10.0), 18_131)),
            job: None,
        },
        Slot {
            category: CategoryId::Rp,
            index: 2,
            engine_port: 18_132,
            occupant: None,
            job: None,
        },
    ];
    // The holder-scan leg: an admitted-not-installed RAM row — booked, reserving,
    // never installed, the row half of the holder scan.
    let ram_row = state.register_queued_load(2, &cpu, Vec::new());
    state.jobs.get_mut(&ram_row).expect("row").need = Some(Gibibytes(0.0));
    state.jobs.get_mut(&ram_row).expect("row").ram_need = Some(Gibibytes(5.0));
    state.step(ram_row, SwapState::Admit);
    let sample = || {
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 0.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        })
    };
    // Leg one: no card holder exists — a seated RAM resident and an
    // in-flight RAM row block nothing and are listed nowhere.
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(job_id, sample());
    let job = &state.jobs[&job_id];
    assert!(
        !matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::WholeMachineBusy { .. }
            ))
        ),
        "a RAM resident and an in-flight RAM row must not block the whole gate: {:?}",
        job.error
    );
    assert!(job.error.is_none(), "{:?}", job.error);
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    // Leg two: a GPU holder forces the refusal, and the honest holder list
    // names exactly it — no RAM name may appear at all (the seated
    // `aux-cpu` and the in-flight `cpu` row included).
    state.slots[2].occupant = Some(seat("gpu-holder", 6.0, None, 18_132));
    let job_two = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_two, 0, entry.clone());
    state.finish_admit_card(job_two, sample());
    let error = state.jobs[&job_two]
        .error
        .as_ref()
        .expect("a GPU holder keeps the machine from the whole request")
        .to_string();
    assert!(
        error.contains("gpu-holder"),
        "the GPU holder is named: {error}"
    );
    assert!(
        !error.contains("cpu"),
        "no RAM holder, resident or in-flight row, is listed: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (8): the whole dispatched path fails closed when the host
/// witness never answers — the Admit task sends no card sample for a RAM row
/// at all, a silent host witness answers `None`, and the gate refuses
/// without guessing (DOCTRINE 4).
#[tokio::test]
async fn a_cpu_row_refuses_closed_when_the_host_witness_is_silent() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("ram-witness-down");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "10");
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.to_path_buf(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.ram_witness = Arc::new(SilentRamWitness);
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    state.catalog = vec![cpu.clone()];
    state.slots = ram_slots();
    let job_id = state.register_queued_load(0, &cpu, Vec::new());
    state.start_queued_load(job_id, 0, cpu.clone());
    let msg = rx
        .recv()
        .await
        .expect("the admit task dispatched its reading");
    match msg {
        ActorMsg::Event { job, event } => match event {
            super::Event::Admitted(card, ram) => {
                assert!(card.is_none(), "a RAM row never samples the card");
                assert!(ram.is_none(), "the silent host witness answers nothing");
                state.apply_event(job, super::Event::Admitted(card, ram));
            }
            other => panic!("the admit task sends its sample: {other:?}"),
        },
        _ => panic!("expected an event"),
    }
    let job = &state.jobs[&job_id];
    assert!(
        job.terminal,
        "a missing host reading is a refusal, never a wait"
    );
    let error = job.error.as_ref().expect("the refusal rides the row");
    assert!(
        matches!(
            error,
            SchedulerError::Admission(AdmissionError::WitnessUnavailable { reason })
                if reason.contains("ram witness")
        ),
        "the refusal names the RAM witness: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (the RAM twin of the card gate's negative-reading refusal, the
/// DECISIONS 2026-11-26 fail-closed family): a finite negative host `used` is
/// garbage, and `free_gib()` clamps only its floor — the reading INFLATES the
/// compared free (64 + 8-side reserve and all) past every budget. The RAM gate
/// must refuse it as unreadable BEFORE the arithmetic, quoting the raw figure,
/// exactly like the card gate. The honesty rule of the witness seam is `None`,
/// not a figure range, so the decision point itself carries the guard.
/// Falsifier: delete the negative-reading guard in `finish_admit_ram` and the
/// seeded row rides the inflated free to a FitsNow admit — the terminal and
/// refusal-shape legs here go red.
#[tokio::test]
async fn a_negative_host_reading_refuses_at_the_ram_gate_before_the_arithmetic() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("ram-witness-garbage");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "10");
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.to_path_buf(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.ram_witness = Arc::new(NegativeRamWitness);
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    state.catalog = vec![cpu.clone()];
    state.slots = ram_slots();
    let job_id = state.register_queued_load(0, &cpu, Vec::new());
    state.start_queued_load(job_id, 0, cpu.clone());
    let msg = rx
        .recv()
        .await
        .expect("the admit task dispatched its reading");
    match msg {
        ActorMsg::Event { job, event } => match event {
            super::Event::Admitted(card, ram) => {
                assert!(card.is_none(), "a RAM row never samples the card");
                assert!(ram.is_some(), "the garbage witness does answer");
                state.apply_event(job, super::Event::Admitted(card, ram));
            }
            other => panic!("the admit task sends its sample: {other:?}"),
        },
        _ => panic!("expected an event"),
    }
    let job = &state.jobs[&job_id];
    assert!(
        job.terminal,
        "a garbage host reading is a refusal, never an inflated admit"
    );
    let error = job.error.as_ref().expect("the refusal rides the row");
    assert!(
        matches!(
            error,
            SchedulerError::Admission(AdmissionError::WitnessUnavailable { reason })
                if reason.contains("unreadable ram witness reading")
                    && reason.contains("-5 GiB used")
        ),
        "the refusal names the RAM witness and quotes its raw garbage: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// State wired for the settle legs: a counting host witness, a kill-confirming
/// runtime, and the card witness never consulted (a RAM row must not read it —
/// `LedgerOnlyWitness` keeps any accidental card question loud).
struct RamSwapFixture {
    state: ActorState,
    rx: mpsc::UnboundedReceiver<ActorMsg>,
    witness: Arc<ScriptedRamWitness>,
    runtime: Arc<SettleRuntime>,
}

fn ram_swap_fixture(
    dir: &std::path::Path,
    cfg: crate::config::AppConfig,
    slots: Vec<Slot>,
    confirm: bool,
) -> RamSwapFixture {
    let (tx, rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let runtime = Arc::new(SettleRuntime {
        confirm,
        ..SettleRuntime::default()
    });
    let witness = Arc::new(ScriptedRamWitness {
        held: std::sync::atomic::AtomicBool::new(true),
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut state = ActorState::empty(
        cfg,
        dir.to_path_buf(),
        adapters,
        Arc::clone(&runtime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.ram_witness = Arc::clone(&witness) as Arc<dyn crate::witness::RamWitness>;
    state.slots = slots;
    RamSwapFixture {
        state,
        rx,
        witness,
        runtime,
    }
}

/// Drives the admit to its applied decision, asserting the RAM row asked the
/// host and never the card.
async fn drive_ram_admit(
    state: &mut ActorState,
    rx: &mut mpsc::UnboundedReceiver<ActorMsg>,
    job_id: JobId,
) {
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the admit task dispatches its host reading")
        .expect("the actor channel is open");
    match msg {
        ActorMsg::Event { job, event } => match event {
            super::Event::Admitted(card, ram) => {
                assert_eq!(job, job_id, "the reading answers the armed row");
                assert!(card.is_none(), "a RAM row never samples the card");
                assert!(ram.is_some(), "the scripted host witness answers");
                state.apply_event(job, super::Event::Admitted(card, ram));
            }
            other => panic!("the admit task sends its sample: {other:?}"),
        },
        _ => panic!("expected an event"),
    }
}

/// The RAM settle window itself: a confirmed Ram
/// release does not spawn into a host that has not proven the bytes back —
/// the spawn waits for a host witness reading past the release inside the
/// `settle_timeout_s` window, on the RAM ladder's own figures. The release is
/// the window's entry ticket, never its substitute.
/// Falsifier: hand the RAM leg back the skip (spawn on release evidence
/// alone) and the `spawns == 0` waits below go red while the witness is held.
#[tokio::test]
async fn the_ram_settle_waits_for_the_witness_before_the_spawn() {
    let dir = crate::testutil::temp_dir("ram-settle-window");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "20");
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    let mut fx = ram_swap_fixture(
        &dir,
        crate::config::AppConfig::default(),
        vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(ram_settle_seat("old-cpu", 0.0, Some(10.0), 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ],
        true,
    );
    fx.state.catalog = vec![cpu.clone()];
    let job_id = fx.state.register_queued_load(0, &cpu, Vec::new());
    fx.state.start_queued_load(job_id, 0, cpu.clone());
    drive_ram_admit(&mut fx.state, &mut fx.rx, job_id).await;
    assert_eq!(fx.state.jobs[&job_id].path, Some(AdmissionPath::FitsNow));
    assert_eq!(fx.state.jobs[&job_id].state, SwapState::Dispose);
    assert!(
        drive_ram_release(&mut fx.state, &mut fx.rx).await,
        "the driver confirms this release"
    );
    // The window is open and the host has not freed: the release is proven,
    // the spawn is not. No actor turn has run since `apply_event`, so the
    // witness has spent exactly its admit reading so far.
    assert_eq!(fx.witness.reads(), 1, "the admit reading alone so far");
    assert_eq!(fx.runtime.spawns(), 0, "the spawn waits on the witness");
    assert_eq!(fx.state.jobs[&job_id].state, SwapState::Dispose);
    assert!(!fx.state.jobs[&job_id].terminal);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        fx.witness.reads() >= 2,
        "the window samples the host after the release — release evidence alone proves nothing"
    );
    assert_eq!(
        fx.runtime.spawns(),
        0,
        "the held witness keeps the spawn parked"
    );
    fx.witness.release_pages();
    match next_ram_event(&mut fx.rx, job_id, "the settle verdict").await {
        super::Event::Settled(Ok(())) => {
            fx.state.apply_event(job_id, super::Event::Settled(Ok(())))
        }
        other => panic!("the window ends when the witness agrees: {other:?}"),
    }
    assert_eq!(
        fx.runtime.spawns(),
        0,
        "the spawn fires on the actor's turn"
    );
    match next_ram_event(&mut fx.rx, job_id, "the spawn").await {
        super::Event::Spawned(Ok(spawned)) => {
            assert_eq!(spawned.port, 18_130);
            fx.state
                .apply_event(job_id, super::Event::Spawned(Ok(spawned)));
        }
        other => panic!("the spawn answers: {other:?}"),
    }
    fx.state.apply_event(
        job_id,
        super::Event::Probed(crate::engine::Probe::Ready, None),
    );
    assert_eq!(fx.runtime.spawns(), 1, "exactly one engine");
    assert_eq!(fx.runtime.kills(), 1, "exactly one victim kill");
    let occupant = fx.state.slots[0]
        .occupant
        .as_ref()
        .expect("the replacement installs");
    assert_eq!(occupant.model, "cpu");
    assert_eq!(occupant.ram_need, Some(Gibibytes(20.0)));
    assert_eq!(
        fx.state.ram_ledger_sum(),
        Gibibytes(20.0),
        "one RAM booking moves through the swap, never two"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The window's deadline: a host that never frees the gate target
/// plus `ram_reserve_gb` within `settle_timeout_s` fails the incoming load
/// with `SettleTimeout` and its own numbers (G13) — it never spawns, and the
/// proven release stays proven: the victim slot stays clear and its debit
/// stays committed. The failure belongs to the incoming row alone.
/// Falsifier: fail the settle open (wave the timeout through) and the spawn
/// counter goes non-zero; quote a flattering figure and the numbers asserts
/// go red.
#[tokio::test]
async fn a_ram_settle_timeout_fails_the_incoming_load_never_the_victim() {
    let dir = crate::testutil::temp_dir("ram-settle-timeout");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "20");
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    let mut fx = ram_swap_fixture(
        &dir,
        crate::config::AppConfig {
            settle_timeout_s: 1,
            ..crate::config::AppConfig::default()
        },
        vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(ram_settle_seat("old-cpu", 0.0, Some(10.0), 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ],
        true,
    );
    fx.state.catalog = vec![cpu.clone()];
    let job_id = fx.state.register_queued_load(0, &cpu, Vec::new());
    fx.state.start_queued_load(job_id, 0, cpu.clone());
    drive_ram_admit(&mut fx.state, &mut fx.rx, job_id).await;
    assert!(
        drive_ram_release(&mut fx.state, &mut fx.rx).await,
        "the release itself is proven"
    );
    match next_ram_event(&mut fx.rx, job_id, "the settle verdict").await {
        super::Event::Settled(Err(reason)) => {
            fx.state
                .apply_event(job_id, super::Event::Settled(Err(reason)));
        }
        other => panic!("the witness stays held, so the window expires: {other:?}"),
    }
    let job = &fx.state.jobs[&job_id];
    assert!(job.terminal, "the load is done, not parked");
    assert_eq!(job.state, SwapState::Idle);
    let error = job.error.as_ref().expect("the row carries its reason");
    assert!(
        matches!(error, super::SchedulerError::SettleTimeout { reason }
            if reason.contains("20.0 GiB RAM free")
                && reason.contains("28.0 GiB required")
                && reason.contains("8.0 GiB reserve")),
        "the failure quotes the compared host figures, reserve named: {error}"
    );
    assert_eq!(
        fx.runtime.spawns(),
        0,
        "a host that never freed never spawns"
    );
    assert!(
        fx.state.slots[0].occupant.is_none(),
        "the proven release stays proven — only the incoming row failed"
    );
    assert_eq!(
        fx.state.ram_ledger_sum(),
        Gibibytes(0.0),
        "the released holder's debit stays committed off the ledger"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The settle window's negative-reading rule, the RAM mirror of the card
/// window: a host reading that turns negative mid-window inflates the
/// compared free (`free_gib()` clamps only its floor) past the required
/// figure, and the window must HOLD on it exactly like a silent reading —
/// then expire naming the garbage it saw, never a flattering free. The
/// proven release stays proven; only the incoming row fails.
/// Falsifier: delete the `used_gib >= 0.0` filter in `spawn_ram_settle_task`
/// and the window settles green on the flip's garbage — the panic arm below
/// fires at `Settled(Ok)`.
#[tokio::test]
async fn the_ram_settle_window_holds_a_negative_host_reading() {
    let dir = crate::testutil::temp_dir("ram-settle-garbage");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "20");
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    let mut fx = ram_swap_fixture(
        &dir,
        crate::config::AppConfig {
            settle_timeout_s: 1,
            ..crate::config::AppConfig::default()
        },
        vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(ram_settle_seat("old-cpu", 0.0, Some(10.0), 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ],
        true,
    );
    // The flip witness: sane for the admit reading (free 22.0 credits the
    // same-slot release the row waits on), negative for every window reading.
    fx.state.ram_witness = Arc::new(NegativeAfterFirstRamWitness::default());
    fx.state.catalog = vec![cpu.clone()];
    let job_id = fx.state.register_queued_load(0, &cpu, Vec::new());
    fx.state.start_queued_load(job_id, 0, cpu.clone());
    drive_ram_admit(&mut fx.state, &mut fx.rx, job_id).await;
    assert_eq!(fx.state.jobs[&job_id].state, SwapState::Dispose);
    assert!(
        drive_ram_release(&mut fx.state, &mut fx.rx).await,
        "the release itself is proven"
    );
    match next_ram_event(&mut fx.rx, job_id, "the settle verdict").await {
        super::Event::Settled(Err(reason)) => {
            fx.state
                .apply_event(job_id, super::Event::Settled(Err(reason)));
        }
        other => {
            panic!("a garbage window reading must hold the spawn, never settle it green: {other:?}")
        }
    }
    let job = &fx.state.jobs[&job_id];
    assert!(job.terminal, "the load is done, not parked");
    let error = job.error.as_ref().expect("the row carries its reason");
    assert!(
        matches!(error, super::SchedulerError::SettleTimeout { reason }
            if reason.contains("unreadable ram witness reading during settle")
                && reason.contains("-5 GiB used")),
        "the expiry names the garbage it held on, not the inflated free: {error}"
    );
    assert_eq!(fx.runtime.spawns(), 0, "garbage never spawns");
    assert!(
        fx.state.slots[0].occupant.is_none(),
        "the proven release stays proven — only the incoming row failed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier 6: a RAM eviction whose release is
/// never confirmed installs nothing and keeps the debit — the slot stays
/// occupied, the RAM ledger still books the holder, and the settle window
/// never opens: the host witness spends only its admit reading. The settle
/// window exists after proven release and nowhere else (DOCTRINE 5).
/// Falsifier: treat the reap timeout as a release (map the kill-confirm
/// timeout to `Ok`) and the debit and occupant asserts go red; let the RAM
/// settle start on an unproven release and the witness-reads assert goes red.
#[tokio::test]
async fn a_ram_eviction_with_an_unconfirmed_release_installs_nothing_and_keeps_the_debit() {
    let dir = crate::testutil::temp_dir("ram-unproven-release");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "20");
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    let mut fx = ram_swap_fixture(
        &dir,
        crate::config::AppConfig {
            release_timeout_s: 1,
            ..crate::config::AppConfig::default()
        },
        vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(ram_settle_seat("old-cpu", 0.0, Some(10.0), 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ],
        false,
    );
    fx.state.catalog = vec![cpu.clone()];
    let job_id = fx.state.register_queued_load(0, &cpu, Vec::new());
    fx.state.start_queued_load(job_id, 0, cpu.clone());
    drive_ram_admit(&mut fx.state, &mut fx.rx, job_id).await;
    assert!(
        !drive_ram_release(&mut fx.state, &mut fx.rx).await,
        "nobody ever proved this release"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let job = &fx.state.jobs[&job_id];
    assert!(job.terminal, "an unproven release fails the row");
    assert_eq!(job.state, SwapState::Idle);
    let error = job.error.as_ref().expect("the row carries its reason");
    assert!(
        matches!(error, super::SchedulerError::ReleaseTimeout { holder } if holder == "old-cpu"),
        "the holder keeps its name in the failure: {error}"
    );
    assert_eq!(
        fx.runtime.spawns(),
        0,
        "nothing installs on a ghost release"
    );
    assert_eq!(fx.runtime.kills(), 1, "the harness honestly tried the kill");
    assert_eq!(
        fx.witness.reads(),
        1,
        "the settle window never opened: an unproven release proves no bytes"
    );
    let occupant = fx.state.slots[0]
        .occupant
        .as_ref()
        .expect("the slot still reports its resident — nobody proved it gone");
    assert_eq!(occupant.model, "old-cpu");
    assert_eq!(
        fx.state.ram_ledger_sum(),
        Gibibytes(10.0),
        "the debit stays booked as long as the holder might be alive"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same-slot release figure a RAM row
/// plans with is the holder's RAM booking — credited once, never doubled —
/// and a GPU holder credits 0.0 on the RAM ladder (its host RAM is real but
/// unbooked; the settle witness reads the freed bytes instead). A VRAM row's
/// own-slot figure is unchanged.
/// Falsifier: revert the own-slot cost to `o.need.0` and the RAM-holder cost
/// goes 0.0 (the drain plans against nothing); credit it on the ledger leg
/// too and the double-count asserts go red.
#[tokio::test]
async fn the_same_slot_release_under_a_ram_row_credits_the_ram_booking_once() {
    let dir = crate::testutil::temp_dir("ram-same-slot-cost");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Leg A: a RAM holder releases its own booking — 10.0, credited exactly
    // once (the ledger leg subtracts the holder leg it already carries).
    let new_yaml = write_cpu_row_yaml(&dir, "new-cpu", "10");
    let new_cpu = rescan_entry(&new_yaml, "new-cpu");
    let mut fx = ram_swap_fixture(
        &dir,
        crate::config::AppConfig::default(),
        vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(ram_settle_seat("old-cpu", 0.0, Some(10.0), 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ],
        true,
    );
    fx.state.catalog = vec![new_cpu.clone()];
    let job_id = fx.state.register_queued_load(0, &new_cpu, Vec::new());
    fx.state.start_queued_load(job_id, 0, new_cpu.clone());
    drive_ram_admit(&mut fx.state, &mut fx.rx, job_id).await;
    assert_eq!(
        fx.state.jobs[&job_id]
            .victim
            .as_ref()
            .expect("the same-slot holder")
            .cost,
        10.0,
        "the figure a RAM holder's release credits is its RAM booking"
    );
    assert_eq!(
        fx.state.jobs[&job_id].free_at_admit,
        Some(Gibibytes(22.0)),
        "64 total - 8 reserve - 44 used + 10 holder credit"
    );
    assert!(drive_ram_release(&mut fx.state, &mut fx.rx).await);
    let job = &fx.state.jobs[&job_id];
    assert_eq!(job.released_cost, 10.0);
    assert_eq!(job.holder_released_cost, 10.0);
    assert_eq!(
        job.free_at_admit.map_or(0.0, |f| f.0) + (job.released_cost - job.holder_released_cost),
        job.free_at_admit.map_or(0.0, |f| f.0),
        "the drain re-check adds nothing the admit credit did not already hold"
    );
    match next_ram_event(&mut fx.rx, job_id, "the settle verdict").await {
        super::Event::Settled(Ok(())) => {
            fx.state.apply_event(job_id, super::Event::Settled(Ok(())))
        }
        other => panic!("the released bytes are back in the reading: {other:?}"),
    }
    match next_ram_event(&mut fx.rx, job_id, "the spawn").await {
        super::Event::Spawned(Ok(spawned)) => {
            fx.state
                .apply_event(job_id, super::Event::Spawned(Ok(spawned)));
        }
        other => panic!("the spawn answers: {other:?}"),
    }
    fx.state.apply_event(
        job_id,
        super::Event::Probed(crate::engine::Probe::Ready, None),
    );
    assert_eq!(
        fx.state.ram_ledger_sum(),
        Gibibytes(10.0),
        "one booking for one resident: the holder's debit left, the new row's arrived"
    );
    // Leg B: a GPU holder on the same slot credits 0.0 to a RAM row — its
    // host RAM is not a booked figure; the settle witness, not the gate,
    // reads the freed bytes.
    let gpu_yaml = write_cpu_row_yaml(&dir, "cpu-two", "10");
    let cpu_two = rescan_entry(&gpu_yaml, "cpu-two");
    let mut fx = ram_swap_fixture(
        &dir,
        crate::config::AppConfig::default(),
        vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(ram_settle_seat("gpu-holder", 12.0, None, 18_130)),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ],
        true,
    );
    fx.state.catalog = vec![cpu_two.clone()];
    fx.witness.release_pages();
    let job_id = fx.state.register_queued_load(0, &cpu_two, Vec::new());
    fx.state.start_queued_load(job_id, 0, cpu_two.clone());
    drive_ram_admit(&mut fx.state, &mut fx.rx, job_id).await;
    assert_eq!(
        fx.state.jobs[&job_id]
            .victim
            .as_ref()
            .expect("the same-slot GPU holder")
            .cost,
        0.0,
        "a GPU holder's host RAM is unbooked: the RAM gate credits no figure it cannot state"
    );
    assert!(drive_ram_release(&mut fx.state, &mut fx.rx).await);
    assert_eq!(fx.state.jobs[&job_id].released_cost, 0.0);
    match next_ram_event(&mut fx.rx, job_id, "the settle verdict").await {
        super::Event::Settled(Ok(())) => {
            fx.state.apply_event(job_id, super::Event::Settled(Ok(())))
        }
        other => panic!("the physical reading carries the GPU holder's freed RAM: {other:?}"),
    }
    match next_ram_event(&mut fx.rx, job_id, "the spawn").await {
        super::Event::Spawned(Ok(spawned)) => {
            fx.state
                .apply_event(job_id, super::Event::Spawned(Ok(spawned)));
        }
        other => panic!("the spawn answers: {other:?}"),
    }
    fx.state.apply_event(
        job_id,
        super::Event::Probed(crate::engine::Probe::Ready, None),
    );
    assert_eq!(
        fx.state.ram_ledger_sum(),
        Gibibytes(10.0),
        "the new RAM row books once; the GPU holder's card figure never entered it"
    );
    assert_eq!(
        fx.state.ledger_sum(),
        Gibibytes(0.0),
        "the GPU holder left the VRAM ledger with its release"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (10): the two ledgers keep their rows apart — the VRAM sum
/// never counts a RAM resident or a RAM row's booking, and the RAM sums
/// never count a GPU resident or a GPU row's.
#[test]
fn the_two_ledgers_keep_their_rows_apart() {
    let dir = crate::testutil::temp_dir("two-ledgers");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = fold_state(&dir);
    let seat = |model: &str, need: f64, ram_need: Option<f64>, port: u16| Occupant {
        model: model.to_string(),
        need: Gibibytes(need),
        ram_need: ram_need.map(Gibibytes),
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
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_130,
            occupant: Some(seat("gpu-r", 12.0, None, 18_130)),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_131,
            occupant: Some(seat("cpu-r", 0.0, Some(10.0), 18_131)),
            job: None,
        },
    ];
    assert_eq!(
        state.ledger_sum(),
        Gibibytes(12.0),
        "the RAM resident holds no card bytes"
    );
    assert_eq!(
        state.ram_ledger_sum(),
        Gibibytes(10.0),
        "the GPU resident is not a RAM booking"
    );
    // The in-flight half: one admitted-not-installed row on each ladder.
    let gpu_entry = ModelEntry::from_flat(
        &BTreeMap::from([
            ("engine".to_string(), "mock".to_string()),
            ("vram_gb".to_string(), "12".to_string()),
        ]),
        "gpu-load",
    )
    .expect("valid");
    let cpu_entry = ModelEntry::from_flat(
        &BTreeMap::from([
            ("engine".to_string(), "llama".to_string()),
            ("cpu_offload".to_string(), "true".to_string()),
            ("ram_gb".to_string(), "5".to_string()),
        ]),
        "cpu-load",
    )
    .expect("valid");
    let gpu_row = state.register_queued_load(0, &gpu_entry, Vec::new());
    state.jobs.get_mut(&gpu_row).expect("row").need = Some(Gibibytes(12.0));
    state.step(gpu_row, SwapState::Admit);
    let cpu_row = state.register_queued_load(1, &cpu_entry, Vec::new());
    state.jobs.get_mut(&cpu_row).expect("row").need = Some(Gibibytes(0.0));
    state.jobs.get_mut(&cpu_row).expect("row").ram_need = Some(Gibibytes(5.0));
    state.step(cpu_row, SwapState::Admit);
    assert_eq!(
        state.in_flight_need(cpu_row),
        12.0,
        "the card leg sees only the card row"
    );
    assert_eq!(
        state.ram_in_flight_need(cpu_row),
        0.0,
        "the other row is the RAM leg's own"
    );
    assert_eq!(
        state.in_flight_need(gpu_row),
        0.0,
        "the card leg never spends the RAM row"
    );
    assert_eq!(
        state.ram_in_flight_need(gpu_row),
        5.0,
        "the RAM leg sees only the RAM row"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same-slot RAM credit and the in-flight RAM debit, each counted once:
/// raw free 64 - 8 - 22 = 34, +10 the resident
/// this very replacement releases, -5 a RAM load already admitted elsewhere
/// — a row needing exactly 39 admits on the line, and the same host reading
/// refuses a row needing 40.
/// Falsifier: double-counting the credit admits the 40-row; missing the
/// in-flight debit admits it too; missing the credit refuses the 39-row.
#[tokio::test]
async fn a_ram_replacement_credits_its_own_slot_once_and_pays_other_ram_loads() {
    let dir = crate::testutil::temp_dir("ram-credit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let build = |name: &str, ram_gb: &str| {
        let yaml = write_cpu_row_yaml(&dir, name, ram_gb);
        rescan_entry(&yaml, name)
    };
    let seat_state = |entry: &ModelEntry| {
        let mut state = fold_state(&dir);
        state.adapters.insert(
            "llama".to_string(),
            Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
        );
        state.catalog = vec![entry.clone()];
        state.slots = vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_130,
                occupant: Some(Occupant {
                    model: "old-cpu".to_string(),
                    need: Gibibytes(0.0),
                    ram_need: Some(Gibibytes(10.0)),
                    port: 18_130,
                    ticket: fold_ticket(18_130),
                    loaded_at: Instant::now(),
                    launched_at_wall: std::time::SystemTime::now(),
                    spawn_config: None,
                    stale_config: false,
                    serve_id: None,
                    measure: None,
                    live_measured_gb: None,
                }),
                job: None,
            },
            Slot {
                category: CategoryId::Aux,
                index: 1,
                engine_port: 18_131,
                occupant: None,
                job: None,
            },
        ];
        // An admitted-not-installed RAM load elsewhere: its 5 GiB is spent.
        let other = build("other-cpu", "5");
        state.catalog.push(other.clone());
        let other_row = state.register_queued_load(1, &other, Vec::new());
        state.jobs.get_mut(&other_row).expect("row").need = Some(Gibibytes(0.0));
        state.jobs.get_mut(&other_row).expect("row").ram_need = Some(Gibibytes(5.0));
        state.step(other_row, SwapState::Admit);
        state
    };
    let fits = build("fits", "39");
    let state = &mut seat_state(&fits);
    let job_id = state.register_queued_load(0, &fits, Vec::new());
    state.start_queued_load(job_id, 0, fits.clone());
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 22.0,
        }),
    );
    assert!(
        state.jobs[&job_id].error.is_none(),
        "{:?}",
        state.jobs[&job_id].error
    );
    assert_eq!(state.jobs[&job_id].path, Some(AdmissionPath::FitsNow));
    let over = build("over", "40");
    let state = &mut seat_state(&over);
    let job_id = state.register_queued_load(0, &over, Vec::new());
    state.start_queued_load(job_id, 0, over.clone());
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 22.0,
        }),
    );
    let msg = state.jobs[&job_id]
        .error
        .as_ref()
        .expect("40 > the 39 the host honestly has")
        .to_string();
    assert!(
        msg.contains("free 39.0 GiB"),
        "credit and debit counted once: {msg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (3): RAM short with an evictable Ram resident in a
/// category the load's own may evict. The RAM planner selects it, the
/// planned victim rides the row with the resident's own release ticket (the
/// unchanged evidence-based release path), and the load admits on the Drain
/// rung.
/// Falsifier: before the RAM planner existed the RAM gate never planned, so
/// this row reads a terminal `InsufficientRam` refusal and no victim exists.
#[tokio::test]
async fn a_ram_short_admission_evicts_the_yielding_ram_resident() {
    let dir = crate::testutil::temp_dir("ram-evict");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "20");
    let mut state = fold_state(&dir);
    state.adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    state.catalog = vec![cpu.clone()];
    let aux_ticket = fold_ticket(18_131);
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
            occupant: Some(Occupant {
                model: "aux-cpu".to_string(),
                need: Gibibytes(0.0),
                ram_need: Some(Gibibytes(10.0)),
                port: 18_131,
                ticket: aux_ticket.clone(),
                loaded_at: Instant::now(),
                launched_at_wall: std::time::SystemTime::now(),
                spawn_config: None,
                stale_config: false,
                serve_id: None,
                measure: None,
                live_measured_gb: None,
            }),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &cpu, Vec::new());
    state.start_queued_load(job_id, 0, cpu.clone());
    // free = 64 - 8 (reserve) - 44 (used) = 12 < need 20: an 8 GiB deficit,
    // and the 10 GiB Ram resident in AUX (evictable by WORK in the shipped
    // matrix) covers it.
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 44.0,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.error.is_none(), "{:?}", job.error);
    assert!(
        !job.terminal,
        "a coverable RAM deficit admits on the drain rung, it does not refuse"
    );
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    let victim = job
        .victim
        .as_ref()
        .expect("the planned victim rides the row");
    assert_eq!(victim.model, "aux-cpu");
    assert_eq!(victim.slot, 1);
    assert!(
        victim.evicted,
        "a cross-slot eviction, not a same-slot holder"
    );
    assert_eq!(
        victim.cost, 10.0,
        "the freed figure is the RAM booking, never the zero VRAM need"
    );
    assert_eq!(
        victim.ticket, aux_ticket,
        "the release runs on the resident's own (port, generation) ticket"
    );
    assert_eq!(job.state, SwapState::Dispose);
    assert!(
        state.slots[1].occupant.is_some(),
        "the victim stays seated until its release is confirmed (DOCTRINE 5)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Falsifier (3, the refusal leg): RAM short with no evictable Ram
/// resident. The refusal names every Ram holder with its matrix reason and
/// kills nothing.
/// Falsifier: before the RAM planner existed the RAM refusal's only holder reason was
/// the earlier uniform "the RAM gate does not release" line, so this pin's
/// matrix reason is absent and the pin goes red.
#[tokio::test]
async fn a_ram_short_admission_with_no_evictable_ram_resident_refuses_naming_holders() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("ram-no-victim");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("cpu-aux.yaml");
    std::fs::write(
        &yaml,
        "engine: llama\ncategory: aux\nram_gb: 20\ncpu_offload: true\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    let cpu = rescan_entry(&yaml, "cpu-aux");
    state.catalog = vec![cpu.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_130,
            occupant: Some(Occupant {
                model: "work-cpu".to_string(),
                need: Gibibytes(0.0),
                ram_need: Some(Gibibytes(10.0)),
                port: 18_130,
                ticket: fold_ticket(18_130),
                loaded_at: Instant::now(),
                launched_at_wall: std::time::SystemTime::now(),
                spawn_config: None,
                stale_config: false,
                serve_id: None,
                measure: None,
                live_measured_gb: None,
            }),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_131,
            occupant: None,
            job: None,
        },
    ];
    let job_id = state.register_queued_load(1, &cpu, Vec::new());
    state.start_queued_load(job_id, 1, cpu.clone());
    // free = 64 - 8 - 44 = 12 < need 20: the only Ram holder sits in WORK,
    // which the shipped matrix lets nobody evict.
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 44.0,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.terminal, "an unyieldable RAM shortage refuses");
    assert_eq!(job.state, SwapState::AdmitFail);
    let error = job.error.as_ref().expect("the refusal rides the row");
    let SchedulerError::Admission(AdmissionError::InsufficientRam {
        victims_considered,
        blockers,
        ..
    }) = error
    else {
        panic!("the RAM refusal has its own class: {error:?}");
    };
    assert!(
        victims_considered.is_empty(),
        "WORK yields no victim to any planner: {victims_considered:?}"
    );
    assert!(
        blockers
            .iter()
            .any(|b| b.model == "work-cpu" && b.reason.contains("nothing may evict WORK")),
        "the refusal names the Ram holder with its matrix reason: {blockers:?}"
    );
    let msg = error.to_string();
    assert!(
        !msg.contains("considered evicting"),
        "nothing was ever a RAM candidate: {msg}"
    );
    assert!(
        state.slots[0].occupant.is_some(),
        "nothing killed: the named holder stays seated"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Wrong-class pin (W2, falsifier 4's RAM side): the RAM planner over a
/// machine whose only occupant is a GPU job in an evictable category never
/// selects or even considers it. The deficit is one the GPU resident's
/// booked 15 GiB would cover if the resource filter were down, and AUX is
/// evictable by WORK in the shipped matrix — only the resource filter keeps
/// it out of the candidate set; it rides the refusal as the uncontrolled
/// holder its host RAM actually is.
/// Falsifier: with the RAM planner seeing all occupants (the resource filter
/// reverted in place), the GPU job becomes the covering candidate — the row
/// drains instead of refusing and the tail names it as considered. Against
/// the planner-absent tree the refusal shape is the same; the
/// pin's weight is the mutation red.
#[tokio::test]
async fn the_ram_planner_never_considers_a_gpu_occupant_as_a_victim() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("ram-planner-wrong-class");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cpu_yaml = write_cpu_row_yaml(&dir, "cpu", "18");
    let mut state = fold_state(&dir);
    let cpu = rescan_entry(&cpu_yaml, "cpu");
    state.catalog = vec![cpu.clone()];
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
            occupant: Some(Occupant {
                model: "gpu-res".to_string(),
                need: Gibibytes(15.0),
                ram_need: None,
                port: 18_131,
                ticket: fold_ticket(18_131),
                loaded_at: Instant::now(),
                launched_at_wall: std::time::SystemTime::now(),
                spawn_config: None,
                stale_config: false,
                serve_id: None,
                measure: None,
                live_measured_gb: None,
            }),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &cpu, Vec::new());
    state.start_queued_load(job_id, 0, cpu.clone());
    // free = 64 - 8 - 52 = 4 < need 18: a 14 GiB deficit a filter-blind
    // planner would happily cover with the GPU job's bytes.
    state.finish_admit(
        job_id,
        None,
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: 52.0,
        }),
    );
    let job = &state.jobs[&job_id];
    let error = job.error.as_ref().expect("a RAM-only shortage refuses");
    let SchedulerError::Admission(AdmissionError::InsufficientRam {
        victims_considered,
        blockers,
        ..
    }) = error
    else {
        panic!("a RAM shortage stays the RAM class: {error:?}");
    };
    assert!(
        job.terminal,
        "the GPU job is no RAM victim: it never drains"
    );
    assert!(
        victims_considered.is_empty(),
        "the GPU occupant was not even considered: {victims_considered:?}"
    );
    assert!(
        blockers.iter().any(
            |b| b.model == "gpu-res" && b.reason.contains("never under the RAM gate's control")
        ),
        "it rides the refusal only as the uncontrolled holder it is: {blockers:?}"
    );
    let msg = error.to_string();
    assert!(
        !msg.contains("considered evicting"),
        "never considered, never victim: {msg}"
    );
    assert_eq!(job.state, SwapState::AdmitFail);
    assert!(
        state.slots[1].occupant.is_some(),
        "nothing killed: the GPU resident the RAM gate may not touch stays seated"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
