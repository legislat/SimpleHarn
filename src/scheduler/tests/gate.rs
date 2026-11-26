use super::*;

/// Shared fixture for the parked-repeat pins: one `ActorState` whose single
/// Work slot keeps its running job (the id names a row the test never holds,
/// exactly the shape every slot shows when it parks) while the parked row
/// lives in `pending` and owns no slot. Same scaffold as
/// `a_rescan_keeps_a_queue_whose_fingerprint_cannot_show_a_change`.
fn busy_work_slot_state(dir: &std::path::Path) -> ActorState {
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
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
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_100,
        occupant: None,
        // The parked row does not own the slot: some other job is in
        // flight on it, exactly as parking requires.
        job: Some(JobId(9_999)),
    }];
    state
}

/// The repeat of a parked model attaches to the parked row instead of
/// superseding it: `on_swap`'s identical-in-flight attach scans
/// `slots[s].job`, and a parked row is not on a slot — it lives in `pending`
/// while the busy slot keeps its running job, so the occupant scan above it
/// cannot see it. Falsifier: drop the parked attach check and the repeat
/// falls to the park-and-supersede path: `next_job` mints a second row,
/// `A` terminalizes with `Cancelled("superseded by a newer load before it
/// started")`, and the `next_job`, `pending`, and `terminal` asserts all go
/// red together — the live ledger's `Idle cancelled…` row followed by a fresh
/// `Ready` row. Ordering against the `Already` loop is not judged here: a
/// serving occupant and a parked row for the same model cannot coexist, so the
/// parked check cannot shadow an `Already` answer. The different-model test
/// below pins that the park-and-supersede path is untouched.
#[test]
fn a_repeat_load_of_a_parked_model_attaches_to_its_row() {
    let dir = crate::testutil::temp_dir("parked-repeat");
    let yaml = dir.join("park-a.yaml");
    std::fs::write(&yaml, "engine: mock\nvram_gb: 4\n").expect("seed");
    let mut state = busy_work_slot_state(&dir);
    let entry = entry_from(&yaml, "park-a");
    state.catalog = vec![entry.clone()];
    let a = state.register_queued_load(0, &entry, Vec::new());
    state.pending.insert(0, a);
    let next_before = state.next_job;
    let waiters_before = state.jobs[&a].waiters.len();
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    state.on_swap("park-a".to_string(), CategoryId::Work, reply_tx);
    assert_eq!(
        state.next_job, next_before,
        "the repeat must not mint a second ledger row"
    );
    assert_eq!(
        state.pending.get(&0),
        Some(&a),
        "the parked row keeps its queue place"
    );
    let row = &state.jobs[&a];
    assert!(!row.terminal, "the parked row stays in flight");
    assert_eq!(
        row.waiters.len(),
        waiters_before + 1,
        "the repeat joins the same row as a waiter"
    );
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "an attach answers nothing before the load finishes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_category_without_a_slot_answers_the_swap_with_an_invariant() {
    use super::SchedulerError;

    let dir = crate::testutil::temp_dir("category-without-slot-invariant");
    let yaml = dir.join("swap-a.yaml");
    std::fs::write(&yaml, "engine: mock\nvram_gb: 4\n").expect("seed");
    let mut state = busy_work_slot_state(&dir);
    state.slots.clear();
    state.catalog = vec![entry_from(&yaml, "swap-a")];

    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    state.on_swap("swap-a".to_string(), CategoryId::Work, reply_tx);
    let answer = match reply_rx.try_recv().expect("the swap answers immediately") {
        Ok(reply) => panic!("expected an invariant refusal, got {reply:?}"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &answer,
            SchedulerError::InvariantViolation { reason }
                if reason.contains("the category has at least one slot")
        ),
        "expected the missing-category-slot invariant, got {answer:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other half of the same rule: a repeat only attaches to a parked row
/// of the SAME model. A different model in the same category still parks and
/// supersedes, because nothing in flight can serve it. Falsifier: let the
/// parked attach match any parked Load of the category (drop the model
/// check) and the different model would join `A`'s row instead: `A` stays
/// non-terminal with the wrong model's waiter, and the supersede legs go red
/// here.
#[test]
fn a_different_model_still_supersedes_the_parked_one() {
    let dir = crate::testutil::temp_dir("parked-supersede");
    let yaml_a = dir.join("park-a.yaml");
    let yaml_b = dir.join("park-b.yaml");
    std::fs::write(&yaml_a, "engine: mock\nvram_gb: 4\n").expect("seed a");
    std::fs::write(&yaml_b, "engine: mock\nvram_gb: 4\n").expect("seed b");
    let mut state = busy_work_slot_state(&dir);
    let entry_a = entry_from(&yaml_a, "park-a");
    let entry_b = entry_from(&yaml_b, "park-b");
    state.catalog = vec![entry_a.clone(), entry_b.clone()];
    let a = state.register_queued_load(0, &entry_a, Vec::new());
    state.pending.insert(0, a);
    let next_before = state.next_job;
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    state.on_swap("park-b".to_string(), CategoryId::Work, reply_tx);
    let row = &state.jobs[&a];
    assert!(row.terminal, "a different model must still supersede");
    assert_eq!(row.outcome.as_deref(), Some("cancelled"));
    let error = row
        .error
        .clone()
        .expect("the supersede reason rides the row")
        .to_string();
    assert!(error.contains("superseded by a newer load"), "{error}");
    let parked = *state
        .pending
        .get(&0)
        .expect("the newer load parks in the superseded row's place");
    assert_ne!(parked, a, "the queue place now names a different row");
    assert_eq!(parked, JobId(next_before), "the place names the newer row");
    let newer = &state.jobs[&parked];
    assert!(!newer.terminal, "the newer row waits for the running job");
    assert_eq!(newer.model, "park-b");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Port separation ("8003-8005 are manual only"): a ninfer row
/// borrowing a manual number as its container-internal port is refused before
/// spawn, the SE internal default (8808) arms like any ordinary row,
/// and the rescan twin names the key on the page before any load tries.
#[tokio::test]
async fn a_borrowed_internal_port_is_refused_at_the_gate_and_reported_at_rescan() {
    use super::{AdmissionError, SchedulerError};
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "ninfer".to_string(),
        Arc::new(crate::engine::ninfer::NinferAdapter::new("Ubuntu", "ops"))
            as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        crate::testutil::temp_dir("r10-port"),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_109,
        occupant: None,
        job: None,
    }];
    let ninfer_row = |port: &str| -> ModelEntry {
        let mut map = BTreeMap::new();
        map.insert("engine".to_string(), "ninfer".to_string());
        map.insert("vram_gb".to_string(), "4".to_string());
        if !port.is_empty() {
            map.insert("ninfer_internal_port".to_string(), port.to_string());
        }
        ModelEntry::from_flat(&map, "nin").expect("entry")
    };
    // A manual number is dead on arrival: a named refusal, no spawn.
    let stolen = ninfer_row("8004");
    let id = state.register_queued_load(0, &stolen, Vec::new());
    state.start_queued_load(id, 0, stolen);
    let job = &state.jobs[&id];
    assert!(job.terminal, "a borrowed port must never reach spawn");
    assert!(
        matches!(
            job.error,
            Some(SchedulerError::Admission(AdmissionError::PortClaimed {
                port: 8004,
                ..
            }))
        ),
        "{:?}",
        job.error
    );
    // The engine's own EXPOSE number arms like any ordinary row.
    let ordinary = ninfer_row("");
    let id = state.register_queued_load(0, &ordinary, Vec::new());
    state.start_queued_load(id, 0, ordinary);
    assert!(
        !state.jobs[&id].terminal,
        "8808 is no namespace's to refuse: {:?}",
        state.jobs[&id].error
    );
    // The page twin: the rescan names the key before any load tries.
    state.apply_catalog(
        crate::model::scan::Catalog {
            models: vec![ninfer_row("8005")],
            errors: Vec::new(),
        },
        None,
        &DriftStats::default(),
    );
    assert!(
        state
            .catalog_errors
            .iter()
            .any(|e| { e.reason.contains("ninfer_internal_port") && e.reason.contains("8005") }),
        "{:?}",
        state.catalog_errors
    );
}

/// Launch-floor guard: a floor the card denies must refuse BEFORE
/// any kill. The release-first ruling buys the victim's bytes only when the
/// deficit is one a release could actually fix — when freeing everything the
/// harness may release still leaves the floor out of reach, the cost is not
/// bought. Field case (GOTCHAS): foreign residents hold the
/// card at 70.5/72 and nothing is evictable.
#[tokio::test]
async fn a_floor_no_release_can_reach_refuses_before_the_victim_dies() {
    let dir = crate::testutil::temp_dir("r10-floor");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("greedy.yaml");
    std::fs::write(
            &yaml,
            "engine: mock\ncategory: work\ncost: greedy\nvram_gb: 2\nlaunch_only_with_at_least_vram_free: 60\n",
        )
        .expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "greedy");
    assert_eq!(
        entry.vram_gate.floor(),
        Some(60.0),
        "the floor key must survive the real parser"
    );
    state.catalog = vec![entry.clone()];
    let ticket = fold_ticket(19_049);
    seat_spawn(&mut state, &entry, &ticket, 19_049);
    // Arm the row against its own slot exactly as `start_queued_load` does,
    // then hand the admit the witness sample the armed sample task would send.
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(2.0));
        job.victim = Some(super::Victim {
            model: entry.name.clone(),
            ticket: ticket.clone(),
            slot: 0,
            evicted: false,
            cost: 2.0,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 70.5,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.terminal, "an unreachable floor must refuse at the gate");
    assert!(
        job.path.is_none(),
        "a refusal never names a rung the gate never accepted: {:?}",
        job.path
    );
    assert!(
        !job.release_first,
        "the guard precedes release-first: no resident is doomed for a floor the card denies"
    );
    let error = job
        .error
        .as_ref()
        .expect("the refusal travels with the row")
        .to_string();
    assert!(
        error.contains("No releases allow for the VRAM floor")
            && error.contains("Check foreign residents"),
        "{error}"
    );
    assert!(error.contains("unreleasable"), "{error}");
    assert!(
        state.slots[0].occupant.is_some(),
        "refusal before kill: the resident the ruling protects is still seated"
    );
    assert!(
        state.slots[0].job.is_none(),
        "the refused row releases its claim on the slot"
    );
}

