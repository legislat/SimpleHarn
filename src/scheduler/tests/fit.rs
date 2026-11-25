use super::*;
use crate::scheduler::FitEntry;

/// Test double for the fit probe: records every (program, argv) it is
/// asked to run and answers from two canned queues — one per `--version`
/// call, one per row call, in arrival order; an exhausted queue answers
/// `None` (the tool did not answer). It NEVER spawns: `cargo test` must
/// never exec a real `llama-fit-params.exe`, and a load reaching this
/// runtime is a test bug.
struct FitProbeRuntime {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    version: Mutex<std::collections::VecDeque<Option<String>>>,
    row: Mutex<std::collections::VecDeque<Option<String>>>,
}

impl FitProbeRuntime {
    fn new(version: Vec<Option<String>>, row: Vec<Option<String>>) -> Self {
        FitProbeRuntime {
            calls: Mutex::new(Vec::new()),
            version: Mutex::new(version.into()),
            row: Mutex::new(row.into()),
        }
    }
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for FitProbeRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the R11 fit pin must never load");
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        panic!("the R11 fit pin must never load");
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        panic!("the R11 fit pin must never load");
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the R11 fit pin must never load");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the R11 fit pin must never load");
    }
    async fn run_probe_command(
        &self,
        program: &std::path::Path,
        args: &[String],
        _timeout: Duration,
    ) -> Option<String> {
        self.calls
            .lock()
            .expect("calls lock")
            .push((program.display().to_string(), args.to_vec()));
        let queue = if args == crate::engine::fit::version_args() {
            &self.version
        } else {
            &self.row
        };
        let mut answers = queue.lock().expect("queue lock");
        answers.pop_front().unwrap_or(None)
    }
}

/// The live probe's captured answer: the same fixture the
/// parser pins, so the end-to-end expectation is the ruling's own number.
const FIT_CAPTURED: &str = "CUDA0 21484 8853 1380\nHost 994 0 296\n";

/// Config with the probe switched on (margin 0.5, no surcharge — this row
/// carries no mmproj) and the tool path under `tool_dir`.
fn fit_config(tool_dir: &std::path::Path) -> crate::config::AppConfig {
    let mut cfg = crate::config::AppConfig::parse(
        r#"{"engines": {"llama": {"fit_probe": true, "fit_margin_gb": 0.5}}}"#,
    )
    .expect("valid");
    cfg.llama_server_exe = Some(tool_dir.join("llama-server.exe"));
    cfg
}

/// The config sibling must EXIST for a pass to start (absence is a named
/// warn, not a silent skip): a zero-byte stand-in is enough, the mock
/// runtime is what answers, the file is only ever stat'ed.
fn seed_fit_tools(dir: &std::path::Path) {
    std::fs::write(dir.join("llama-server.exe"), b"MZ").expect("server stand-in");
    std::fs::write(dir.join("llama-fit-params.exe"), b"MZ").expect("tool stand-in");
}

/// State carrying one fit-capable row and the live calibration, never
/// loading anything.
fn fit_state(
    cfg: crate::config::AppConfig,
    runtime: Arc<dyn crate::engine::EngineRuntime>,
) -> ActorState {
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    ActorState::empty(
        cfg,
        crate::testutil::temp_dir("fit-state"),
        adapters,
        runtime,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    )
}

fn warns_holding(capture: &Arc<Mutex<Vec<CapturedWarn>>>, needle: &str) -> usize {
    capture
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(message, fields)| {
            message.contains(needle) || fields.values().any(|value| value.contains(needle))
        })
        .count()
}

