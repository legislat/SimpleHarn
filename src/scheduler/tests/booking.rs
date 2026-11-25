use super::*;

/// One WORK slot holding one occupant of `(port, generation)`, for the
/// serve-id fold-door pins: fold answers land as direct
/// calls, so the never-called runtime is never reached.
fn serve_id_state(port: u16, generation: u64, serve_id: Option<&str>) -> ActorState {
    let BootWiring {
        adapters,
        runtime,
        witness,
        tx,
    } = boot_stub_wiring();
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        crate::testutil::temp_dir("serve-id-door"),
        adapters,
        runtime,
        witness,
        tx,
    );
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: port,
        occupant: Some(Occupant {
            model: format!("door-{generation}"),
            need: Gibibytes(2.0),
            ram_need: None,
            port,
            ticket: SpawnTicket {
                generation,
                kind: ResourceKind::ChildProcess,
                pid: 4242,
                port,
            },
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: None,
            stale_config: false,
            serve_id: serve_id.map(str::to_string),
            measure: None,
            live_measured_gb: None,
        }),
        job: None,
    }];
    state
}

/// Silence re-discovery warns captured for one model name.
fn silence_warns(capture: &Arc<Mutex<Vec<CapturedWarn>>>, model: &str) -> usize {
    capture
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(message, fields)| {
            message.contains("re-discovery still reports nothing")
                && fields.get("model").is_some_and(|name| name.contains(model))
        })
        .count()
}

#[test]
fn a_serve_id_answer_for_a_replaced_generation_is_dropped_at_the_door() {
    // The sibling of `a_stale_epoch_fit_answer_is_dropped_at_the_door`:
    // the occupant on this port was replaced mid-probe, so an answer naming
    // the dead generation describes an engine that no longer exists and may
    // not patch the one that does.
    let mut state = serve_id_state(19_111, 7, None);
    assert!(
        !state.on_serve_id_refreshed(19_111, 6, Some("engine-dead")),
        "an answer for a superseded spawn reports no change"
    );
    let occupant = state.slots[0]
        .occupant
        .as_ref()
        .expect("the current occupant stands");
    assert_eq!(
        occupant.serve_id, None,
        "a replaced generation's id is never laundered onto the current engine"
    );
}

#[test]
fn a_serve_id_answer_is_refused_over_an_occupant_that_already_has_one() {
    // The second guard: an id that landed at install (or
    // an earlier probe) is the engine's word for THIS spawn — a later
    // answer, however fresh, may not overwrite it, and a fold that
    // overwrites would report a publish for a change that never happened.
    let mut state = serve_id_state(19_112, 7, Some("engine-first"));
    assert!(
        !state.on_serve_id_refreshed(19_112, 7, Some("engine-second")),
        "installing over an occupant with an id is refused, not published"
    );
    let occupant = state.slots[0]
        .occupant
        .as_ref()
        .expect("the occupant keeps its seat");
    assert_eq!(
        occupant.serve_id.as_deref(),
        Some("engine-first"),
        "the already-held id is never replaced"
    );
}

#[test]
fn silence_warns_once_per_spawn_and_a_changed_outcome_arms_the_warn_again() {
    // Mirroring `the_reconcile_warns_once_and_an_unchanged_set_never_re_warns`:
    // every resolve of an id-less occupant re-runs discovery, so a client
    // hammering a permanently silent engine used to log one warn per landed
    // silence (~0.5/s/slot forever). The warn marks the CHANGE into silence.
    let capture = install_warn_capture();
    // Warm the reporting callsite and PROVE delivery before the measured
    // window (the D9 rule): a sentinel name no assertion below reads about.
    let mut warm = serve_id_state(19_113, 1, None);
    assert!(!warm.on_serve_id_refreshed(19_113, 1, None));
    assert_eq!(
        silence_warns(&capture, "door-1"),
        1,
        "the reporting callsite was warmed but never delivered"
    );

    let mut state = serve_id_state(19_114, 5, None);
    for _ in 0..3 {
        assert!(
            !state.on_serve_id_refreshed(19_114, 5, None),
            "silence installs nothing and publishes nothing"
        );
    }
    assert_eq!(
        silence_warns(&capture, "door-5"),
        1,
        "a hammered silence must name the unchanged state exactly once"
    );

    // A landed id is a NEW outcome: the silence state ends, so a return to
    // silence is news again and may name itself once more.
    assert!(state.on_serve_id_refreshed(19_114, 5, Some("engine-x")));
    assert!(!state.on_serve_id_refreshed(19_114, 5, None));
    assert_eq!(
        silence_warns(&capture, "door-5"),
        2,
        "silence that ended and returned is a changed state, not the old one"
    );

    // A new spawn on the same port is a new silence state: first warn again.
    {
        let occupant = state.slots[0]
            .occupant
            .as_mut()
            .expect("the occupant still holds its seat");
        occupant.ticket.generation = 6;
        occupant.serve_id = None;
    }
    assert!(!state.on_serve_id_refreshed(19_114, 6, None));
    assert_eq!(
        silence_warns(&capture, "door-5"),
        3,
        "the next generation is a fresh silence state and warns once on its own"
    );
}