/// The other half: the guard guards the FLOOR only. A `need`-short swap with a
/// same-slot victim still takes the release-first path: the settle window
/// re-gates it against the physical card.
#[tokio::test]
async fn a_need_short_swap_still_takes_the_release_first_path() {
    let dir = crate::testutil::temp_dir("r10-release-first");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("big.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 40\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "big");
    assert_eq!(entry.vram_gate.floor(), None, "this row carries no floor");
    state.catalog = vec![entry.clone()];
    let ticket = fold_ticket(19_050);
    seat_spawn(&mut state, &entry, &ticket, 19_050);
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(40.0));
        job.victim = Some(super::Victim {
            model: entry.name.clone(),
            ticket,
            slot: 0,
            evicted: false,
            cost: 2.0,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 70.5,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        job.release_first,
        "a releasable deficit still re-gates on the physical card: {:?}",
        job.error
    );
    assert_eq!(
        job.path,
        Some(AdmissionPath::ReleaseFirst),
        "the release-first door records its own rung"
    );
    assert!(!job.terminal, "release-first proceeds, it does not refuse");
    assert_eq!(job.state, SwapState::Dispose);
}

/// Shared fixture for the gate-park pins: two
/// empty WORK slots on a 72 GiB card with the 2 GiB reserve, plus `ninfer-like`
/// — the admitted, non-terminal booking row sitting on slot 0 with its spawn
/// still in flight and no occupant installed: the witness counts its
/// allocation and `in_flight_need` debits its paper booking on top, exactly the
/// live `Ninfer-nvfp4` shape. Returns the state and the entry of the row under
/// test (`waiting`), which the pin arms on slot 1.
fn booking_short_fixture(
    dir: &std::path::Path,
    waiting_name: &str,
    waiting_yaml: &str,
) -> (ActorState, ModelEntry) {
    let inflight_yaml = dir.join("ninfer-like.yaml");
    std::fs::write(
        &inflight_yaml,
        "engine: mock\ncategory: work\nvram_gb: 29.8\n",
    )
    .expect("seed booking");
    let waiting_path = dir.join(format!("{waiting_name}.yaml"));
    std::fs::write(&waiting_path, waiting_yaml).expect("seed waiting");
    let mut state = fold_state(dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    let inflight = rescan_entry(&inflight_yaml, "ninfer-like");
    let waiting = rescan_entry(&waiting_path, waiting_name);
    state.catalog = vec![inflight.clone(), waiting.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_140,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Work,
            index: 1,
            engine_port: 18_141,
            occupant: None,
            job: None,
        },
    ];
    // The in-flight booking, armed as `start_queued_load` arms a row (slot
    // claim, booked need, `Admit`) without a dispatched sample task: the
    // booking is what the second gate debits, and its row never receives the
    // event that would let it install.
    let booking = state.register_queued_load(0, &inflight, Vec::new());
    state.slots[0].job = Some(booking);
    state.jobs.get_mut(&booking).expect("booking row").need = Some(Gibibytes(29.8));
    state.step(booking, SwapState::Admit);
    (state, waiting)
}

/// The incident shape: `Ninfer-nvfp4` was
/// installing (booking 29.8), the witness already counted its allocation and
/// the gate also debited its booking, and RP-1 (need 29) saw free
/// 72 - 2 - 23.8 - 29.8 = 16.4 and died `AdmitFail` three times - while the
/// moment the Ninfer row terminalized, the same load admitted on the spot.
/// The ruling: a deficit covered only by in-flight bookings waits, and the
/// terminal funnel re-gates it. Falsifier: delete the park arm in
/// `finish_admit` and the first gate terminalizes the row with `AdmitFail` —
/// the non-terminal, error-free, `pending`, and unanswered-waiter legs all go
/// red together, and the second half never runs.
#[tokio::test]
async fn a_load_short_only_on_an_in_flight_booking_waits_then_admits() {
    let dir = crate::testutil::temp_dir("gate-park-wait");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting) =
        booking_short_fixture(&dir, "rp-1", "engine: mock\ncategory: work\nvram_gb: 29\n");
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(1, waiting.clone(), vec![reply_tx]);
    // free = 72 - 2 - 23.8 - 29.8 = 16.4 against need 29: short 12.6, and the
    // 29.8 booking covers it exactly once the spawn terminalizes.
    state.finish_admit_card(parked, Ok(gate_sample(23.8)));
    let row = &state.jobs[&parked];
    assert!(
        !row.terminal,
        "a paper-booking deficit is a wait, not a refusal: {:?}",
        row.error
    );
    assert!(row.error.is_none(), "{:?}", row.error);
    assert_eq!(row.state, SwapState::Idle, "the row is parked, not armed");
    assert_eq!(
        state.pending.get(&1),
        Some(&parked),
        "the row waits in its slot's queue place"
    );
    assert!(
        state.slots[1].job.is_none(),
        "the parked row releases the gate's slot claim"
    );
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the caller keeps waiting - the row answers nothing at park time"
    );
    // The booked load terminalizes: the booking leaves the gate, and the
    // terminal funnel must re-gate the waiting row through the queue path.
    let booking = state.slots[0].job.expect("the in-flight row owns slot 0");
    state.cancel_job(booking, "the in-flight engine died before install");
    assert!(
        state.jobs[&booking].terminal,
        "the booking has left the sum"
    );
    assert_eq!(
        state.slots[1].job,
        Some(parked),
        "the drain re-arms the parked row on its free slot"
    );
    assert!(state.pending.is_empty(), "and takes its queue place");
    assert_eq!(
        state.jobs[&parked].state,
        SwapState::Admit,
        "the row stands before the gate again"
    );
    assert!(!state.jobs[&parked].terminal);
    // Re-gate with the booking gone and the card free: the gate says yes on
    // the fresh witness sample, and the caller's waiter gets the real answer.
    state.finish_admit_card(parked, Ok(gate_sample(2.0)));
    let row = &state.jobs[&parked];
    assert!(
        !row.terminal,
        "the roomy card admits the waiting load: {:?}",
        row.error
    );
    assert_eq!(row.path, Some(AdmissionPath::FitsNow));
    assert_eq!(
        row.state,
        SwapState::Spawn,
        "the row advanced past Idle: the re-gate armed it"
    );
    state.apply_event(
        parked,
        super::Event::Spawned(Ok(crate::engine::Spawned {
            port: 18_141,
            pid: 7_777,
        })),
    );
    state.apply_event(
        parked,
        super::Event::Probed(crate::engine::Probe::Ready, Some("rp-1".to_string())),
    );
    let row = &state.jobs[&parked];
    assert!(row.terminal, "the load completes as any ordinary load");
    assert_eq!(row.outcome.as_deref(), Some("loaded"));
    assert_eq!(
        state.slots[1].occupant.as_ref().map(|o| o.model.as_str()),
        Some("rp-1"),
        "the waiter waited; the engine now serves"
    );
    match reply_rx.try_recv() {
        Ok(Ok(Reply::Loaded(receipt))) => {
            assert_eq!(receipt.model, "rp-1");
            assert_eq!(receipt.port, 18_141);
        }
        other => panic!("the waiter receives the load receipt: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The second arm of the drain's re-gate: the blocking
/// booking terminalises, the drain re-arms the parked row, but the card no
/// longer has the room - the witness now reads 60 of 72 GiB used, foreign
/// bytes the dead booking never held - so `in_flight` is zero, the park
/// guard fails, and the re-gate takes the ordinary named `AdmitFail` with
/// the caller's waiter answered exactly once: not another park, not a hang.
/// Falsifier: remove the re-gate's refusal arm - park unconditionally
/// instead of only when the bookings cover the deficit, i.e. drop the
/// `in_flight > 0 && deficit <= in_flight` legs from the park guard - and
/// this re-gate parks the row again: the `terminal`, `AdmitFail`,
/// empty-`pending`, and answered-waiter legs all go red, and the wait
/// outlives the very memory it was waiting for.
#[tokio::test]
async fn a_wait_that_cannot_fit_after_the_booking_dies_refuses() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("gate-park-refuse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting) =
        booking_short_fixture(&dir, "rp-1", "engine: mock\ncategory: work\nvram_gb: 29\n");
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(1, waiting, vec![reply_tx]);
    state.finish_admit_card(parked, Ok(gate_sample(23.8)));
    assert_eq!(state.pending.get(&1), Some(&parked), "the row waits");
    // The blocking booking terminalises and the terminal funnel re-gates the
    // parked row through the queue path.
    let booking = state.slots[0].job.expect("the in-flight row owns slot 0");
    state.cancel_job(booking, "the in-flight engine died before install");
    assert_eq!(
        state.slots[1].job,
        Some(parked),
        "the drain re-arms the parked row"
    );
    // The re-gate now reads a card the booking's death did not free: free
    // 72 - 2 - 60 = 10 against need 29, with no booking left to cover the
    // 19 GiB deficit.
    state.finish_admit_card(parked, Ok(gate_sample(60.0)));
    let row = &state.jobs[&parked];
    assert!(
        row.terminal,
        "a re-gate short on foreign memory refuses, not parks: {:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(
        matches!(
            row.error,
            Some(SchedulerError::Admission(
                AdmissionError::Insufficient { .. }
            ))
        ),
        "{:?}",
        row.error
    );
    assert!(state.pending.is_empty(), "the refusal never re-parks");
    assert!(state.slots[1].job.is_none(), "and releases the slot");
    match reply_rx.try_recv() {
        Ok(Err(SchedulerError::Admission(AdmissionError::Insufficient { .. }))) => {}
        other => panic!("the caller receives the named refusal: {other:?}"),
    }
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ),
        "exactly once"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other half of the ruling: the wait is reserved for a deficit another
/// load's booking causes. Nothing is in flight here - a foreign tenant holds
/// 60 of 72 GiB and the harness may release nothing - so the shortfall is the
/// memory the wait cannot free, and DOCTRINE 3's named refusal stands.
/// Falsifier: let the park guard ignore `in_flight > 0` (wait whenever the
/// gate is short) and this row parks instead of refusing: the `terminal` and
/// `Insufficient` legs go red, `pending` grows, and the real refusal is
/// swallowed.
#[tokio::test]
async fn foreign_memory_short_of_need_still_refuses() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("gate-park-foreign");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("big.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 40\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "big");
    state.catalog = vec![entry.clone()];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_142,
        occupant: None,
        job: None,
    }];
    let row_id = JobId(state.next_job);
    state.begin_load(0, entry, Vec::new());
    // free = 72 - 2 - 60 = 10 < need 40, with no booking to wait behind.
    state.finish_admit_card(row_id, Ok(gate_sample(60.0)));
    let row = &state.jobs[&row_id];
    assert!(
        row.terminal,
        "memory no wait can free is a named refusal, never a park"
    );
    assert!(
        matches!(
            row.error,
            Some(SchedulerError::Admission(
                AdmissionError::Insufficient { .. }
            ))
        ),
        "{:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(
        state.pending.is_empty(),
        "the wait must not swallow the real refusal"
    );
    assert!(
        state.slots[0].job.is_none(),
        "the refused row releases its slot"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The floor leg of the same ruling: the in-flight booking numerically covers
/// this floor deficit (`deficit 19.8 <= in_flight 29.8`), but even releasing
/// everything the harness may touch leaves the launch floor out of reach
/// (`no_releases_reach`), and the launch-floor guard refuses before any wait.
/// Falsifier: drop `!no_releases_reach` from the park guard and this row
/// parks on a floor the card denies: `terminal` and the `BelowLaunchFloor` leg
/// go red - a wait would end only in the same refusal, late and un-named.
#[tokio::test]
async fn an_unreachable_floor_still_refuses() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("gate-park-floor");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting) = booking_short_fixture(
        &dir,
        "floor-row",
        "engine: mock\ncategory: work\nvram_gb: 2\nlaunch_only_with_at_least_vram_free: 40\n",
    );
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    let row_id = JobId(state.next_job);
    state.begin_load(1, waiting, vec![reply_tx]);
    // free = 72 - 2 - 20 - 29.8 = 20.2; floor 40 is 19.8 out of reach and no
    // release the harness may perform can raise free to 38 (tolerance 2).
    state.finish_admit_card(row_id, Ok(gate_sample(20.0)));
    let row = &state.jobs[&row_id];
    assert!(
        row.terminal,
        "an unreachable floor refuses even when a booking covers the deficit"
    );
    assert!(
        matches!(
            row.error,
            Some(SchedulerError::Admission(
                AdmissionError::BelowLaunchFloor {
                    no_releases_reach: true,
                    ..
                }
            ))
        ),
        "{:?}",
        row.error
    );
    let error = row
        .error
        .as_ref()
        .expect("the refusal rides the row")
        .to_string();
    assert!(
        error.contains("No releases allow for the VRAM floor"),
        "{error}"
    );
    assert_eq!(row.state, SwapState::AdmitFail);
    assert!(state.pending.is_empty(), "no queue place for a refusal");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every path that takes a parked row away answers its waiters
/// exactly once. Here: `Cancel` on a gate-parked row (the `Idle`-state queued
/// branch), then `Unload All` over a second parked row - the command the
/// parking ruling newly gives that shape. Falsifier: answer the waiter at park
/// time or in both the drain and the cancel, and the first `try_recv` leg or
/// the `Closed` follow-up goes red; without the `pending`-slot leg of
/// `on_unload_all`, the second waiter never answers and `pending` still names
/// the row at the unload asserts.
#[tokio::test]
async fn a_cancelled_waiting_load_is_answered_once() {
    use super::SchedulerError;
    let dir = crate::testutil::temp_dir("gate-park-cancel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting) =
        booking_short_fixture(&dir, "rp-1", "engine: mock\ncategory: work\nvram_gb: 29\n");
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(1, waiting.clone(), vec![reply_tx]);
    state.finish_admit_card(parked, Ok(gate_sample(23.8)));
    assert_eq!(state.pending.get(&1), Some(&parked), "the row waits");
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    state.on_cancel(parked, cancel_tx);
    match reply_rx.try_recv() {
        Ok(Err(SchedulerError::Cancelled(reason))) => {
            assert!(reason.contains("cancelled by request"), "{reason}");
        }
        other => panic!("the parked row's single waiter is answered once: {other:?}"),
    }
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ),
        "nobody answers it a second time"
    );
    assert!(matches!(cancel_rx.try_recv(), Ok(Ok(Reply::Done))));
    let row = &state.jobs[&parked];
    assert!(row.terminal, "the cancel terminalizes the parked row");
    assert_eq!(row.outcome.as_deref(), Some("cancelled"));
    assert!(state.pending.is_empty(), "and releases the queue place");
    assert!(state.slots[1].job.is_none());
    let view = state
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.id == parked)
        .expect("the row stands on the ledger");
    assert!(
        !view.is_fault,
        "a waiting load the operator took back is not a fault"
    );
    // The other answer path: `Unload All` while a row is parked. The row the
    // command can see is the row the command removes.
    let (reply2_tx, mut reply2_rx) = tokio::sync::oneshot::channel();
    let parked2 = JobId(state.next_job);
    state.begin_load(1, waiting, vec![reply2_tx]);
    state.finish_admit_card(parked2, Ok(gate_sample(23.8)));
    assert_eq!(state.pending.get(&1), Some(&parked2), "a fresh wait forms");
    let (unload_tx, mut unload_rx) = tokio::sync::oneshot::channel();
    state.on_unload_all(unload_tx);
    match reply2_rx.try_recv() {
        Ok(Err(SchedulerError::Cancelled(reason))) => {
            assert!(reason.contains("unloaded by request"), "{reason}");
        }
        other => panic!("the unload answers the parked waiter once: {other:?}"),
    }
    assert!(
        matches!(
            reply2_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ),
        "and never twice"
    );
    match unload_rx.try_recv() {
        Ok(Ok(Reply::Unloaded {
            unloading,
            jobs_cancelled,
        })) => {
            assert_eq!(unloading, 0, "nothing was installed to unload");
            assert_eq!(
                jobs_cancelled, 2,
                "the in-flight booking and the parked row both answered"
            );
        }
        other => panic!("the unload reports what it cancelled: {other:?}"),
    }
    assert!(state.pending.is_empty());
    assert!(state
        .slots
        .iter()
        .all(|s| s.job.is_none() && s.occupant.is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other half of the protection: the gate-parked row sits
/// on a slot that shows neither job nor occupant, so target selection would
/// happily arm a different model on top of its queue place. It must instead
/// take the park-and-supersede path - which answers the parked row - and the
/// supersede prefers a waiting row's place over killing the in-flight load.
/// Falsifier: drop the `!pending` leg from `on_swap`'s `idle` predicate and
/// the third model begins loading on the parked row's slot: the parked row
/// never terminalizes, its waiter stays unanswered, and `pending` still names
/// the superseded row while the slot gains a second silent job.
#[tokio::test]
async fn a_different_model_supersedes_a_gate_parked_row_instead_of_stealing_its_slot() {
    use super::SchedulerError;
    let dir = crate::testutil::temp_dir("gate-park-supersede");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (mut state, waiting) =
        booking_short_fixture(&dir, "rp-1", "engine: mock\ncategory: work\nvram_gb: 29\n");
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let parked = JobId(state.next_job);
    state.begin_load(1, waiting, vec![reply_tx]);
    state.finish_admit_card(parked, Ok(gate_sample(23.8)));
    assert_eq!(
        state.pending.get(&1),
        Some(&parked),
        "the row waits on slot 1"
    );
    let other_yaml = dir.join("other.yaml");
    std::fs::write(&other_yaml, "engine: mock\ncategory: work\nvram_gb: 4\n").expect("seed other");
    let other = rescan_entry(&other_yaml, "other");
    state.catalog.push(other);
    let (third_tx, _third_rx) = tokio::sync::oneshot::channel();
    state.on_swap("other".to_string(), CategoryId::Work, third_tx);
    let row = &state.jobs[&parked];
    assert!(
        row.terminal,
        "the different model supersedes the parked row's queue place"
    );
    assert_eq!(row.outcome.as_deref(), Some("cancelled"));
    match reply_rx.try_recv() {
        Ok(Err(SchedulerError::Cancelled(reason))) => {
            assert!(reason.contains("superseded by a newer load"), "{reason}");
        }
        other => panic!("the superseded row's waiter is answered once: {other:?}"),
    }
    assert!(
        matches!(
            reply_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ),
        "exactly once"
    );
    // The new load takes the superseded place; the slot being free, the
    // terminal funnel re-gates it at once: supersede of a
    // gate-parked row hands over its queue, and an empty slot needs no wait.
    let newer = state.slots[1]
        .job
        .expect("the new load arms on the freed slot");
    assert_ne!(newer, parked, "a different row now owns the slot");
    assert_eq!(state.jobs[&newer].model, "other");
    assert_eq!(state.jobs[&newer].state, SwapState::Admit);
    assert!(state.pending.is_empty(), "the queue place became an arm");
    // The in-flight booking survives: superseding a waiting row never costs
    // the running load its work.
    let booking = state.slots[0]
        .job
        .expect("the in-flight row keeps its slot");
    assert!(
        !state.jobs[&booking].terminal,
        "the waiting row is the lighter supersede, not the running load"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// B6 rung pin, door 1: a load the card already holds admits on the
/// `FitsNow` rung — and the rung, once recorded, is what the `/har/jobs`
/// projection serves. A same-slot replacement alone never changes the
/// rung: releasing the occupant the request itself named is the request.
/// Falsifier: derive the rung before the planner arm (every admit reads
/// `FitsNow` and the drain pin goes red) or drop the `job.path`
/// assignment (this pin reads `None`).
#[tokio::test]
async fn a_fit_now_admission_records_the_fits_now_rung() {
    let dir = crate::testutil::temp_dir("rung-fits");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("small.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 4\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "small");
    state.catalog = vec![entry.clone()];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_130,
        occupant: None,
        job: None,
    }];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    assert_eq!(state.jobs[&job_id].path, None, "no decision yet, no rung");
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 2.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the roomy card admits the load: {:?}",
        job.error
    );
    assert_eq!(job.path, Some(AdmissionPath::FitsNow));
    // The ledger projection is the only source of the wire `path`, so
    // the served row must carry the same rung the decision recorded.
    let served = state
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.id == job_id)
        .expect("the row is on the ledger");
    assert_eq!(served.path, Some(AdmissionPath::FitsNow));
    assert_eq!(
        serde_json::to_value(&served).expect("JobStatus serializes")["path"],
        "FitsNow",
        "the wire name of the rung is the variant name, like every other ledger enum"
    );
}

/// B6 rung pin, door 2: a load short on its need whose room comes from
/// evictable cross-category victims admits on the `Drain` rung. The pin
/// reads the recorded rung, not `job.victims`: `dequeue_victim` moves the
/// planned victim out of the queue before the row is observed, so the
/// rung is the only honest trace left on the row.
/// Falsifier: derive the rung after the `dequeue_victim` step and this
/// row reads `FitsNow` — the pin goes red.
#[tokio::test]
async fn a_drained_admission_records_the_drain_rung() {
    let dir = crate::testutil::temp_dir("rung-drain");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("big.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 40\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "big");
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_131,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_132,
            occupant: Some(Occupant {
                model: "aux-resident".to_string(),
                need: Gibibytes(30.0),
                ram_need: None,
                port: 18_132,
                ticket: fold_ticket(18_132),
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
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    // free = 72 - 2 - 40 = 30 < need 40: the deficit is 10, and the AUX
    // occupant (AUX is evictable by WORK in the shipped matrix) covers it.
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
    assert!(
        !job.terminal,
        "the planner covers the deficit, so the load admits: {:?}",
        job.error
    );
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    assert!(!job.release_first, "a drain is not the release-first door");
    assert_eq!(
        job.victim.as_ref().map(|v| v.model.as_str()),
        Some("aux-resident"),
        "the planned victim rides the row"
    );
    assert_eq!(job.state, SwapState::Dispose);
}

/// B6R-02 rung pin, the refusal the gate never made: a load the gate
/// admitted but the launch preflight refused still names the rung the
/// gate decided. `finish_admit` derives the rung after every gate
/// refusal arm and before `preflight_spec`, so a `launch refused` row
/// honestly carries refusal AND rung — the rung is the decision record,
/// not an outcome promise. Gate refusals stay the other class: they
/// return before the derivation and carry no rung (pinned by
/// `a_floor_no_release_can_reach_refuses_before_the_victim_dies`).
/// Falsifier: clear `job.path` in the preflight refusal arm, or derive
/// the rung after `preflight_spec` — either way this row reads `None`
/// and the pin goes red.
#[tokio::test]
async fn a_launch_refused_row_keeps_the_rung_the_gate_decided() {
    let dir = crate::testutil::temp_dir("rung-launch-refused");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("small.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 4\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "small");
    state.catalog = vec![entry.clone()];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_133,
        occupant: None,
        job: None,
    }];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    // The launch preflight reads the catalog at refusal
    // time, after the gate and the rung derivation: an emptied catalog
    // drives the `SchedulerError::Spawn` refusal exactly like a row
    // deleted between scan and spawn. The roomy card lets the gate
    // pass untouched, so the refusal class is the preflight's alone.
    state.catalog.clear();
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 2.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.terminal, "the launch refusal terminalizes the row");
    assert_eq!(job.state, SwapState::AdmitFail);
    assert_eq!(job.outcome.as_deref(), Some("launch refused"));
    let error = job.error.as_ref().expect("the refusal rides the row");
    assert!(
        matches!(error, crate::scheduler::SchedulerError::Spawn(reason)
                if reason.contains("vanished from the catalog mid-job")),
        "the refusal is the preflight's, not a gate refusal: {error}"
    );
    assert_eq!(
        job.path,
        Some(AdmissionPath::FitsNow),
        "the gate decided before the preflight refused: the rung names that decision"
    );
    let served = state
        .snapshot()
        .jobs
        .into_iter()
        .find(|j| j.id == job_id)
        .expect("the row is on the ledger");
    assert_eq!(served.path, Some(AdmissionPath::FitsNow));
    assert!(
        served
            .error
            .as_ref()
            .is_some_and(|reason| reason.contains("vanished from the catalog mid-job")),
        "the served row carries refusal AND rung: {:?}",
        served.error
    );
}

/// B6 log pin, by source scan: the rung is derived and logged at exactly ONE
/// site — an `info` event named `admission path` inside the shared admitted
/// tail, carrying the rung under the field `path`. The scan covers the whole
/// file and counts the emissions, so a second `admission path` emission
/// anywhere (a re-gated card leg, a RAM leg deciding its own rung) reddens
/// this pin (the tail re-anchor alone was narrower than the
/// intent; exactly-one-emission plus the site-inside-the-tail check is the
/// complete scope, because both ladders funnel their gate through
/// `finish_admit_tail` and no other line may speak the rung). A
/// `tracing`-capture pin would add a callsite-interest flake to the suite
/// (`GOTCHAS.md` tracing rows, the D9 shape: the first touch wins
/// process-wide), so this scans the source text; the rung VALUES are pinned
/// by the decision pins and the wire shape by the `/har/jobs` integration pin.
/// Falsifier: rename the field (`rung = ?path`), demote the event out of
/// `info!`, drop the line, or emit the event a second time — each fails the scan.
#[test]
fn the_admission_path_log_line_names_the_rung_field() {
    let source = include_str!("../admission.rs");
    let emissions = source.matches("\"admission path\"").count();
    assert_eq!(
        emissions, 1,
        "the rung must ride exactly one log emission — the shared admitted tail's — \
         so both ladders announce the decision they actually took"
    );
    let at = source
        .find("\"admission path\"")
        .expect("the accepted-decision line names the event `admission path`");
    let tail = source
        .find("fn finish_admit_tail(")
        .expect("the admitted tail is still in this file");
    let after_tail = source[tail..]
        .find("fn finish_admit_ram(")
        .expect("the RAM leg still dispatches into this file");
    assert!(
        at > tail && at < tail + after_tail,
        "the one emission must sit inside the shared admitted tail, the single \
         point where a gate has said yes"
    );
    let macro_start = tail
        + source[tail..at]
            .rfind("tracing::info!(")
            .expect("the admission path line is an info event");
    let line = &source[macro_start..at];
    assert!(
        line.contains("path = ?path"),
        "the rung must ride the log under the field `path`: {line}"
    );
    assert!(line.contains("model ="), "the line names the model: {line}");
    assert!(
        line.contains("category ="),
        "the line names the category: {line}"
    );
    assert!(
        !line.contains("need_gib") && !line.contains("free_gib"),
        "the rung line quotes no numbers — the refusal and the extended \
             decision lines already carry them: {line}"
    );
}

/// A `whole` row admits on a fully-free
/// card — here the card holds exactly the driver baseline and that
/// baseline sits inside the reserve band, so the credited reading reaches
/// the whole budget minus the reserve (68 = 72 - 2 * 2). The settle twin
/// rides `gate_target`: after a release the window must see the card at
/// least `total - reserve` free.
/// Falsifier: drop the `whole` leg's auto floor and a key-less-shaped row
/// gates on need alone — `gate_target` reads 12 and this pin fails.
#[tokio::test]
async fn a_whole_row_admits_on_a_fully_free_card() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("whole-free");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("whole.yaml");
    std::fs::write(
        &yaml,
        "engine: mock\ncategory: work\nvram_gb: 12\nlaunch_only_with_at_least_vram_free: whole\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "whole");
    assert_eq!(
        entry.vram_gate,
        crate::model::VramGate::Whole,
        "the keyword must survive the real parser"
    );
    state.catalog = vec![entry.clone()];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_120,
        occupant: None,
        job: None,
    }];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 2.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "a fully-free card admits the whole row: {:?}",
        job.error
    );
    assert_eq!(
        job.gate_target,
        Some(68.0),
        "the settle twin of the whole gate is total - 2 * reserve"
    );
    assert!(
        !matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::WholeMachineBusy { .. }
            ))
        ),
        "{:?}",
        job.error
    );
}