#[test]
fn a_default_build_emits_zero_fit_probes() {
    // The inert default is total silence: no plan, no queue, nothing to
    // cancel — the live build ships before the operator's card window with
    // zero surprise probes.
    let dir = crate::testutil::temp_dir("fit-inert");
    let mut state = fit_state(
        crate::config::AppConfig::default(),
        Arc::new(FitProbeRuntime::new(Vec::new(), Vec::new()))
            as Arc<dyn crate::engine::EngineRuntime>,
    );
    state.catalog = vec![fit_llama_entry(&dir, "inert-row")];
    assert!(
        state.fit_probe_plan().is_none(),
        "probe off is no plan at all"
    );
    let mut entry = fit_llama_entry(&dir, "inert-row");
    entry.fit_gb = None;
    state.refresh_fit_on_admit(&mut entry);
    assert_eq!(
        entry.fit_gb, None,
        "the ladder is untouched by a disabled pass"
    );
}

/// A fold pin's state carrying the live Fit calibration (margin 0.5): the
/// session cross-check's tolerance exists only because this config names it.
fn fit_fold_state(dir: &std::path::Path) -> ActorState {
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    ActorState::empty(
        fit_config(dir),
        dir.to_path_buf(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    )
}

/// A row the gate books from the engine's projection: declared floor 4 GiB,
/// projection 10 GiB (small enough for the witness pins' 24 GiB card), read
/// back through the real parser with its `file` set so the fold's persist
/// leg runs on real yaml.
fn fit_booked_entry(dir: &std::path::Path, name: &str) -> ModelEntry {
    let yaml = dir.join(format!("{name}.yaml"));
    std::fs::write(
        &yaml,
        format!("engine: llama\nvram_gb: 4\nmodel: {name}.gguf\ncontext: 4096\nngl: 99\n"),
    )
    .expect("seed yaml");
    let mut entry = rescan_entry(&yaml, name);
    entry.fit_gb = Some(10.0);
    assert_eq!(
        entry.cost_source(),
        crate::model::CostSource::Fit,
        "the pin must book from Fit, or the cross-check below proves nothing"
    );
    entry
}

/// Cross-check warns for ONE model: the capture is process-global and tests
/// run in parallel, so the model name is the quarantine.
fn fit_verdict_warns(capture: &Arc<Mutex<Vec<CapturedWarn>>>, name: &str, needle: &str) -> usize {
    capture
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(message, fields)| {
            message.contains(needle)
                && fields
                    .get("model")
                    .is_some_and(|m| m.trim_matches('"') == name)
        })
        .count()
}

/// Session cross-check: a witness reading that survived every fold gate
/// and lands inside the projection's session tolerance (margin 0.5 + slack)
/// stamps the row for the session — and the stamp stays in-process: the
/// folded measured pair is the only figure this reading ever writes.
/// Falsifier: drop the tolerance compare and the out-of-tolerance pin fails;
/// persist the stamp and the yaml leg fails.
#[tokio::test]
async fn a_witness_inside_the_fit_projection_stamps_the_row_for_the_session() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-green");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = fit_fold_state(&dir);
    let entry = fit_booked_entry(&dir, "fitgreen-row");
    let yaml = entry.file.clone().expect("seeded file");
    let ticket = fold_ticket(19_301);
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 19_301);
    state.witness("fitgreen-row", &ticket, 11.0);
    assert!(
        state.snapshot().fit_validated.contains("fitgreen-row"),
        "11.0 sits 1.0 GiB from the projection, inside margin 0.5 plus slack"
    );
    assert_eq!(
        fit_verdict_warns(&capture, "fitgreen-row", "kept the Fit row amber"),
        0,
        "an in-tolerance stamp is silence"
    );
    await_yaml_keys(&yaml, &["measured_gb: 11.0"]).await;
    let persisted = std::fs::read_to_string(&yaml).expect("written yaml");
    for token in ["fit_validated", "fit_gb"] {
        assert!(
            !persisted.contains(token),
            "the verdict and the projection are in-process only; the yaml learned \
                 about {token}:\n{persisted}"
        );
    }
}