#[test]
fn booking_need_applies_the_max_truth_including_the_greedy_floor() {
    let mut row = fit_llama_entry(std::path::Path::new("."), "booked");
    row.fit_gb = Some(31.0);
    assert_eq!(
        ActorState::booking_need(&row),
        Ok(Some(Gibibytes(31.0))),
        "the projection overshadows the smaller declared floor"
    );
    row.vram_gb = Some(40.0);
    assert_eq!(
        ActorState::booking_need(&row),
        Ok(Some(Gibibytes(40.0))),
        "a declared floor above the projection stands"
    );
    row.vram_gb = None;
    row.gguf_size_gb = None;
    row.fit_gb = Some(31.0);
    row.cost = crate::model::CostClass::Greedy;
    assert_eq!(
        ActorState::booking_need(&row),
        Ok(Some(Gibibytes(31.0))),
        "greedy never wears the Fit class but its launch floor reads the figure"
    );
    row.engine = "ninfer".to_string();
    assert_eq!(
        ActorState::booking_need(&row),
        Ok(None),
        "a projection no other engine owns floors nothing"
    );
    // A greedy basalt
    // row is the legal unknown-nominal case too - `Ok(None)` books the
    // room at admit, and the gate judges the row's launch floor (60 on
    // the live row), the same floor-only path the strata spills proved.
    row.engine = "basalt".to_string();
    assert_eq!(
        ActorState::booking_need(&row),
        Ok(None),
        "a greedy basalt row without a nominal books the room at admit"
    );
}

/// The wait-estimate ring quotes exact medians: the middle sample for an odd
/// count, and the mean of the two middle samples for an even count — the
/// estimate a refusal carries is computed over the real set, never rounded
/// to one of its members. Falsifier: make the even branch answer
/// `sorted[mid]` and the last leg goes red.
#[test]
fn release_median_is_the_middle_sample_or_the_mean_of_the_two_middles() {
    let mut ring = super::ReleaseSamples::new();
    assert_eq!(
        ring.median_s(),
        None,
        "no confirmed release: no estimate, never an invented default"
    );
    for secs in [4.0, 1.0, 9.0, 3.0, 7.0] {
        ring.push(Duration::from_secs_f64(secs));
    }
    assert_eq!(ring.median_s(), Some(4.0), "odd count: middle of 1,3,4,7,9");
    ring.push(Duration::from_secs(2));
    assert_eq!(
        ring.median_s(),
        Some(3.5),
        "even count: mean of the two middles of 1,2,3,4,7,9"
    );
}

/// The ring is bounded: a push past the cap drops the oldest sample, and the
/// median then runs over the newest `RELEASE_SAMPLE_CAP` releases only — a
/// swap that stopped long ago must not anchor today's estimate forever.
/// Falsifier: delete the `pop_front` and both legs go red.
#[test]
fn the_release_ring_keeps_only_the_newest_samples() {
    let mut ring = super::ReleaseSamples::new();
    for secs in 1..=super::RELEASE_SAMPLE_CAP as u64 + 1 {
        ring.push(Duration::from_secs(secs));
    }
    assert_eq!(
        ring.seconds.len(),
        super::RELEASE_SAMPLE_CAP,
        "the 33rd sample must evict the first, not grow the ring"
    );
    // Kept: 2..=33 — even count, middles 17 and 18.
    assert_eq!(ring.median_s(), Some(17.5));
}

/// The queue rule as documented on `ActorState::wait_hint`: a one-based
/// place behind the loads really ahead in the refused category, and `None`
/// when nothing is ahead — the harness never quotes a place it did not
/// grant. Falsifiers: drop the `+ 1` and the Work leg goes red; drop the
/// category filter and the RP leg goes red; drop the `> 0` guard and the
/// no-ahead leg goes red.
#[test]
fn a_refusal_quotes_a_place_only_behind_real_loads() {
    let dir = crate::testutil::temp_dir("waithint");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert("mock".to_string(), Arc::new(MockAdapter));
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir,
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let entry = |name: &str, category: CategoryId| -> ModelEntry {
        let (map, _) = crate::model::scan::parse_flat_yaml("engine: mock\nvram_gb: 2\n");
        let mut entry = ModelEntry::from_flat(&map, name).expect("entry");
        entry.category = category;
        entry
    };
    let refused = state.register_queued_load(0, &entry("refused", CategoryId::Work), Vec::new());
    let ahead_a = state.register_queued_load(0, &entry("ahead-a", CategoryId::Work), Vec::new());
    let ahead_b = state.register_queued_load(1, &entry("ahead-b", CategoryId::Work), Vec::new());
    state.register_queued_load(2, &entry("other", CategoryId::Rp), Vec::new());

    let hint = state.wait_hint(refused);
    assert_eq!(
        hint.queue_position,
        Some(3),
        "two Work loads are ahead: the refusal places itself third"
    );
    assert_eq!(
        hint.expected_release_s, None,
        "no confirmed release yet: the estimate stays absent"
    );

    // A category with nothing ahead got no place: null, never a quoted "1".
    let empty_job =
        state.register_queued_load(3, &entry("refused-emb", CategoryId::Emb), Vec::new());
    assert_eq!(
        state.wait_hint(empty_job).queue_position,
        None,
        "an empty ahead-set is not a queue"
    );

    // A terminal row is not "ahead": finishing the two Work loads empties the
    // set without pruning the rows (this test bypasses the terminal funnel;
    // the real funnel keeps only the newest 200 terminal rows).
    for id in [ahead_a, ahead_b] {
        state.jobs.get_mut(&id).expect("row").terminal = true;
    }
    assert_eq!(
        state.wait_hint(refused).queue_position,
        None,
        "terminal rows describe history, not a queue"
    );
}