/// The wait posture: any other holder and
/// the `whole` row refuses, naming it with the reason
/// "whole machine requested" — and evicts nothing: no victim was planned
/// (the planner is never asked on this path), no unload job is minted,
/// and the holder is still loaded after the refusal.
/// Falsifier: drop the holder scan from the whole leg and the clean-
/// sample leg admits beside the holder; drop the whole leg itself and
/// the refusal arrives as the numeric floor class — both pins go red.
/// (The `plan_victims` skip above is defense-in-depth: the holder scan
/// already guarantees an empty candidate set, so removing only the skip
/// cannot manifest — verified by mutation, suite stayed green.)
#[tokio::test]
async fn a_whole_row_waits_for_every_other_holder_and_evicts_nothing() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("whole-holder");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("whole.yaml");
    std::fs::write(
        &yaml,
        "engine: mock\ncategory: work\nvram_gb: 12\nlaunch_only_with_at_least_vram_free: whole\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "whole");
    state.catalog = vec![entry.clone()];
    let holder_ticket = fold_ticket(18_122);
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_121,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_122,
            occupant: Some(Occupant {
                model: "aux-resident".to_string(),
                need: Gibibytes(6.0),
                ram_need: None,
                port: 18_122,
                ticket: holder_ticket.clone(),
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
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 8.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(job.terminal, "any holder must refuse the whole row");
    let error = job
        .error
        .as_ref()
        .expect("the refusal travels with the row");
    let msg = error.to_string();
    assert!(
        msg.contains("whole machine requested") && msg.contains("aux-resident"),
        "{msg}"
    );
    assert!(
        msg.contains("at least 68.0 GiB free required"),
        "the refusal must print the target it needs (B5R-02): {msg}"
    );
    match error {
        SchedulerError::Admission(AdmissionError::WholeMachineBusy {
            blockers,
            wait_hint,
            ..
        }) => {
            assert!(
                blockers
                    .iter()
                    .any(|b| b.model == "aux-resident" && b.reason == "whole machine requested"),
                "the holder must be named with the ruling's reason: {blockers:?}"
            );
            // The B2 hint rides the new refusal like any other memory
            // refusal: nothing ahead quotes no place, and with no
            // confirmed release yet the estimate is absent, never zero.
            assert_eq!(wait_hint.queue_position, None, "{wait_hint:?}");
            assert_eq!(wait_hint.expected_release_s, None, "{wait_hint:?}");
        }
        other => panic!("the wait refusal has its own class: {other:?}"),
    }
    assert!(
        !msg.contains("considered evicting"),
        "the whole path never plans victims: {msg}"
    );
    assert!(
        job.victim.is_none() && job.victims.is_empty() && !job.release_first,
        "no victim of any kind was queued"
    );
    assert!(
        !msg.contains("No releases allow"),
        "the row-10 guard sentence belongs to the floor refusal only: {msg}"
    );
    assert!(
        state.jobs.values().all(|j| j.kind != JobKind::Unload),
        "the refusal names holders; it mints no unload job"
    );
    assert!(
        state.slots[1].occupant.is_some(),
        "the holder the refusal protects is still loaded"
    );
    // The holder scan is a separate reason from the arithmetic: a sample
    // that momentarily shows the card clean (attribution Unavailable —
    // exactly when the numeric floor leg would wave the row through)
    // still refuses while another slot holds a model.
    // Falsifier: drop `!holders.is_empty()` from the whole leg and this
    // leg admits — the pin goes red.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_121,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_122,
            occupant: Some(Occupant {
                model: "aux-resident".to_string(),
                need: Gibibytes(6.0),
                ram_need: None,
                port: 18_122,
                ticket: holder_ticket.clone(),
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
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 0.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    assert!(
        matches!(
            state.jobs[&job_id].error,
            Some(SchedulerError::Admission(
                AdmissionError::WholeMachineBusy { .. }
            ))
        ),
        "a momentarily-clean sample must not admit beside a holder: {:?}",
        state.jobs[&job_id].error
    );
}

/// The pinned same-slot behavior: an explicit same-slot load of a `whole` row
/// IS a replacement
/// of its own slot's occupant — the request named that target, and the
/// occupant is never an eviction "to get there". It admits when the rest
/// of the card is free; the moment ANY other slot holds something the
/// same load refuses, still evicting nothing.
/// Falsifier: count the own-slot occupant as a holder and leg 1 goes red;
/// skip the holder scan and leg 2 goes red.
#[tokio::test]
async fn a_whole_row_replaces_its_own_slot_and_waits_for_the_rest() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("whole-sameslot");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("whole.yaml");
    std::fs::write(
        &yaml,
        "engine: mock\ncategory: work\nvram_gb: 40\nlaunch_only_with_at_least_vram_free: whole\n",
    )
    .expect("seed");
    let entry = rescan_entry(&yaml, "whole");
    let seat = |state: &mut ActorState, aux: Option<&str>| {
        state.slots = vec![
            Slot {
                category: CategoryId::Work,
                index: 0,
                engine_port: 18_123,
                occupant: Some(Occupant {
                    model: "old".to_string(),
                    need: Gibibytes(30.0),
                    ram_need: None,
                    port: 18_123,
                    ticket: fold_ticket(18_123),
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
                engine_port: 18_124,
                occupant: aux.map(|name| Occupant {
                    model: name.to_string(),
                    need: Gibibytes(6.0),
                    ram_need: None,
                    port: 18_124,
                    ticket: fold_ticket(18_124),
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
    };
    let whole_state = |aux: Option<&str>| {
        let mut state = fold_state(&dir);
        state.cfg.vram_total_gb = 72.0;
        state.cfg.vram_reserve_gb = 2.0;
        state.catalog = vec![entry.clone()];
        seat(&mut state, aux);
        state
    };
    // Leg 1: only the replacement target (30) plus the baseline (2) hold
    // the card — the credited reading reaches 68 — so the load admits and
    // disposes its own victim, nothing else.
    let mut state = whole_state(None);
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 32.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the own slot may be replaced: {:?}",
        job.error
    );
    assert_eq!(job.state, SwapState::Dispose);
    assert_eq!(job.gate_target, Some(68.0));
    assert_eq!(
        job.victim.as_ref().map(|v| v.model.as_str()),
        Some("old"),
        "the same-slot replacement is the only release a whole load may perform"
    );
    assert!(job.victims.is_empty(), "never a cross-slot victim");
    // Leg 2: the same load one Aux holder later refuses naming it, and
    // both occupants survive the refusal. The sample still shows only the
    // own occupant's bytes (attribution Unavailable: the reading alone
    // would pass the numeric auto floor), so this leg pins the holder
    // scan as a SEPARATE reason — a clean-looking sample must not admit
    // a row while another slot holds a model.
    let mut state = whole_state(Some("aux-resident"));
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 32.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::WholeMachineBusy { .. }
            ))
        ),
        "one foreign slot is enough to wait: {:?}",
        job.error
    );
    let msg = job.error.as_ref().expect("refusal").to_string();
    assert!(msg.contains("aux-resident"), "{msg}");
    assert!(
        msg.contains("at least 68.0 GiB free required"),
        "the refusal must print the target it needs (B5R-02): {msg}"
    );
    assert!(
        state.slots[0].occupant.is_some() && state.slots[1].occupant.is_some(),
        "the wait posture kills nothing: both holders are still loaded"
    );
    assert!(
        state.jobs.values().all(|j| j.kind != JobKind::Unload),
        "the refusal names holders; it mints no unload job"
    );
}

/// `whole` on a greedy row is legal and REPLACES the greedy floor leg
/// (documented in MODELS.md): the global
/// `greedy_launch_min_free_gb` never applies to it. Leg 1 pins the
/// stricter verdict that replacement buys — a driver baseline above the
/// reserve denies the launch although the numeric greedy floor (6) would
/// pass the same reading; leg 2 admits on a card the reserve covers.
/// Falsifier: let `whole` fall through to the greedy default floor and
/// leg 1 admits — the pin goes red.
#[tokio::test]
async fn whole_replaces_the_greedy_floor_leg() {
    use super::{AdmissionError, SchedulerError};
    let dir = crate::testutil::temp_dir("whole-greedy");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("greedy.yaml");
    std::fs::write(
            &yaml,
            "engine: mock\ncategory: work\ncost: greedy\nvram_gb: 20\nlaunch_only_with_at_least_vram_free: whole\n",
        )
        .expect("seed");
    let entry = rescan_entry(&yaml, "greedy");
    assert_eq!(entry.vram_gate, crate::model::VramGate::Whole);
    let whole_state = || {
        let mut state = fold_state(&dir);
        state.cfg.vram_total_gb = 72.0;
        state.cfg.vram_reserve_gb = 2.0;
        state.cfg.greedy_launch_min_free_gb = 6.0;
        state.catalog = vec![entry.clone()];
        state.slots = vec![Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_125,
            occupant: None,
            job: None,
        }];
        state
    };
    // Leg 1: the card holds only a 4 GiB baseline. The greedy floor of 6
    // would let this through (66 >= 6); the whole gate demands the
    // baseline live inside the 2 GiB reserve band, so the row waits.
    let mut state = whole_state();
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 4.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::WholeMachineBusy { .. }
            ))
        ),
        "the greedy floor must NOT stand in for the whole gate: {:?}",
        job.error
    );
    // The holderless refusal (the reviewer's B5R-02 case: a baseline
    // above the reserve, nothing to name) must print the target, so the
    // operator sees why 66.0 waited.
    let msg = job.error.as_ref().expect("refusal").to_string();
    assert!(
        msg.contains("at least 68.0 GiB free required") && msg.contains("free 66.0 GiB"),
        "{msg}"
    );
    // Leg 2: a baseline the reserve covers is a fully-free card.
    let mut state = whole_state();
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.start_queued_load(job_id, 0, entry.clone());
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 2.0,
            processes: Vec::new(),
            attribution: crate::budget::AttributionQuality::Unavailable,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the reserve-covered card admits the greedy whole row: {:?}",
        job.error
    );
}