/// The refusal side, warn-once: the first out-of-tolerance reading names its
/// numbers exactly once per epoch (the operator needs them exactly once), a
/// second refusal adds no noise, and a later reading back inside tolerance
/// still earns the stamp — with a warn naming the reversal.
#[tokio::test]
async fn an_out_of_tolerance_reading_warns_once_and_can_still_earn_the_stamp() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-warn");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = fit_fold_state(&dir);
    let entry = fit_booked_entry(&dir, "fitwarn-row");
    let ticket = fold_ticket(19_302);
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 19_302);
    state.witness("fitwarn-row", &ticket, 20.0);
    assert!(
        !state.snapshot().fit_validated.contains("fitwarn-row"),
        "20.0 sits 10 GiB from the projection: the row stays amber"
    );
    assert_eq!(
        fit_verdict_warns(&capture, "fitwarn-row", "kept the Fit row amber"),
        1,
        "the first refusal names its numbers"
    );
    state.witness("fitwarn-row", &ticket, 21.0);
    assert_eq!(
        fit_verdict_warns(&capture, "fitwarn-row", "kept the Fit row amber"),
        1,
        "the same verdict repeating is silence, not a log flood"
    );
    state.witness("fitwarn-row", &ticket, 11.0);
    assert!(
        state.snapshot().fit_validated.contains("fitwarn-row"),
        "a later reading inside tolerance earns the stamp"
    );
    assert_eq!(
        fit_verdict_warns(&capture, "fitwarn-row", "fell back inside"),
        1,
        "the reversal is worth one warn"
    );
}

/// The `ForeignDominated` grade runs BEFORE the cross-check: memory that
/// moved across the window is someone else's story, so a total-diff reading
/// that the fold itself refuses must never stamp a projection green — the
/// green would vouch for a number the fold refused to mint.
#[tokio::test]
async fn a_contaminated_window_never_stamps_a_fit_row_green() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-contam");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = fit_fold_state(&dir);
    let entry = fit_booked_entry(&dir, "fitcontam-row");
    let ticket = fold_ticket(19_303);
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 19_303);
    // own: None keeps this a total-diff (the only reading the grade guards).
    // At admit 52 GiB of the 72 GiB card was unaccounted; now 58 is: 6 GiB
    // moved while the engine grew 2 — foreign-dominated, refused outright.
    state.on_measured(
        "fitcontam-row",
        &ticket,
        super::Reading {
            total_diff: 2.0,
            own: None,
            total_gib: 72.0,
            used_gib: 60.0,
        },
    );
    assert!(
        !state.snapshot().fit_validated.contains("fitcontam-row"),
        "a contaminated window cannot mint a green"
    );
    assert_eq!(
        fit_verdict_warns(&capture, "fitcontam-row", "kept the Fit row amber"),
        0,
        "the guard refuses BEFORE any verdict is attempted"
    );
}

/// An uncalibrated margin is NO defined tolerance: the row stays amber with
/// total silence — a display cross-check may not nag about a tolerance that
/// was never configured.
#[tokio::test]
async fn an_uncalibrated_margin_never_validates_and_never_says_so() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-quiet");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = fold_state(&dir);
    let entry = fit_booked_entry(&dir, "fitquiet-row");
    let ticket = fold_ticket(19_304);
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 19_304);
    state.witness("fitquiet-row", &ticket, 11.0);
    assert!(
        !state.snapshot().fit_validated.contains("fitquiet-row"),
        "no calibration, no tolerance, no green"
    );
    for needle in ["kept the Fit row amber", "fell back inside"] {
        assert_eq!(
            fit_verdict_warns(&capture, "fitquiet-row", needle),
            0,
            "silence, not a nag about an absent config key: {needle}"
        );
    }
}

