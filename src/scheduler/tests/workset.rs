use super::*;

// ---------------------------------------------------------------------------
// The CPU-resident working set. The process-tree probe feeds a `measured_ram_gb`
// pair that books as max truth over the declared floor, on the serve cadence
// and the symmetric one-GiB band — and never crosses into the card ledger.
// ---------------------------------------------------------------------------

/// Seats a spawn as a RAM resident: `seat_spawn` then stamps the RAM booking
/// the CPU gate wrote at admit, on both the job row and the occupant. Also
/// swaps the working-set probe for a call-counting fake: a seated pin then
/// structurally cannot reach the live Win32 query, so "no gate test calls the
/// real API" is a fact of the seam, not a cadence inference.
/// The returned handle lets the pin assert the fake's call count too.
fn seat_ram_spawn(
    state: &mut ActorState,
    entry: &ModelEntry,
    ticket: &SpawnTicket,
    port: u16,
    booked_gib: f64,
) -> Arc<FakeWorkingSetProbe> {
    seat_spawn(state, entry, ticket, port);
    for job in state.jobs.values_mut() {
        job.ram_need = Some(Gibibytes(booked_gib));
    }
    for slot in state.slots.iter_mut() {
        if let Some(occupant) = slot.occupant.as_mut() {
            occupant.ram_need = Some(Gibibytes(booked_gib));
        }
    }
    let probe = Arc::new(FakeWorkingSetProbe::new(0, 0));
    state.working_set_probe = Arc::clone(&probe) as Arc<dyn crate::process::WorkingSetProbe>;
    probe
}

/// The pin seam for one trusted tree reading, exactly as the serve sampler
/// sends it (`ActorMsg::MeasuredRam`).
fn ram_fold(state: &mut ActorState, model: &str, ticket: &SpawnTicket, ram_gib: f64) {
    state.on_measured_ram(
        model,
        ticket,
        Gibibytes(ram_gib),
        crate::model::MeasuredAttribution::PerProcess,
    );
}

/// The pin seam for the reading class the fold must refuse: a host
/// `total_diff` names no process, so it may not book, persist, or re-book.
fn ram_diff(state: &mut ActorState, model: &str, ticket: &SpawnTicket, ram_gib: f64) {
    state.on_measured_ram(
        model,
        ticket,
        Gibibytes(ram_gib),
        crate::model::MeasuredAttribution::TotalDiff,
    );
}

/// Working-set probe for the RAM pins: per-pid byte answers, a readability
/// switch (the probe refusing a pid is the tree-shrink class), and a call
/// count so a cadence pin can prove what the loop actually asked.
struct FakeWorkingSetProbe {
    root: std::sync::atomic::AtomicU64,
    child: u64,
    root_readable: bool,
    child_readable: bool,
    calls: std::sync::atomic::AtomicUsize,
}