// ---------- the same-slot pre-refuse and the sole-resident planner credit
// (per the DECISIONS 2026-11-26 rows) ---------------------------------------

/// One inline occupant for the ruling pins: the shape `seat_spawn` seats, with
/// the booking and the fold figure the pin is testing written in directly.
fn ruling_resident(name: &str, need_gib: f64, live_gib: Option<f64>, port: u16) -> Occupant {
    Occupant {
        model: name.to_string(),
        need: Gibibytes(need_gib),
        ram_need: None,
        port,
        ticket: fold_ticket(port),
        loaded_at: Instant::now(),
        launched_at_wall: std::time::SystemTime::now(),
        spawn_config: None,
        stale_config: false,
        serve_id: None,
        measure: None,
        live_measured_gb: live_gib,
    }
}

/// Per the DECISIONS 2026-11-26 rows, edge case (a): a same-slot replacement whose settle
/// target sits ABOVE the reachable ceiling (`total - never_releasable`, here
/// 72 - 0 = 72) refuses at the gate BEFORE any release: the swift resident
/// (booked 24.0078, folded 23.3) is not killed for a 75 GiB load that could
/// never settle even after its bytes returned (the physical free tops out at
/// 72, the window needs 75 + 2 - 2). The refusal names the need, the target,
/// the ceiling, and the compared figure.
/// Falsifier: delete the pre-refuse arm and the row takes release-first — the
/// victim dies (the occupant-survival leg goes red) and the error is the
/// release-first absence, not the named refusal.
#[tokio::test]
async fn a_same_slot_need_the_releases_cannot_reach_refuses_before_any_release() {
    let dir = crate::testutil::temp_dir("pre-refuse-ceiling");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("big.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 75\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    let entry = rescan_entry(&yaml, "big");
    state.catalog = vec![entry.clone()];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_171,
        occupant: Some(ruling_resident("swift", 24.0078, Some(23.3), 18_171)),
        job: None,
    }];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(75.0));
        job.victim = Some(super::Victim {
            model: "swift".to_string(),
            ticket: fold_ticket(18_171),
            slot: 0,
            evicted: false,
            cost: 24.0078,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let job = &state.jobs[&job_id];
    assert!(
        matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::SettleUnreachable { .. }
            ))
        ),
        "a target above the reachable ceiling refuses before any kill: {:?}",
        job.error
    );
    let message = job.error.as_ref().expect("refusal").to_string();
    assert!(
        message.contains("settle target unreachable before release")
            && message.contains("need 75.0 GiB")
            && message.contains("settle target 75.0 GiB")
            && message.contains("ceiling 72.0 GiB")
            && message.contains("free 70.0 GiB"),
        "the refusal names need, target, ceiling, and the compared figure: {message}"
    );
    assert!(
        !job.release_first,
        "the ruling refuses INSTEAD of releasing"
    );
    assert!(
        job.path.is_none(),
        "a refusal never names a rung: {:?}",
        job.path
    );
    assert_eq!(job.state, SwapState::AdmitFail);
    assert!(
        state.slots[0].occupant.is_some(),
        "refusal before kill: the doomed-by-request resident is still seated"
    );
}