/// Pin: the session tolerance is exactly `fit_margin_gb` +
/// [`FIT_VALIDATE_SLACK_GIB`], that default slack is exactly 2.0 GiB, and
/// an absent or non-finite margin leaves NO defined tolerance (the row
/// never validates, silently). The calibration figures live once, in the
/// calibration pin in `src/engine/fit.rs`.
#[test]
fn the_fit_validation_tolerance_is_the_margin_plus_the_pinned_default_slack() {
    assert_eq!(
        crate::scheduler::FIT_VALIDATE_SLACK_GIB,
        2.0,
        "the calibrated slack default"
    );
    assert_eq!(
        ActorState::fit_validation_tolerance(Some(3.0)),
        Some(5.0),
        "tolerance = fit_margin_gb + slack, at the live margin 3"
    );
    assert_eq!(
        ActorState::fit_validation_tolerance(Some(0.5)),
        Some(2.5),
        "the same composition the fold pins exercise"
    );
    assert_eq!(
        ActorState::fit_validation_tolerance(None),
        None,
        "no margin, no tolerance: never-validate, silent"
    );
    assert!(
        ActorState::fit_validation_tolerance(Some(f64::NAN)).is_none()
            && ActorState::fit_validation_tolerance(Some(f64::INFINITY)).is_none(),
        "a nonsense margin is no tolerance either (the parse_gib doctrine)"
    );
}

/// The stamp's two retirements: a catalog rescan bumps the fit epoch (and
/// clears the map), and a calculator swap clears it outright — a green the
/// page showed described the superseded catalog or the old tool.
#[tokio::test]
async fn a_rescan_or_a_calculator_swap_retires_the_session_stamp() {
    let dir = crate::testutil::temp_dir("fit-retire");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut state = fit_fold_state(&dir);
    let entry = fit_booked_entry(&dir, "fitretire-row");
    let ticket = fold_ticket(19_305);
    state.catalog = vec![entry.clone()];
    seat_spawn(&mut state, &entry, &ticket, 19_305);
    state.on_fit_version(
        state.fit_gen(),
        &dir.join("llama-fit-params.exe"),
        Some("b11669".to_string()),
    );
    state.witness("fitretire-row", &ticket, 11.0);
    assert!(state.snapshot().fit_validated.contains("fitretire-row"));
    state.spawn_fit_warm();
    assert!(
        state.snapshot().fit_validated.is_empty(),
        "the rescan's epoch retires every session stamp"
    );
    state.witness("fitretire-row", &ticket, 11.0);
    assert!(
        state.snapshot().fit_validated.contains("fitretire-row"),
        "the new epoch can earn a fresh stamp"
    );
    state.on_fit_version(
        state.fit_gen(),
        &dir.join("llama-fit-params.exe"),
        Some("b20000".to_string()),
    );
    assert!(
        state.snapshot().fit_validated.is_empty(),
        "a replaced calculator voids verdicts earned under the old one"
    );
}