impl FakeWorkingSetProbe {
    fn new(root_bytes: u64, child_bytes: u64) -> Self {
        Self {
            root: std::sync::atomic::AtomicU64::new(root_bytes),
            child: child_bytes,
            root_readable: true,
            child_readable: true,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn set_root(&self, bytes: u64) {
        self.root.store(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl crate::process::WorkingSetProbe for FakeWorkingSetProbe {
    fn working_set_bytes(&self, pid: u32) -> Option<u64> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match pid {
            4242 if self.root_readable => {
                Some(self.root.load(std::sync::atomic::Ordering::Relaxed))
            }
            4243 if self.child_readable => Some(self.child),
            _ => None,
        }
    }
}

/// The ladder itself (max truth over the floor): the pair books above the
/// declared `ram_gb`, never undercuts it, and stands alone where no floor
/// exists. Falsifier: a measured-only booking loses the second leg (6 books
/// over a declared 10).
#[test]
fn ram_booking_takes_the_max_of_the_floor_and_the_pair() {
    let pair_above = ModelEntry::from_flat(
        &BTreeMap::from([
            ("engine".to_string(), "llama".to_string()),
            ("cpu_offload".to_string(), "true".to_string()),
            ("ram_gb".to_string(), "10".to_string()),
            ("measured_ram_gb".to_string(), "14".to_string()),
            ("measured_ram_gb_min".to_string(), "11".to_string()),
        ]),
        "cpu",
    )
    .expect("valid");
    assert_eq!(
        super::ActorState::ram_booking(&pair_above).expect("both bases"),
        Gibibytes(14.0),
        "the trusted working set books above the declared floor"
    );
    let pair_below = ModelEntry {
        measured_ram_gb: Some(6.0),
        ..pair_above.clone()
    };
    assert_eq!(
        super::ActorState::ram_booking(&pair_below).expect("both bases"),
        Gibibytes(10.0),
        "a reading below the declared floor never undercuts it"
    );
    let floorless = ModelEntry {
        ram_gb: None,
        ..pair_above.clone()
    };
    assert_eq!(
        super::ActorState::ram_booking(&floorless).expect("the pair is a basis"),
        Gibibytes(14.0),
        "a trusted pair alone books: the row never claimed a number to keep"
    );
}

/// The band predicate at its edges (the sampler-side cadence rule): inside
/// and at the GiB boundary is noise, only strictly past it moves.
#[test]
fn the_ram_band_holds_the_noise_and_moves_only_past_the_gib() {
    assert!(!super::ActorState::ram_rebook_due(13.5, 14.0), "0.5 fall");
    assert!(
        !super::ActorState::ram_rebook_due(13.0, 14.0),
        "exactly the band is still inside it"
    );
    assert!(
        super::ActorState::ram_rebook_due(12.9, 14.0),
        "past it falls"
    );
    assert!(
        super::ActorState::ram_rebook_due(15.1, 14.0),
        "past it rises"
    );
    assert_eq!(
        super::ActorState::RAM_REBOOK_BAND_GIB,
        1.0,
        "the sampler band is one GiB; the ruling's standing allowance is the \
         gate's reserve, a different number (DESIGN: \"The reserve band is \
         the standing allowance\")"
    );
}

/// The unit discipline in one conversion (bytes at the probe, GiB everywhere
/// else) and the tree's honest sum: every readable pid counts once, a child
/// the probe cannot answer is a shrunk tree, and nothing readable at all is
/// absence. Falsifiers: a `/ 1024^2` (MiB) conversion fails the boundary, and
/// dropping the unreadable-pid skip turns the shrunk-tree leg into a miss.
#[tokio::test]
async fn the_working_set_converts_once_and_sums_the_tree_it_can_read() {
    assert_eq!(
        super::ActorState::working_set_to_gib(1u64 << 30),
        Gibibytes(1.0),
        "one binary gibibyte, not a decimal gigabyte and not MiB"
    );
    assert_eq!(
        super::ActorState::working_set_to_gib(1u64 << 40),
        Gibibytes(1024.0),
        "the same divide scales"
    );
    let runtime = Arc::new(StatusRuntime {
        status: None,
        children: vec![4243],
    }) as Arc<dyn crate::engine::EngineRuntime>;
    let both = Arc::new(FakeWorkingSetProbe::new(1 << 30, 1 << 29))
        as Arc<dyn crate::process::WorkingSetProbe>;
    assert_eq!(
        super::ActorState::tree_working_set_gib(&runtime, &both, 4242).await,
        Some(Gibibytes(1.5)),
        "engine pid plus descendant, summed before the one conversion"
    );
    let shrunk = Arc::new(FakeWorkingSetProbe {
        child_readable: false,
        ..FakeWorkingSetProbe::new(1 << 30, 1 << 29)
    }) as Arc<dyn crate::process::WorkingSetProbe>;
    assert_eq!(
        super::ActorState::tree_working_set_gib(&runtime, &shrunk, 4242).await,
        Some(Gibibytes(1.0)),
        "a child that died between the walk and the query shrinks the tree honestly"
    );
    let gone = Arc::new(FakeWorkingSetProbe {
        root_readable: false,
        child_readable: false,
        ..FakeWorkingSetProbe::new(1 << 30, 1 << 29)
    }) as Arc<dyn crate::process::WorkingSetProbe>;
    assert_eq!(
        super::ActorState::tree_working_set_gib(&runtime, &gone, 4242).await,
        None,
        "nothing readable is absence: the caller ends the sampler on it"
    );
}

/// The cadence: booked at 8.0, the tree answers 4.5 (past the band, it
/// moves), then 4.75 (a 0.25 move inside the band, silence), then 6.5 (past
/// it, it moves again) — and the reading that crosses is `per_process` tree
/// truth in the GiB newtype. Falsifiers: a band that is always true lands a
/// message inside the silence window; a loop that skips `ram_rebook_due`
/// does the same; a MiB conversion turns 4.5 into 4608 and fails the figure.
#[tokio::test]
async fn the_ram_refresh_polls_on_the_band_and_sends_only_trusted_bytes() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(StatusRuntime {
        status: None,
        children: vec![4243],
    }) as Arc<dyn crate::engine::EngineRuntime>;
    let scripted = Arc::new(FakeWorkingSetProbe::new(4 << 30, 1 << 29));
    let ticket = fold_ticket(19_051);
    let ctx = sampler_ctx("cadence", "llama", ticket.clone());
    let sampler_probe = Arc::clone(&scripted) as Arc<dyn crate::process::WorkingSetProbe>;
    let sampler = tokio::spawn(async move {
        super::ActorState::ram_refresh(
            &runtime,
            &tx,
            &ctx,
            &sampler_probe,
            8.0,
            Duration::from_millis(40),
        )
        .await;
    });
    async fn next_ram(rx: &mut mpsc::UnboundedReceiver<ActorMsg>) -> (f64, bool) {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the crossed figure reaches the actor")
            .expect("actor channel open");
        let ActorMsg::MeasuredRam {
            ram_gib,
            attribution,
            ..
        } = msg
        else {
            panic!("expected a RAM reading");
        };
        (
            ram_gib.0,
            attribution == crate::model::MeasuredAttribution::PerProcess,
        )
    }
    let (rise, trusted) = next_ram(&mut rx).await;
    assert!(
        (rise - 4.5).abs() < 1e-9 && trusted,
        "the first crossing re-books, under the process-tree provenance: {rise}"
    );
    scripted.set_root((4 << 30) + (1 << 28)); // 4.75 total: 0.25 inside the band
    assert!(
        tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .is_err(),
        "in-band moves are noise: the sampler sends nothing, only asks"
    );
    scripted.set_root(6 << 30); // 6.5 total: 2.0 past the band
    let (fall, _) = next_ram(&mut rx).await;
    assert!(
        (fall - 6.5).abs() < 1e-9,
        "the symmetric band guards both ways: {fall}"
    );
    assert!(scripted.calls() > 0, "the loop really walked the tree");
    sampler.abort();
}

/// The whole trusted fold: a CPU row's tree reading persists only the RAM
/// pair on the shared stamp, re-books the resident by max truth, touches no
/// card key, and a later `total_diff` reading is refused outright — nothing
/// books, nothing persists. Falsifiers: dropping the provenance guard books
/// 20.0; writing the Vram pair from the Ram fold fails the key absence.
#[tokio::test]
async fn a_ram_fold_persists_its_own_pair_and_refuses_a_total_diff() {
    let dir = crate::testutil::temp_dir("ram-fold");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("cpu.yaml");
    std::fs::write(&yaml, "engine: llama\ncpu_offload: true\nram_gb: 8\n").expect("seed");
    let mut state = fold_state(&dir);
    state.ram_witness = Arc::new(ScriptedRamWitness::default());
    let entry = rescan_entry(&yaml, "cpu");
    assert_eq!(entry.resource(), crate::model::Resource::Ram);
    let ticket = fold_ticket(19_052);
    state.catalog = vec![entry.clone()];
    let probe = seat_ram_spawn(&mut state, &entry, &ticket, 19_052, 8.0);

    ram_fold(&mut state, "cpu", &ticket, 14.0);
    let text = await_yaml_keys(
        &yaml,
        &[
            "measured_ram_gb: 14.0",
            "measured_ram_gb_min: 14.0",
            "measured_offload: true",
        ],
    )
    .await;
    assert!(
        !text.contains("measured_gb:"),
        "a RAM fold never writes the card pair:\n{text}"
    );
    assert!(
        !text.contains("measured_attribution"),
        "the RAM pair carries no attribution stamp — this guard is its proof:\n{text}"
    );
    assert_eq!(
        state.slots[0].occupant.as_ref().expect("resident").ram_need,
        Some(Gibibytes(14.0)),
        "max truth re-books the resident above its 8 GiB floor"
    );

    ram_diff(&mut state, "cpu", &ticket, 20.0);
    assert_eq!(
        state.catalog[0].measured_ram_gb,
        Some(14.0),
        "a host delta names no process: the pair stands at the tree truth"
    );
    assert_eq!(
        state.slots[0].occupant.as_ref().expect("resident").ram_need,
        Some(Gibibytes(14.0)),
        "and nothing re-books from it"
    );
    assert!(
        !std::fs::read_to_string(&yaml)
            .expect("readable")
            .contains("20.0"),
        "and nothing persists from it"
    );
    assert_eq!(
        probe.calls(),
        0,
        "a seated pin never reaches a working-set query — the fake the seat \
         swapped in stayed silent, so the live probe is unreachable by \
         structure, not by cadence (W23-02)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The floor's own pin: a trusted reading below the declared `ram_gb` still
/// persists (it is truth about the process) but never undercuts the booking
/// the row declared. Falsifier: a measured-only booking drops to 5.0 here.
#[tokio::test]
async fn a_ram_reading_below_the_declared_floor_persists_without_undercutting() {
    let dir = crate::testutil::temp_dir("ram-floor");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("cpu2.yaml");
    std::fs::write(&yaml, "engine: llama\ncpu_offload: true\nram_gb: 8\n").expect("seed");
    let mut state = fold_state(&dir);
    state.ram_witness = Arc::new(ScriptedRamWitness::default());
    let entry = rescan_entry(&yaml, "cpu2");
    let ticket = fold_ticket(19_053);
    state.catalog = vec![entry.clone()];
    let probe = seat_ram_spawn(&mut state, &entry, &ticket, 19_053, 8.0);

    ram_fold(&mut state, "cpu2", &ticket, 5.0);
    await_yaml_keys(&yaml, &["measured_ram_gb: 5.0"]).await;
    assert_eq!(
        state.catalog[0].measured_ram_gb,
        Some(5.0),
        "the record keeps the truth about the process"
    );
    assert_eq!(
        state.slots[0].occupant.as_ref().expect("resident").ram_need,
        Some(Gibibytes(8.0)),
        "the booking keeps the declared floor the gate admitted on"
    );
    assert_eq!(
        probe.calls(),
        0,
        "the seated pin asked the swapped-in fake nothing: the live probe \
         stays structurally unreachable (W23-02)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Surface join: the snapshot carries a CPU-resident occupant's RAM
/// booking — the ledger figure plus the rung and source labels read from the
/// catalog row's ONE ladder — each rung with its own words, and a row the
/// catalog no longer names proves nothing: all three RAM fields stay absent
/// together (the presence rule). A GPU occupant and an empty slot carry no
/// RAM keys at all (their snapshot shape is exactly the pre-RAM one).
/// Falsifiers: dropping the `ram_need` filter gives the GPU leg a label;
/// reading the figure from the card `need` fails the booked compare on every
/// rung; making the agree check always-true lets the two transition
/// pins below print the stale rung.
#[test]
fn the_snapshot_labels_a_ram_resident_booking_with_its_ladder_basis() {
    let dir = crate::testutil::temp_dir("ram-surface");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let seat_and_snapshot = |yaml_name: &str,
                             yaml: &str,
                             name: &str,
                             gguf_size: Option<f64>,
                             measured_pair: Option<f64>,
                             booked: f64,
                             port: u16|
     -> (crate::scheduler::SlotStatus, ActorState) {
        let path = dir.join(yaml_name);
        std::fs::write(&path, yaml).expect("seed");
        let mut state = fold_state(&dir);
        let mut entry = rescan_entry(&path, name);
        entry.gguf_size_gb = gguf_size;
        entry.measured_ram_gb = measured_pair;
        state.catalog = vec![entry.clone()];
        let _ = seat_ram_spawn(&mut state, &entry, &fold_ticket(port), port, booked);
        (state.snapshot().slots[0].clone(), state)
    };

    let (row, _state) = seat_and_snapshot(
        "cpu-measured.yaml",
        "engine: llama\ncpu_offload: true\nram_gb: 2\n",
        "cpu-measured",
        None,
        Some(9.4),
        9.4,
        19_060,
    );
    assert_eq!(
        (row.booked_ram_gb, row.ram_cost_basis, row.ram_source),
        (Some(9.4), Some("measured"), Some("per_process")),
        "the measured rung books above the declared floor and says so: {row:?}"
    );

    let (row, _state) = seat_and_snapshot(
        "cpu-declared.yaml",
        "engine: llama\ncpu_offload: true\nram_gb: 8\n",
        "cpu-declared",
        None,
        None,
        8.0,
        19_061,
    );
    assert_eq!(
        (row.booked_ram_gb, row.ram_cost_basis, row.ram_source),
        (Some(8.0), Some("declared"), Some("declared")),
        "the declared floor names itself and its source: {row:?}"
    );

    let (row, _state) = seat_and_snapshot(
        "cpu-estimate.yaml",
        "engine: llama\ncpu_offload: true\n",
        "cpu-estimate",
        Some(1.6),
        None,
        3.6,
        19_062,
    );
    assert_eq!(
        (row.booked_ram_gb, row.ram_cost_basis, row.ram_source),
        (Some(3.6), Some("estimate"), Some("declared")),
        "the GGUF estimate rung (x 1.0 + 2) labels itself: {row:?}"
    );

    let (row, mut state) = seat_and_snapshot(
        "cpu-vanished.yaml",
        "engine: llama\ncpu_offload: true\nram_gb: 2\n",
        "cpu-vanished",
        None,
        Some(9.4),
        9.4,
        19_063,
    );
    assert_eq!(row.booked_ram_gb, Some(9.4));
    state.catalog.clear();
    let orphan = state.snapshot().slots[0].clone();
    assert_eq!(
        (
            orphan.booked_ram_gb,
            orphan.ram_cost_basis,
            orphan.ram_source
        ),
        (None, None, None),
        "a row the catalog no longer names proves nothing: the resident \
         claims no RAM story until a row can vouch for it: {orphan:?}"
    );

    // GPU twin: the same ladder keys are inert on a card row, and the slot
    // carries no RAM key at all.
    let gpu_path = dir.join("gpu.yaml");
    std::fs::write(&gpu_path, "engine: mock\nvram_gb: 2\n").expect("seed");
    let mut state = fold_state(&dir);
    let gpu = rescan_entry(&gpu_path, "gpu");
    state.catalog = vec![gpu.clone()];
    seat_spawn(&mut state, &gpu, &fold_ticket(19_064), 19_064);
    let row = state.snapshot().slots[0].clone();
    assert_eq!(row.need_gib, Some(2.0), "the card row books the card");
    assert_eq!(
        (row.booked_ram_gb, row.ram_cost_basis, row.ram_source),
        (None, None, None),
        "a GPU occupant carries no RAM key, not even a null: {row:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Transition leg (a): a cost-only pair edit under a loaded CPU
/// resident moves the row's rung off the ledger booking, and the join must
/// not re-label the stale figure. The resident books 9.4 on a row whose
/// trusted pair decides the rung — while ladder and ledger agree the labels
/// ride and are true. The operator then deletes the pair keys and the
/// rescan lands under the resident: the move is cost-only (the ledger-boundary
/// guard neither re-books nor ambers), so the resident keeps 9.4 while the row
/// answers the 2 GiB floor. The snapshot now omits all three RAM fields:
/// absence is "no claim" — the ledger figure stays the gate's truth, the
/// row's new rung must not speak for it. Falsifier: make the agree check
/// always-true and the post-edit leg reads `declared`/`declared` beside the
/// stale 9.4 — the re-label this pin exists to refuse.
#[test]
fn the_snapshot_drops_the_ram_labels_when_a_cost_only_edit_moves_the_rung() {
    let dir = crate::testutil::temp_dir("ram-drift-pair");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("drift.yaml");
    std::fs::write(
        &yaml,
        "engine: llama\ncpu_offload: true\nram_gb: 2\nmeasured_ram_gb: 9.4\nmeasured_ram_gb_min: 9.4\n",
    )
    .expect("seed");
    let mut state = fold_state(&dir);
    let entry = rescan_entry(&yaml, "drift");
    assert_eq!(
        entry.measured_ram_gb,
        Some(9.4),
        "the pair is on the row the gate booked from"
    );
    let ticket = fold_ticket(19_070);
    state.catalog = vec![entry.clone()];
    let _ = seat_ram_spawn(&mut state, &entry, &ticket, 19_070, 9.4);

    let agreed = state.snapshot().slots[0].clone();
    assert_eq!(
        (
            agreed.booked_ram_gb,
            agreed.ram_cost_basis,
            agreed.ram_source
        ),
        (Some(9.4), Some("measured"), Some("per_process")),
        "while ladder and ledger agree the labels ride and are true: {agreed:?}"
    );

    std::fs::write(&yaml, "engine: llama\ncpu_offload: true\nram_gb: 2\n").expect("edit");
    state.catalog = vec![rescan_entry(&yaml, "drift")];
    assert_eq!(
        state.slots[0].occupant.as_ref().expect("resident").ram_need,
        Some(Gibibytes(9.4)),
        "a cost-only move re-books nothing (W1-01 guard)"
    );
    let stale = state.snapshot().slots[0].clone();
    assert_eq!(
        (stale.booked_ram_gb, stale.ram_cost_basis, stale.ram_source),
        (None, None, None),
        "the row now answers the 2 GiB floor for a resident booked 9.4: no \
         label may re-speak that figure: {stale:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Transition leg (b): the same model resident in two slots, and the
/// fold that moves the row's rung re-books only the resident whose ticket
/// produced the reading (the `(model, ticket)` guard in
/// `rebook_ram_after_measurement`). The re-booked twin agrees with the row
/// and keeps its true labels; the never-re-booked twin keeps its floor
/// booking, and beside the row's new measured figure the join omits all
/// three fields — a provenance the gate never took for that booking.
/// Falsifier: make the agree check always-true and the twin leg prints
/// `measured`/`per_process` beside a booking taken from the floor.
#[tokio::test]
async fn the_snapshot_drops_the_ram_labels_for_a_twin_the_fold_never_rebooked() {
    let dir = crate::testutil::temp_dir("ram-drift-twin");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("twin.yaml");
    std::fs::write(&yaml, "engine: llama\ncpu_offload: true\nram_gb: 2\n").expect("seed");
    let mut state = fold_state(&dir);
    state.ram_witness = Arc::new(ScriptedRamWitness::default());
    let entry = rescan_entry(&yaml, "twin");
    let ticket_a = fold_ticket(19_071);
    let mut ticket_b = ticket_a.clone();
    ticket_b.port = 19_072;
    state.catalog = vec![entry.clone()];
    let _ = seat_ram_spawn(&mut state, &entry, &ticket_a, 19_071, 2.0);
    state.slots.push(Slot {
        category: CategoryId::Work,
        index: 1,
        engine_port: 19_072,
        occupant: Some(Occupant {
            model: "twin".to_string(),
            need: Gibibytes(2.0),
            ram_need: Some(Gibibytes(2.0)),
            port: 19_072,
            ticket: ticket_b,
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: None,
            stale_config: false,
            serve_id: None,
            measure: None,
            live_measured_gb: None,
        }),
        job: None,
    });

    let before = state.snapshot().slots;
    for row in &before {
        assert_eq!(
            (row.booked_ram_gb, row.ram_cost_basis, row.ram_source),
            (Some(2.0), Some("declared"), Some("declared")),
            "both twins agree with the pair-less floor row: {row:?}"
        );
    }

    // A's working set folds onto the shared row; the re-book touches A only.
    ram_fold(&mut state, "twin", &ticket_a, 14.0);
    await_yaml_keys(&yaml, &["measured_ram_gb: 14.0"]).await;

    let rows = state.snapshot().slots;
    assert_eq!(
        (
            rows[0].booked_ram_gb,
            rows[0].ram_cost_basis,
            rows[0].ram_source
        ),
        (Some(14.0), Some("measured"), Some("per_process")),
        "the re-booked twin matches the row's new rung and keeps true labels: {:?}",
        rows[0]
    );
    assert_eq!(
        (
            rows[1].booked_ram_gb,
            rows[1].ram_cost_basis,
            rows[1].ram_source
        ),
        (None, None, None),
        "the fold never re-booked the twin: its 2.0 booking is not the row's \
         measured rung, and no label may re-label it: {:?}",
        rows[1]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The drain's pair discipline: a refused RAM persist clears
/// exactly the RAM pair and keeps the shared stamp the card ladder may still
/// read; a refused card persist does the mirrored thing to its own pair.
/// Falsifier: a drain that ignores `pair` clears both on either failure.
#[tokio::test]
async fn a_refused_ram_persist_drains_only_the_ram_pair() {
    let dir = crate::testutil::temp_dir("ram-drain");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let yaml = dir.join("both.yaml");
    std::fs::write(&yaml, "engine: mock\nvram_gb: 2\n").expect("seed");
    let mut entry = rescan_entry(&yaml, "both");
    entry.measured_gb = Some(30.0);
    entry.measured_min_gb = Some(20.0);
    entry.measured_ram_gb = Some(14.0);
    entry.measured_ram_min_gb = Some(14.0);
    entry.measured_offload = Some(true);
    let mut state = fold_state(&dir);
    state.catalog = vec![entry];

    state.fail_persist(
        "both",
        super::measured::MeasuredData {
            high: 14.0,
            low: 14.0,
            offload: Some(true),
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Ram,
        },
        Some(yaml.clone()),
    );
    let entry = &state.catalog[0];
    assert_eq!(entry.measured_ram_gb, None, "the refused RAM pair drains");
    assert_eq!(entry.measured_ram_min_gb, None);
    assert_eq!(
        (entry.measured_gb, entry.measured_min_gb),
        (Some(30.0), Some(20.0)),
        "the card pair the disk never refused stands"
    );
    assert_eq!(
        entry.measured_offload,
        Some(true),
        "the shared stamp describes the launch, and the RAM ladder books without it"
    );

    state.fail_persist(
        "both",
        super::measured::MeasuredData {
            high: 30.0,
            low: 20.0,
            offload: Some(true),
            attribution: None,
            profile: None,
            pair: super::measured::MeasuredPair::Vram,
        },
        Some(yaml.clone()),
    );
    let entry = &state.catalog[0];
    assert_eq!((entry.measured_gb, entry.measured_min_gb), (None, None));
    assert_eq!(
        entry.measured_offload, None,
        "the card drain keeps its full legacy shape"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The anchor's resource honesty: a CPU spawn's range seeds from the RAM pair
/// — never from a stray card pair that survived without a flip to void it.
/// Falsifier: seeding `range` straight from `measured_gb` for every resource
/// books host GiB against card GiB history in the serve fold.
#[test]
fn a_ram_spawn_seeds_its_anchor_from_the_ram_pair() {
    let entry = ModelEntry::from_flat(
        &BTreeMap::from([
            ("engine".to_string(), "llama".to_string()),
            ("cpu_offload".to_string(), "true".to_string()),
            ("ram_gb".to_string(), "10".to_string()),
            ("measured_ram_gb".to_string(), "12".to_string()),
            ("measured_ram_gb_min".to_string(), "9".to_string()),
            ("measured_gb".to_string(), "30".to_string()),
            ("measured_gb_min".to_string(), "25".to_string()),
            ("measured_offload".to_string(), "true".to_string()),
        ]),
        "cpu",
    )
    .expect("valid");
    assert_eq!(entry.resource(), crate::model::Resource::Ram);
    let anchor = MeasuredAnchor::at_spawn(&entry);
    assert_eq!(
        anchor.range,
        Some((12.0, 9.0)),
        "the RAM row folds RAM history"
    );
}