/// Per the DECISIONS 2026-11-26 rows, edge case (b): the ceiling counts what a release for
/// THIS decision provably cannot reclaim — here the protected `guard-p`
/// resident (WORK: nothing may evict it) holds a witnessed 12.0 fold, so the
/// ceiling is 72 - 12 = 60 and the 65 GiB same-slot replacement refuses naming
/// its holder. The bound counts the holder's WITNESSED bytes only (per the
/// DECISIONS 2026-11-26 rows): a bare booking is a projection, never a proven
/// byte, so `guard-p` carries the fold that proves its 12 GiB — the pin holds
/// the same refusal with the honest basis. The own-slot victim never bounds
/// its own ceiling (it IS the release), so only the protected byte counts.
/// Falsifier: drop the blocker-occupant leg of `never_releasable_holds` and
/// the ceiling reads 72: the row slips into release-first, the victim is
/// doomed, and the settle timeout arrives where the gate could have named the
/// holder.
#[tokio::test]
async fn a_proven_unreachable_ceiling_names_the_protected_holder_before_the_release() {
    let dir = crate::testutil::temp_dir("pre-refuse-holder");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("mid.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 65\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    let entry = rescan_entry(&yaml, "mid");
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_172,
            occupant: Some(ruling_resident("swift-v", 4.0, None, 18_172)),
            job: None,
        },
        Slot {
            category: CategoryId::Work,
            index: 1,
            engine_port: 18_173,
            occupant: Some(ruling_resident("guard-p", 12.0, Some(12.0), 18_173)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(65.0));
        job.victim = Some(super::Victim {
            model: "swift-v".to_string(),
            ticket: fold_ticket(18_172),
            slot: 0,
            evicted: false,
            cost: 4.0,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(35.3)));
    let job = &state.jobs[&job_id];
    assert!(
        matches!(
            job.error,
            Some(SchedulerError::Admission(
                AdmissionError::SettleUnreachable { .. }
            ))
        ),
        "65 above a 60 ceiling with a named holder refuses: {:?}",
        job.error
    );
    let message = job.error.as_ref().expect("refusal").to_string();
    assert!(
        message.contains("ceiling 60.0 GiB") && message.contains("blocked by: guard-p"),
        "the refusal names the ceiling and the never-releasable holder: {message}"
    );
    assert!(!job.release_first);
    assert!(
        state.slots[0].occupant.is_some() && state.slots[1].occupant.is_some(),
        "both residents survive a refusal that arrives before the kill"
    );
}

/// Per the DECISIONS 2026-11-26 rows, both falsifier directions: a target the release CAN
/// reach still pays the eviction and re-gates (never pre-refused).
/// Leg 1: swift (booked 13.2, folded 23.3), floor 60, exact need 70.5 — a
/// 0.5 GiB deficit with the victim already credited; target 70.5 + 2 - 2 =
/// 70.5 ≤ 72, so release-first runs (the booking-would-clear-with-release
/// shape: a release DID happen). Leg 2, edge case (d): the same card with a
/// 67.4 need under the 60 floor has no deficit at all — the gate admits and
/// the same-slot replacement releases and re-gates (never refused).
/// Falsifier: widen the pre-refuse predicate (drop the ceiling compare) and
/// leg 1 refuses instead of going release-first — the rung and the Dispose
/// legs go red.
#[tokio::test]
async fn a_ceiling_reachable_target_still_pays_the_release_first() {
    let dir = crate::testutil::temp_dir("pre-refuse-release-first");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let swift_yaml = dir.join("swift.yaml");
    std::fs::write(&swift_yaml, "engine: mock\ncategory: work\nvram_gb: 13.2\n").expect("seed");
    let big_yaml = dir.join("floor.yaml");
    std::fs::write(
        &big_yaml,
        "engine: mock\ncategory: work\nvram_gb: 70.5\nlaunch_only_with_at_least_vram_free: 60\n",
    )
    .expect("seed");
    let swift = rescan_entry(&swift_yaml, "swift");
    let entry = rescan_entry(&big_yaml, "floor");
    assert_eq!(
        entry.vram_gate.floor(),
        Some(60.0),
        "floor survives the scan"
    );

    // Leg 1: release-first on a reachable ceiling.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    state.catalog = vec![swift.clone(), entry.clone()];
    let ticket = fold_ticket(18_174);
    seat_spawn(&mut state, &swift, &ticket, 18_174);
    if let Some(o) = state.slots[0].occupant.as_mut() {
        o.need = Gibibytes(13.2);
        o.live_measured_gb = Some(23.3);
    }
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(70.5));
        job.victim = Some(super::Victim {
            model: "swift".to_string(),
            ticket: ticket.clone(),
            slot: 0,
            evicted: false,
            cost: 13.2,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "a target inside the ceiling is release-first, not a refusal: {:?}",
        job.error
    );
    assert!(job.release_first, "the release-first door stays first");
    assert_eq!(job.path, Some(AdmissionPath::ReleaseFirst));
    assert_eq!(job.state, SwapState::Dispose, "the release DID happen");

    // Leg 2, edge case (d): need 67.4 under the same floor and the same
    // credited figure — no deficit, so the ordinary drain-replace runs.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    state.catalog = vec![swift.clone(), entry.clone()];
    let ticket = fold_ticket(18_175);
    seat_spawn(&mut state, &swift, &ticket, 18_175);
    if let Some(o) = state.slots[0].occupant.as_mut() {
        o.need = Gibibytes(13.2);
        o.live_measured_gb = Some(23.3);
    }
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(67.4));
        job.victim = Some(super::Victim {
            model: "swift".to_string(),
            ticket: ticket.clone(),
            slot: 0,
            evicted: false,
            cost: 13.2,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal && job.error.is_none(),
        "the ruling never pre-refuses a replacement the card can settle: {:?}",
        job.error
    );
    assert_eq!(
        job.state,
        SwapState::Dispose,
        "the replacement still releases"
    );
}

/// Per the DECISIONS 2026-11-26 rows, edge case (c): a shortfall covered by another load's
/// in-flight booking, with no same-slot victim, still PARKS — the pre-refuse
/// (which requires a victim) never turns a wait into a refusal, even when the
/// card also holds a protected resident the planner names as a blocker.
/// Falsifier: make the pre-refuse victim-optional and this row terminalizes
/// `AdmitFail` where it used to wait: the non-terminal and pending legs red.
#[tokio::test]
async fn a_booking_covered_shortfall_without_a_victim_still_parks_not_refuses() {
    let dir = crate::testutil::temp_dir("pre-refuse-park");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("waiter.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 45\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    let entry = rescan_entry(&yaml, "waiter");
    state.catalog = vec![entry.clone()];
    state.slots = (0..3)
        .map(|index| Slot {
            category: CategoryId::Work,
            index,
            engine_port: 18_180 + index as u16,
            occupant: (index == 0).then(|| ruling_resident("swift", 13.2, Some(23.3), 18_180)),
            job: None,
        })
        .collect();
    // The in-flight booking on slot 1: armed like `start_queued_load` arms a
    // row, no occupant installed yet (the witness already counts its bytes).
    let booking_yaml = dir.join("booking.yaml");
    std::fs::write(&booking_yaml, "engine: mock\ncategory: work\nvram_gb: 30\n").expect("seed");
    let booking_entry = rescan_entry(&booking_yaml, "booking");
    state.catalog.push(booking_entry.clone());
    let booking = state.register_queued_load(1, &booking_entry, Vec::new());
    state.slots[1].job = Some(booking);
    state.jobs.get_mut(&booking).expect("booking row").need = Some(Gibibytes(30.0));
    state.step(booking, SwapState::Admit);
    // The waiting row on slot 2: free = 72 - 2 - 23.3 - 30 = 16.7 against
    // need 45; swift is a named blocker (nothing evicts WORK) and the 30 GiB
    // booking covers the 28.3 deficit — the park rule, unchanged.
    let job_id = state.register_queued_load(2, &entry, Vec::new());
    state.slots[2].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(45.0));
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let row = &state.jobs[&job_id];
    assert!(
        !row.terminal && row.error.is_none(),
        "a booking-covered deficit without a victim waits: {:?}",
        row.error
    );
    assert_eq!(row.state, SwapState::Idle, "the row is parked, not refused");
    assert_eq!(state.pending.get(&2), Some(&job_id));
    assert!(
        state.slots[0].occupant.is_some(),
        "the protected swift byte-holder is untouched by the park"
    );
}

/// Per the DECISIONS 2026-11-26 rows, the sole-resident planner credit: the only installed
/// GPU resident on a real reading is planned at `max(booking, its own live
/// bytes)`, so a drain the booking alone cannot cover admits on the witness
/// figure. Slot 1 holds a swift AUX resident booked 13.2 but folded 23.3;
/// the 68.0 load sees free 46.7 (deficit 21.3) — the booking 13.2 is short,
/// the live figure 23.3 covers, and the planner drains the resident (the
/// pre-ruling shape refused this load at the gate).
/// Falsifier: drop the sole-resident credit and the plan runs on 13.2 — the
/// refusal returns, the rung leg and the victim leg go red.
#[tokio::test]
async fn the_sole_gpu_resident_is_planned_at_its_live_footprint_not_its_booking() {
    let dir = crate::testutil::temp_dir("sole-resident-credit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("need68.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 68\n").expect("seed");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    let entry = rescan_entry(&yaml, "need68");
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_185,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_186,
            occupant: Some(ruling_resident("swift-holder", 13.2, Some(23.3), 18_186)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(68.0));
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the live figure covers the deficit the booking cannot: {:?}",
        job.error
    );
    assert_eq!(job.path, Some(AdmissionPath::Drain));
    assert_eq!(
        job.victim.as_ref().map(|v| v.model.as_str()),
        Some("swift-holder"),
        "the sole resident is the planned victim"
    );
    assert_eq!(job.state, SwapState::Dispose);
}

/// Per the DECISIONS 2026-11-26 rows, the credit's three bounds, read from the refusal each
/// one leaves behind. Leg A (never count foreign bytes): per-process
/// attribution shows the resident holding 13.2 of 23.2 used — the 10.0
/// foreign GiB never enters the figure, so a 61.0 load (deficit 14.2) still
/// refuses and the report names 13.2, not 23.2. Leg B (the `max()` keeps the
/// booking): the over-booked swift (24.0078 booked, 23.3 folded) reports
/// 24.0 — the clamp applies to the witness term only, exactly as the ruling
/// sentence says. Leg C (ledger-only mode unchanged): with no witness the
/// figure is the booking — the same resident shape reports 13.2, never its
/// fold.
/// Falsifier: credit `used_gib` instead of `own minus foreign` and leg A
/// admits; clamp the whole figure to the witness and leg B reads 23.3; let
/// the credit fire on a `None` sample and leg C reads 23.3 — each leg goes
/// red with its named figure.
#[tokio::test]
async fn the_sole_resident_figure_never_counts_foreign_and_the_booking_stays_the_floor() {
    let dir = crate::testutil::temp_dir("sole-resident-bounds");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("need.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 61\n").expect("seed");
    let need61 = rescan_entry(&yaml, "need");
    let yaml90 = dir.join("need90.yaml");
    std::fs::write(&yaml90, "engine: mock\ncategory: work\nvram_gb: 90\n").expect("seed");
    let need90 = rescan_entry(&yaml90, "need90");

    // Leg A: per-process attribution — foreign bytes never credited.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.catalog = vec![need61.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_187,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_188,
            occupant: Some(ruling_resident("own-pid", 13.2, None, 18_188)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &need61, Vec::new());
    state.slots[0].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(61.0));
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 23.2,
            processes: vec![(4242, 13.2), (9999, 10.0)],
            attribution: crate::budget::AttributionQuality::PerProcess,
        }),
    );
    let job = &state.jobs[&job_id];
    assert!(
        job.terminal,
        "13.2 attributable, 14.2 short: still a refusal"
    );
    let message = job.error.as_ref().expect("refusal").to_string();
    assert!(
        message.contains("insufficient VRAM") && message.contains("own-pid (AUX), 13.2 GiB"),
        "the report credits the attributed figure only, never the 23.2 used: {message}"
    );

    // Leg B: over-booked victim — the max() keeps the booking.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.catalog = vec![need90.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_189,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_190,
            occupant: Some(ruling_resident("swift-holder", 24.0078, Some(23.3), 18_190)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &need90, Vec::new());
    state.slots[0].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(90.0));
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let job = &state.jobs[&job_id];
    assert!(job.terminal, "90 is short of even 24.0078 releasable");
    let message = job.error.as_ref().expect("refusal").to_string();
    assert!(
        message.contains("swift-holder (AUX), 24.0 GiB"),
        "the over-booked victim keeps its booking: {message}"
    );

    // Leg C: ledger-only mode — the figure stays the booking, the fold never
    // enters without a witness.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.catalog = vec![need90.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_191,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_192,
            occupant: Some(ruling_resident("swift-holder", 13.2, Some(23.3), 18_192)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &need90, Vec::new());
    state.slots[0].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(90.0));
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Err("ledger-only: no witness".to_string()));
    let job = &state.jobs[&job_id];
    assert!(
        job.terminal,
        "the ledger route still refuses the short load"
    );
    let message = job.error.as_ref().expect("refusal").to_string();
    assert!(
        message.contains("swift-holder (AUX), 13.2 GiB"),
        "no witness means no witness figure: the booking is planned: {message}"
    );
}