#[tokio::test]
async fn the_warm_pass_books_the_projection_and_never_touches_the_yaml() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-warm");
    seed_fit_tools(&dir);
    std::fs::write(dir.join("turbo.gguf"), b"weights").expect("model stand-in");
    let yaml = "name: fit-turbo\nengine: llama\ncategory: work\nvram_gb: 24\n\
                    model: turbo.gguf\ncontext: 4096\nngl: 99\n";
    std::fs::write(dir.join("fit-turbo.yaml"), yaml).expect("seed llama yaml");
    let probe = Arc::new(FitProbeRuntime::new(
        vec![Some("b11669".to_string())],
        vec![Some(FIT_CAPTURED.to_string())],
    ));
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "llama".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let handle = Scheduler::start(
        fit_config(&dir),
        dir.clone(),
        adapters,
        probe.clone() as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
    );
    let expected = 31_717.0 / 1024.0 + 0.5;
    let deadline = Instant::now() + Duration::from_secs(20);
    let catalog = loop {
        let catalog = handle.catalog().await.expect("actor alive");
        if catalog
            .models
            .iter()
            .any(|m| m.name == "fit-turbo" && m.fit_gb.is_some())
        {
            break catalog;
        }
        assert!(
            Instant::now() < deadline,
            "the warm pass never landed a figure (warns: {:?})",
            capture.lock().expect("capture lock")
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let row = catalog
        .models
        .iter()
        .find(|m| m.name == "fit-turbo")
        .expect("the row is there");
    assert_eq!(
        row.fit_gb,
        Some(expected),
        "the captured device sum, Host excluded, + one active device's margin"
    );
    let calls = probe.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 2, "one version leg, one row leg: {calls:?}");
    let program = std::path::Path::new(&calls[0].0);
    assert_eq!(
        program.file_stem().expect("stem"),
        "llama-fit-params",
        "the probe is the config sibling, never the server itself"
    );
    assert_eq!(
        calls[0].1,
        crate::engine::fit::version_args(),
        "the epoch rides first"
    );
    assert_eq!(
        calls[1].1,
        vec![
            "-m",
            "turbo.gguf",
            "-c",
            "4096",
            "-ngl",
            "99",
            "--fit-print",
            "on",
        ],
        "only the memory-relevant flags, --fit-print last, nothing the server forces"
    );
    let row_args = calls[1].1.join(" ");
    for forbidden in ["--host", "--port", "--jinja", "--mmproj"] {
        assert!(
            !row_args.contains(forbidden),
            "fit-params rejects or ignores {forbidden}: {row_args}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("fit-turbo.yaml")).expect("yaml readable"),
        yaml,
        "the figure never reaches the operator's bytes: warm booking does not write"
    );
}

#[test]
fn a_replaced_calculator_voids_every_projection() {
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-epoch");
    let mut state = fit_state(
        fit_config(&dir),
        Arc::new(FitProbeRuntime::new(Vec::new(), Vec::new()))
            as Arc<dyn crate::engine::EngineRuntime>,
    );
    let mut row = fit_llama_entry(&dir, "epoch-row");
    row.fit_gb = Some(31.0);
    let tool = dir.join("llama-fit-params.exe");
    state.catalog = vec![row];
    state
        .fit_versions
        .insert(tool.clone(), "b11669".to_string());
    state.fit_cache.insert(
        (tool.clone(), 7),
        FitEntry {
            gib: 31.0,
            fingerprint: None,
        },
    );
    let gen = state.fit_gen();
    state.on_fit_version(gen, &tool, Some("b12000".to_string()));
    assert_eq!(
        state.fit_cache.len(),
        0,
        "a figure belongs to the tool that minted it"
    );
    assert_eq!(
        state.catalog[0].fit_gb, None,
        "a stripped row falls back to the ladder"
    );
    assert_eq!(
        state.fit_versions.get(&tool).map(String::as_str),
        Some("b12000")
    );
    assert!(
        warns_holding(&capture, "the fit calculator changed under us") >= 1,
        "the void must be named, not silent"
    );
}

#[test]
fn an_unanswered_version_check_is_silence_about_this_pass_only() {
    // A momentarily absent tool is no evidence the tool CHANGED: the
    // established epoch and its figures stand, the pass simply goes quiet.
    let capture = install_warn_capture();
    let dir = crate::testutil::temp_dir("fit-silent");
    let mut state = fit_state(
        fit_config(&dir),
        Arc::new(FitProbeRuntime::new(Vec::new(), Vec::new()))
            as Arc<dyn crate::engine::EngineRuntime>,
    );
    let mut row = fit_llama_entry(&dir, "silent-row");
    row.fit_gb = Some(3.0);
    let tool = dir.join("llama-fit-params.exe");
    state.catalog = vec![row];
    state
        .fit_versions
        .insert(tool.clone(), "b11669".to_string());
    let gen = state.fit_gen();
    state.on_fit_version(gen, &tool, None);
    assert_eq!(
        state.fit_versions.get(&tool).map(String::as_str),
        Some("b11669")
    );
    assert_eq!(state.catalog[0].fit_gb, Some(3.0));
    assert!(warns_holding(&capture, "did not answer --version") >= 1);
}

#[test]
fn a_stale_epoch_fit_answer_is_dropped_at_the_door() {
    // A rescan bumps the epoch mid-probe: the answer describes a catalog
    // that no longer exists and may not patch anything.
    let dir = crate::testutil::temp_dir("fit-stale");
    let mut state = fit_state(
        fit_config(&dir),
        Arc::new(FitProbeRuntime::new(Vec::new(), Vec::new()))
            as Arc<dyn crate::engine::EngineRuntime>,
    );
    let entry = fit_llama_entry(&dir, "stale-row");
    let key = ActorState::fit_row_key(&entry).expect("probeable");
    state.catalog = vec![entry];
    let stale = state.fit_gen().wrapping_add(1);
    state.on_fit_row_done(
        stale,
        &dir.join("llama-fit-params.exe"),
        "stale-row",
        key,
        Ok((31.0, None)),
    );
    assert_eq!(
        state.catalog[0].fit_gb, None,
        "the superseded epoch's figure is void"
    );
    assert_eq!(state.fit_cache.len(), 0, "and it never seeds the cache");
}

#[tokio::test]
async fn an_admit_hit_books_the_projection_a_proven_change_voids_it() {
    let dir = crate::testutil::temp_dir("fit-admit");
    seed_fit_tools(&dir);
    let probe = Arc::new(FitProbeRuntime::new(Vec::new(), Vec::new()));
    let mut state = fit_state(
        fit_config(&dir),
        Arc::clone(&probe) as Arc<dyn crate::engine::EngineRuntime>,
    );
    let entry = fit_llama_entry(&dir, "admit-row");
    let key = ActorState::fit_row_key(&entry).expect("probeable");
    state.catalog = vec![entry.clone()];

    // Unstat'able model file (both sides absent): no evidence, figure stands.
    state.fit_cache.insert(
        (dir.join("llama-fit-params.exe"), key),
        FitEntry {
            gib: 31.0,
            fingerprint: None,
        },
    );
    let mut probe_row = entry.clone();
    state.refresh_fit_on_admit(&mut probe_row);
    assert_eq!(probe_row.fit_gb, Some(31.0), "same argv, same tool, KEEP");
    assert_eq!(
        state.catalog[0].fit_gb,
        Some(31.0),
        "the page reads the figure the admit booked"
    );

    // The ruled lock is "version checked at boot AND launch": a booked
    // hit is no exemption — the admit still asks the calculator its
    // version, fire-and-forget (this admit never parks behind it).
    let mut version_checked = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let first_call = probe.calls.lock().expect("calls lock").first().cloned();
        if let Some((_, args)) = first_call {
            assert_eq!(
                args,
                crate::engine::fit::version_args(),
                "the FIRST thing a hit admit asks is the tool identity"
            );
            version_checked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        version_checked,
        "a cache-hit admit never checked the version"
    );

    // A PROVEN model-file change (both fingerprints known, differing) is the
    // only thing that voids the hit — and this admit books the ladder now
    // while the refresh re-projects. The admit never stats; the proven
    // change reaches it the way production delivers it — the rescan re-read
    // the file and left the fresh reading on the scan-carried table.
    std::fs::write(dir.join("turbo.gguf"), b"weights").expect("model file");
    let fresh = crate::model::fingerprint(&dir.join("turbo.gguf")).expect("stat");
    let stale_stamp = crate::model::FileFingerprint {
        mtime: fresh.mtime + std::time::Duration::from_secs(60),
        size: fresh.size + 1,
    };
    state.fit_cache.insert(
        (dir.join("llama-fit-params.exe"), key),
        FitEntry {
            gib: 31.0,
            fingerprint: Some(stale_stamp),
        },
    );
    let mut changed_row = entry.clone();
    // The carrier rides the row's yaml identity; the admit reads the table,
    // it never stats.
    changed_row.file = Some(dir.join("admit-row.yaml"));
    changed_row.fit_gb = Some(31.0);
    crate::model::note_scan_fingerprint(
        changed_row.file.as_deref().expect("carrier yaml"),
        changed_row.fit_model_key().expect("a local model"),
        fresh,
    );
    state.refresh_fit_on_admit(&mut changed_row);
    assert_eq!(
        changed_row.fit_gb, None,
        "a rewritten model file is a proven change: the current admit books the ladder"
    );
}