/// Per the DECISIONS 2026-11-26 rows, the bounds of the sole test: it counts INSTALLED GPU
/// residents. Leg A: an in-flight paper booking does not disqualify the one
/// installed resident from the credit, and the paper row is never a victim
/// and never a byte of the figure — the drain plans the resident alone.
/// Leg B: with TWO installed GPU residents the credit is off entirely —
/// every figure stays exactly its booking, so a 61.0 load whose combined
/// bookings (13.2 + 5.0 = 18.2) miss the 19.2 deficit refuses naming both
/// booking figures, not their larger folds.
/// Falsifier: drop the sole test and leg B admits on 27.3 of credit; count
/// paper rows as residents and leg A's credit (and its drain) disappear.
#[tokio::test]
async fn the_sole_test_counts_installed_residents_and_two_residents_stay_bookings() {
    let dir = crate::testutil::temp_dir("sole-test-bounds");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("need.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 68\n").expect("seed");
    let need68 = rescan_entry(&yaml, "need68");
    let yaml61 = dir.join("need61.yaml");
    std::fs::write(&yaml61, "engine: mock\ncategory: work\nvram_gb: 61\n").expect("seed");
    let need61 = rescan_entry(&yaml61, "need61");

    // Leg A: the paper booking neither qualifies nor disqualifies.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.catalog = vec![need68.clone()];
    state.slots = (0..3)
        .map(|index| Slot {
            category: if index == 1 {
                CategoryId::Aux
            } else {
                CategoryId::Work
            },
            index,
            engine_port: 18_195 + index as u16,
            occupant: (index == 1)
                .then(|| ruling_resident("swift-holder", 13.2, Some(23.3), 18_196)),
            job: None,
        })
        .collect();
    let booking = state.register_queued_load(2, &need68, Vec::new());
    state.slots[2].job = Some(booking);
    state.jobs.get_mut(&booking).expect("booking row").need = Some(Gibibytes(20.0));
    state.step(booking, SwapState::Admit);
    let job_id = state.register_queued_load(0, &need68, Vec::new());
    state.slots[0].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(48.0));
    state.step(job_id, SwapState::Admit);
    // free = 72 - 2 - 23.3 - 20 (in-flight) = 26.7; deficit 21.3; the sole
    // resident's live 23.3 covers it (its 13.2 booking would not).
    state.finish_admit_card(job_id, Ok(gate_sample(23.3)));
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the credit survives the paper booking and covers the deficit: {:?}",
        job.error
    );
    assert_eq!(
        job.victim.as_ref().map(|v| v.model.as_str()),
        Some("swift-holder"),
        "the installed resident is the victim; paper is never plannable"
    );

    // Leg B: two residents, bookings only.
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.catalog = vec![need61.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_200,
            occupant: None,
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_201,
            occupant: Some(ruling_resident("swift-holder", 13.2, Some(23.3), 18_201)),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 2,
            engine_port: 18_202,
            occupant: Some(ruling_resident("slow-holder", 5.0, Some(4.0), 18_202)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &need61, Vec::new());
    state.slots[0].job = Some(job_id);
    state.jobs.get_mut(&job_id).expect("row").need = Some(Gibibytes(61.0));
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(28.2)));
    let job = &state.jobs[&job_id];
    assert!(
        job.terminal,
        "18.2 of booking cannot cover a 19.2 deficit when the credit is off: {:?}",
        job.error
    );
    let message = job.error.as_ref().expect("refusal").to_string();
    assert!(
        message.contains("swift-holder (AUX), 13.2 GiB")
            && message.contains("slow-holder (AUX), 5.0 GiB"),
        "two residents: every figure stays its booking, never its fold: {message}"
    );
}

/// Per the DECISIONS 2026-11-26 rows, the occupant-identity rule on the bound:
/// the same model resident in two slots is a supported state (the twin pins in
/// `workset.rs` seat one on purpose). The slot-1 twin sits inside its
/// `min_turn` guard — a named never-releasable holder — while the slot-2 twin
/// is past the guard, a live candidate whose bytes this decision's release
/// COULD reclaim. The planner refuses holders by slot, never by model name, so
/// only the guard-holder's witnessed 12.0 bounds the ceiling (72 - 12 = 60)
/// and the 58.0 target inside it still takes release-first. A name-matching
/// bound would also count the candidate slot's 5.0 — the very bytes a release
/// reclaims — read the ceiling as 55.0, and refuse a swap whose settle gate
/// can pass: the wrong refusal the ruling sentence forbids.
/// Falsifier (red on name-matching holders): restore the model-name join in
/// `never_releasable_holds` and this row refuses `SettleUnreachable` — the
/// non-terminal, release-first and Dispose legs all go red.
#[tokio::test]
async fn a_same_model_candidate_in_another_slot_is_never_counted_as_never_releasable() {
    let dir = crate::testutil::temp_dir("pre-refuse-identity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let twin_yaml = dir.join("twin.yaml");
    std::fs::write(
        &twin_yaml,
        "engine: mock\ncategory: aux\nvram_gb: 5\nmin_turn: 3600\n",
    )
    .expect("seed");
    let big_yaml = dir.join("twinload.yaml");
    std::fs::write(&big_yaml, "engine: mock\ncategory: work\nvram_gb: 58\n").expect("seed");
    let twin = rescan_entry(&twin_yaml, "twin");
    assert_eq!(twin.min_turn_s, Some(3600), "the guard knob scans");
    let entry = rescan_entry(&big_yaml, "twinload");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    state.catalog = vec![twin.clone(), entry.clone()];
    let mut stale_twin = ruling_resident("twin", 5.0, Some(5.0), 18_212);
    // Past the guard: served far longer than the knob, so the planner treats
    // this twin as the evictable candidate, never as the blocked holder. Its
    // witnessed 5.0 is the bait a name join would wrongly count as
    // never-releasable; the index-identified bound must ignore it.
    stale_twin.loaded_at = Instant::now() - Duration::from_secs(7200);
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_210,
            occupant: Some(ruling_resident("swift-v", 4.0, None, 18_210)),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 1,
            engine_port: 18_211,
            occupant: Some(ruling_resident("twin", 12.0, Some(12.0), 18_211)),
            job: None,
        },
        Slot {
            category: CategoryId::Aux,
            index: 2,
            engine_port: 18_212,
            occupant: Some(stale_twin),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(58.0));
        job.victim = Some(super::Victim {
            model: "swift-v".to_string(),
            ticket: fold_ticket(18_210),
            slot: 0,
            evicted: false,
            cost: 4.0,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(45.0)));
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the candidate slot's 5.0 belongs to the release, not to the bound: \
         58.0 fits the 60.0 ceiling: {:?}",
        job.error
    );
    assert!(
        job.error.is_none() && job.release_first,
        "a target inside the identity-correct ceiling takes release-first: {:?}",
        job.error
    );
    assert_eq!(job.state, SwapState::Dispose, "the release DID happen");
}

/// Per the DECISIONS 2026-11-26 rows, the bound claims witnessed bytes only:
/// the over-booked swift class (the DECISIONS 2026-11-22 row books Fit
/// 24.0078125 while the card held 23.3). The matrix-refused blocker booked
/// 24.0078 but its own fold — the single figure the card proves for it — is
/// 23.3, so the never-releasable bound is 23.3 and the ceiling is 48.7, not
/// the booking-based 47.9922. A 48.5 settle target sits inside the
/// witnessed ceiling and outside the booking-based one: release-first runs,
/// the 0.7 GiB booking excess never becomes a refusal band. The bound is a
/// lower bound on unreclaimable bytes, so claiming un-witnessed booking
/// excess would refuse exactly the swaps some witness reading could settle —
/// the direction the fail-closed asymmetry forbids.
/// Falsifier (red on the booking-based term): restore `max(fold, booking)`
/// in `never_releasable_holds` and the ceiling reads 47.9922 — 48.5 above it
/// refuses `SettleUnreachable` and every leg here goes red.
#[tokio::test]
async fn the_over_booked_blocker_claims_no_unwitnessed_bytes_in_the_ceiling() {
    let dir = crate::testutil::temp_dir("pre-refuse-overbooked");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("over.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 48.5\n").expect("seed");
    let entry = rescan_entry(&yaml, "over");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    state.catalog = vec![entry.clone()];
    state.slots = vec![
        Slot {
            category: CategoryId::Work,
            index: 0,
            engine_port: 18_213,
            occupant: Some(ruling_resident("swift-v", 10.0, Some(10.0), 18_213)),
            job: None,
        },
        Slot {
            category: CategoryId::Work,
            index: 1,
            engine_port: 18_214,
            occupant: Some(ruling_resident("swift-over", 24.0078, Some(23.3), 18_214)),
            job: None,
        },
    ];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(48.5));
        job.victim = Some(super::Victim {
            model: "swift-v".to_string(),
            ticket: fold_ticket(18_213),
            slot: 0,
            evicted: false,
            cost: 10.0,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(job_id, Ok(gate_sample(33.3)));
    let job = &state.jobs[&job_id];
    assert!(
        !job.terminal,
        "the 0.7 booking excess may not shrink the ceiling past a settle-reachable \
         target: {:?}",
        job.error
    );
    assert!(
        job.error.is_none() && job.release_first,
        "the booking excess is a projection the bound never claims: release-first: {:?}",
        job.error
    );
    assert_eq!(job.state, SwapState::Dispose, "the release DID happen");
}

/// Per the DECISIONS 2026-11-26 rows, the message names holders only when a
/// holder exists: when the bound rides foreign per-process bytes only, no
/// harness holder was named, and the refusal says so — it names the need and
/// the ceiling, quantifies the unreleasable bound, and points the operator at
/// the foreign residents instead of printing an empty holder list.
/// Falsifier: revert the pre-refuse tail to the bare blocker list and the
/// message prints no reason at all for a foreign-only bound — the
/// `check foreign residents` leg goes red while the blocker-naming legs of
/// the other pins stay green (they name a holder and never take this arm).
#[tokio::test]
async fn a_foreign_only_bound_names_no_holder_and_points_at_foreign_residents() {
    let dir = crate::testutil::temp_dir("pre-refuse-foreign-only");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("huge.yaml");
    std::fs::write(&yaml, "engine: mock\ncategory: work\nvram_gb: 74\n").expect("seed");
    let entry = rescan_entry(&yaml, "huge");
    let mut state = fold_state(&dir);
    state.cfg.vram_total_gb = 72.0;
    state.cfg.vram_reserve_gb = 2.0;
    state.cfg.witness_tolerance_gb = 2.0;
    state.catalog = vec![entry.clone()];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_215,
        occupant: Some(ruling_resident("swift", 13.2, Some(23.3), 18_215)),
        job: None,
    }];
    let job_id = state.register_queued_load(0, &entry, Vec::new());
    state.slots[0].job = Some(job_id);
    if let Some(job) = state.jobs.get_mut(&job_id) {
        job.need = Some(Gibibytes(74.0));
        job.victim = Some(super::Victim {
            model: "swift".to_string(),
            ticket: fold_ticket(18_215),
            slot: 0,
            evicted: false,
            cost: 13.2,
        });
    }
    state.step(job_id, SwapState::Admit);
    state.finish_admit_card(
        job_id,
        Ok(crate::budget::MemorySample {
            total_gib: 72.0,
            used_gib: 23.3,
            processes: vec![(999_999, 12.0)],
            attribution: crate::budget::AttributionQuality::PerProcess,
        }),
    );
    let job = &state.jobs[&job_id];
    let error = match &job.error {
        Some(SchedulerError::Admission(AdmissionError::SettleUnreachable { blockers, .. })) => {
            assert!(
                blockers.is_empty(),
                "a foreign-only bound has no harness holder to name: {blockers:?}"
            );
            job.error.as_ref().expect("refusal")
        }
        other => panic!("the foreign-only bound still pre-refuses: {other:?}"),
    };
    let message = error.to_string();
    assert!(
        message.contains("need 74.0 GiB") && message.contains("ceiling 60.0 GiB"),
        "the refusal keeps need and ceiling even with no holder: {message}"
    );
    assert!(
        message.contains("12.0 GiB unreleasable"),
        "the bound quantifies the foreign bytes it counted: {message}"
    );
    assert!(
        !message.contains("blocked by:"),
        "no holder was named, so the message names none: {message}"
    );
    assert!(
        message.contains("check foreign residents"),
        "with no harness holder the message points at the foreign residents: {message}"
    );
}
