use super::*;

/// The hard invariant's STRUCTURAL tier (established `include_str!` seam, same
/// shape as the api's `swap_for_port` tripwire): the `snapshot` body — the one
/// status-path reader of the marker — may never reach for the filesystem or the
/// clock. The ban-list convicts SYNONYMS, not just the lazy spelling: bare
/// `fs::` catches `std::fs::` and `tokio::fs::` alike, `metadata(` catches
/// `std::fs::metadata`, `Path::metadata()` and the `MetadataExt` forms, and
/// `SystemTime::now` is banned because the stamp is stored at install
/// (`launched_at_wall`) for exactly that reason.
///
/// The two-layer contract, stated honestly: what the NET cannot see is a stat
/// whose result is DISCARDED — inert by definition, it cannot move a verdict —
/// and a stat parked behind a helper name outside the scan. What it convicts is
/// every spelling of a reaching fs/clock call inside the read path's own bodies.
/// The BEHAVIOURAL backstop (the hidden-file probe below, and the api-side
/// `/har/models` leg) catches anything whose result reaches the published
/// verdict regardless of spelling: a stat feeding the verdict is convicted by
/// BOTH tiers; a stat whose result is dropped is harmless to the verdict by
/// construction, and the net still keeps the synonym out of the hot path.
/// The tail sentinel asserts the slice actually covers the body's end (a
/// future early column-4 `}` cannot silently shrink what is scanned).
///
/// Falsifier: move the verdict computation into `snapshot` (any spelling) and
/// the net plus both probes fire; inject an inert `Path::new(f).metadata()` and
/// the net fires.
#[test]
fn the_snapshot_reads_the_cached_verdict_and_never_stats() {
    let source = include_str!("../status.rs");
    let start = source
        .find("    pub(super) fn snapshot(&self) -> SchedulerStatus {")
        .expect("snapshot is still in this file");
    let body = &source[start
        ..start
            + source[start..]
                .find("\n    }\n")
                .expect("the function closes")];
    assert!(
        body.contains("loaded_at:"),
        "the scan slice must reach the body's tail (early-close fragility)"
    );
    for banned in [
        "fs::",
        "fingerprint(",
        "read_to_string(",
        "metadata(",
        "modified(",
        "try_exists",
        "File::",
        "FileTimes",
        "OpenOptions",
        "SystemTime::now",
        "read_dir",
    ] {
        assert!(
            !body.contains(banned),
            "the snapshot may only read cached state; found `{banned}` in its body"
        );
    }
    let (state, yaml, _) = resident_on_a_file();
    let before = state.snapshot();
    assert_eq!(before.slots[0].stale_config, Some(false));
    std::fs::remove_file(&yaml).expect("hide the file");
    let after = state.snapshot();
    assert_eq!(
        before.slots[0].stale_config, after.slots[0].stale_config,
        "hiding the file from under the status path changes nothing: the verdict \
             is a cache, not a stat (a statting snapshot would flip to Some(true))"
    );
    assert_eq!(before.slots[0].launched_at, after.slots[0].launched_at);
}

/// The production half of a source file: everything before its test module (if
/// no `mod tests` boundary exists, the whole file counts as production — the
/// safe direction).
fn production_half(source: &str) -> &str {
    let cut = [
        "#[cfg(test)]\nmod tests {",
        "#[cfg(test)]\npub(crate) mod tests {",
    ]
    .iter()
    .filter_map(|marker| source.find(marker))
    .min()
    .unwrap_or(source.len());
    &source[..cut]
}

/// Structural net: the reload turn's drift rounds in
/// `catalog.rs` judge only pre-gathered evidence. `apply_catalog` (the single
/// writer's turn) and both rounds it runs contain no filesystem token at all —
/// every stat that moves a `stale_config` verdict or a queued-row retirement
/// is gathered off the actor by `gather_drift_stats`, on the reload task's
/// blocking legs, and travels in as [`DriftStats`].
///
/// Falsifier: restore the inline stat in `refresh_stale_markers` or
/// `invalidate_queued_loads` (any synonym spelling) and this is the test that
/// goes red first; the behavioural twins (supersede/amber after
/// `handle.reload()`) stay green either way because they pin semantics, not
/// threading — the same two-tier shape as the snapshot net above.
#[test]
fn the_drift_rounds_judge_only_pre_gathered_evidence() {
    let source = include_str!("../catalog.rs");
    for (head, tail) in [
        ("fn apply_catalog(", "spawn_fit_warm"),
        ("fn invalidate_queued_loads(", "finish_queued_load"),
        ("fn refresh_stale_markers(", "occupant.stale_config = "),
    ] {
        let start = source
            .find(head)
            .unwrap_or_else(|| panic!("{head} is still in catalog.rs"));
        let close = source[start..]
            .find("\n    }\n")
            .unwrap_or_else(|| panic!("{head} closes at column 4"));
        let body = &source[start..start + close];
        assert!(
            body.contains(tail),
            "the scan slice must reach {head}'s tail (early-close fragility)"
        );
        for banned in [
            "fs::",
            "fingerprint(",
            "read_to_string(",
            "metadata(",
            "try_exists",
            "read_dir",
            ".exists()",
            "is_file(",
            "is_dir(",
            "File::open",
        ] {
            assert!(
                !body.contains(banned),
                "`{head}` may only judge the gathered `DriftStats`; found \
                     `{banned}` — the actor turn must never reach the filesystem \
                     (FINDINGS-07 [8])"
            );
        }
    }
}

/// P-4: the re-baseline table documented at
/// [`ActorState::on_model_file_written`] claims a CLOSED set of legitimate model-
/// yaml mutators — the measured persist (`scheduler/measured.rs`, its note riding
/// the success path) and `har_config_model`'s flag edit (api, its note sent before the
/// handler's reload). "True today" was grep-only; this pin makes the claim
/// structural, the repo's established `include_str!` style: every scanned source
/// keeps exactly its allowed count of writeback call sites in its production
/// half, and each mutator file must also carry its note. The scan list names
/// every scheduler production file, so a new writer anywhere in the scheduler
/// tree trips this pin, not only the files that already carry mutators. A
/// third caller must register the write in the re-baseline table (send
/// `ModelFileWritten` / `note_model_file_written` with the write) or the amber
/// blames the harness for its own write — the exact lie interlock 5 forbids.
/// Falsifier: inject `let _ = crate::model::writeback::set_flags_on_file(
/// std::path::Path::new("x"), &[]);` into any scanned production body and this
/// is the sole failing test; add the call to api WITHOUT the note and the note
/// leg fires too.
#[test]
fn the_writeback_mutator_set_stays_closed_to_the_rebaseline_table() {
    for (file, source, allowed) in [
        ("scheduler/mod.rs", include_str!("../mod.rs"), 0usize),
        ("scheduler/admission.rs", include_str!("../admission.rs"), 0),
        ("scheduler/catalog.rs", include_str!("../catalog.rs"), 0),
        ("scheduler/commands.rs", include_str!("../commands.rs"), 0),
        ("scheduler/events.rs", include_str!("../events.rs"), 0),
        ("scheduler/fit.rs", include_str!("../fit.rs"), 0),
        ("scheduler/handle.rs", include_str!("../handle.rs"), 0),
        ("scheduler/jobs.rs", include_str!("../jobs.rs"), 0),
        ("scheduler/measured.rs", include_str!("../measured.rs"), 1),
        ("scheduler/messages.rs", include_str!("../messages.rs"), 0),
        ("scheduler/occupant.rs", include_str!("../occupant.rs"), 0),
        ("scheduler/spawn.rs", include_str!("../spawn.rs"), 0),
        ("scheduler/status.rs", include_str!("../status.rs"), 0),
        ("scheduler/vocab.rs", include_str!("../vocab.rs"), 0),
        ("api/mod.rs", include_str!("../../api/mod.rs"), 0),
        ("api/guards.rs", include_str!("../../api/guards.rs"), 0),
        (
            "api/status_render.rs",
            include_str!("../../api/status_render.rs"),
            0,
        ),
        (
            "api/config_persist.rs",
            include_str!("../../api/config_persist.rs"),
            1,
        ),
        ("api/proxy.rs", include_str!("../../api/proxy.rs"), 0),
        ("main.rs", include_str!("../../main.rs"), 0),
        ("catalog.rs", include_str!("../catalog.rs"), 0),
        ("tray.rs", include_str!("../../tray.rs"), 0),
        ("model/mod.rs", include_str!("../../model/mod.rs"), 0),
        ("engine/mod.rs", include_str!("../../engine/mod.rs"), 0),
    ] {
        let production = production_half(source);
        let calls = production.matches("set_flags_on_file(").count()
            + production.matches("set_flag_on_file(").count();
        assert_eq!(
            calls, allowed,
            "{file} carries {calls} writeback mutator call sites; the closed table \
                 allows {allowed} (the measured persist in scheduler/measured.rs, \
                 har_config_model's flag edit in api). A new writer must register in \
                 the re-baseline table — send the ModelFileWritten note with the write \
                 (see ActorState::on_model_file_written) — or the stale marker lies \
                 about the harness's own write: FINDINGS-BATCH-2026-10-28P [P-4]."
        );
        if allowed == 1 {
            let notes = if file.starts_with("api") {
                production.contains("note_model_file_written(")
            } else {
                production.contains("ActorMsg::ModelFileWritten")
            };
            assert!(
                notes,
                "{file} mutates a model yaml but never sends the re-baseline note: \
                     the write would self-amber its resident"
            );
        }
    }
}

#[test]
fn a_stale_reap_target_does_not_abort_the_rest() {
    // The stale-ticket fix: aborting the reap on a stale victim ticket stranded the job's own live
    // engine — the very orphan the multi-target reap exists to prevent.
    use crate::engine::SpawnError;
    assert!(
        ActorState::reap_should_continue(&SpawnError::StaleTicket {
            port: 18_109,
            expected: 3,
            found: 4,
        }),
        "a vanished victim leaves nothing to reap; the next target must still be tried"
    );
    assert!(
        !ActorState::reap_should_continue(&SpawnError::KillFailed {
            holder: "model".to_string(),
            reason: "TerminateProcess failed".to_string(),
        }),
        "a real kill failure must still fail the job"
    );
}

// --- The one bounded late-release watch ---------------------

/// Whether the late-release watch has spent its bound. The boundary is the whole
/// contract (option B: one `release_timeout_s`, never a second unbounded task), so
/// it is pinned here rather than inferred from a wall-clock wait.
#[test]
fn the_late_release_watch_bounds_at_exactly_one_window() {
    let window = Duration::from_secs(2);
    assert!(
        !ActorState::late_release_expired(Duration::ZERO, window),
        "the watch starts its one window empty"
    );
    assert!(
        !ActorState::late_release_expired(window - RELEASE_POLL, window),
        "the watch may still poll inside its window, on the reap's own cadence"
    );
    assert!(
        ActorState::late_release_expired(window, window),
        "the watch ends the moment the window is spent: the bound is one window, not two"
    );
    assert!(ActorState::late_release_expired(
        window + Duration::from_secs(5),
        window
    ));
}

/// The watch is scoped to the uninstalled spawn. An occupant carrying this
/// very `(port, generation)` is what `/har/status` reports as serving, so the watch
/// stays out; any other occupant (or none) leaves it armed.
#[test]
fn an_installed_occupant_of_the_same_ticket_blinds_the_late_watch() {
    let orphan = SpawnTicket {
        generation: 41,
        kind: ResourceKind::ChildProcess,
        pid: 4_241,
        port: 19_441,
    };
    let mut installed_same = orphan.clone();
    let installed_other = SpawnTicket {
        generation: 42,
        kind: ResourceKind::ChildProcess,
        pid: 4_242,
        port: 19_441,
    };
    let mut other_port = orphan.clone();
    other_port.port = 19_442;

    assert!(
        ActorState::late_watch_names_an_orphan(None, &orphan),
        "no occupant at all is the R2-02-02 shape: nothing but the watch can name it"
    );
    assert!(
        !ActorState::late_watch_names_an_orphan(Some(&installed_same), &orphan),
        "the slot reports THIS ticket as serving; the watch may not call it gone"
    );
    installed_same.pid = 9_999;
    assert!(
        !ActorState::late_watch_names_an_orphan(Some(&installed_same), &orphan),
        "identity is the runtime table's (port, generation), not the pid"
    );
    assert!(
        ActorState::late_watch_names_an_orphan(Some(&installed_other), &orphan),
        "a newer generation in that slot is a different engine: the orphan still needs \
             its watch"
    );
    assert!(
        ActorState::late_watch_names_an_orphan(Some(&other_port), &orphan),
        "another port's occupant says nothing about this ticket"
    );
}

/// What the fake answers once the late window is open.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LateAnswer {
    /// Death proven late: the runtime answers confirmed for this generation.
    Confirmed,
    /// Still no proof: the watch keeps polling until its own deadline.
    #[default]
    StillHeld,
    /// A newer spawn owns that port now: the ticket is stale.
    Superseded,
    /// The evidence check never returns — the hard outer timeout is the
    /// only thing that can end the watch.
    Wedged,
}

/// Which terminalizing arm a pin drives: the failure funnel's `Failed` or a
/// cancel's `CancelRequested` — the two terminalizing arms.
#[derive(Clone, Copy)]
enum Arm {
    Failed,
    CancelRequested,
}

/// Test double for the bounded late-release watch. `kill` is recorded and answers
/// `Ok` (the terminate was issued); `release_confirmed` answers `false` while the
/// job's own reap window runs — the state that produces the unconfirmed terminal —
/// and then exactly what the test scripts for the late window. Every answer is
/// recorded with the ticket it named and whether the late window was already open,
/// so a pin can tell the reap's polls from the watch's own and name every
/// generation the watch went looking for. It never kills on its own, so a `kill`
/// count above the reap's single kill is a watch that re-killed.
#[derive(Default)]
struct LateWatchRuntime {
    state: Mutex<LateWatch>,
}

#[derive(Default)]
struct LateWatch {
    late_open: bool,
    late: LateAnswer,
    kills: usize,
    /// `(port, generation, asked after the late window opened, verdict)`.
    polls: Vec<(u16, u64, bool, &'static str)>,
}

impl LateWatchRuntime {
    fn new() -> Self {
        LateWatchRuntime::default()
    }

    /// Opens the late window. From here every `release_confirmed` belongs to the
    /// watch, because the reap task was dropped the moment its deadline expired —
    /// its event is what terminalized the job.
    fn open_late_window(&self, answer: LateAnswer) {
        let mut state = self.state.lock().expect("late watch lock");
        state.late_open = true;
        state.late = answer;
    }

    /// The watch's own polls, in order.
    fn late_polls(&self) -> Vec<(u16, u64, &'static str)> {
        self.state
            .lock()
            .expect("late watch lock")
            .polls
            .iter()
            .filter(|(_, _, late, _)| *late)
            .map(|(port, generation, _, verdict)| (*port, *generation, *verdict))
            .collect()
    }

    /// Every `release_confirmed` this runtime was ever asked about, late window or
    /// not: proves the reap window polled first and that no other generation was.
    fn poll_count(&self) -> usize {
        self.state.lock().expect("late watch lock").polls.len()
    }

    fn generations_polled(&self) -> Vec<u64> {
        self.state
            .lock()
            .expect("late watch lock")
            .polls
            .iter()
            .map(|(_, generation, _, _)| *generation)
            .collect()
    }

    fn kill_count(&self) -> usize {
        self.state.lock().expect("late watch lock").kills
    }
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for LateWatchRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the R2-02-02 late-release pin never spawns");
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        self.state.lock().expect("late watch lock").kills += 1;
        Ok(())
    }
    async fn release_confirmed(
        &self,
        ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        let (answer, outcome) = {
            let mut state = self.state.lock().expect("late watch lock");
            let late_open = state.late_open;
            let answer = if late_open {
                state.late
            } else {
                LateAnswer::StillHeld
            };
            let (verdict, outcome) = match answer {
                LateAnswer::Confirmed => ("confirmed", Ok(true)),
                LateAnswer::StillHeld => ("pending", Ok(false)),
                LateAnswer::Superseded => (
                    "superseded",
                    Err(crate::engine::SpawnError::StaleTicket {
                        port: ticket.port,
                        expected: ticket.generation,
                        // The successor the table now carries for this port.
                        found: ticket.generation + 1,
                    }),
                ),
                LateAnswer::Wedged => ("wedged", Ok(false)),
            };
            state
                .polls
                .push((ticket.port, ticket.generation, late_open, verdict));
            (answer, outcome)
        };
        if answer == LateAnswer::Wedged {
            // The call is recorded and then never answers. The lock went
            // back first, so the test can still read what this fake recorded while the
            // watch is stuck inside the runtime call.
            std::future::pending::<()>().await;
        }
        outcome
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the R2-02-02 late-release pin never probes");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the R2-02-02 late-release pin never discovers");
    }
}

/// An actor whose release deadline is `window` seconds, driven by the late-watch
/// fake, with the reap's event channel held by the caller.
fn late_watch_state(
    runtime: Arc<LateWatchRuntime>,
    window: u64,
) -> (ActorState, mpsc::UnboundedReceiver<super::ActorMsg>) {
    let (tx, rx) = mpsc::unbounded_channel::<ActorMsg>();
    (
        ActorState::empty(
            crate::config::AppConfig {
                release_timeout_s: window,
                ..crate::config::AppConfig::default()
            },
            crate::testutil::temp_dir("late-release"),
            BTreeMap::new(),
            runtime as Arc<dyn crate::engine::EngineRuntime>,
            Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
            tx,
        ),
        rx,
    )
}

/// The late-release watch shape: a `Ready` load row that owns a spawn while its slot holds NO
/// occupant — the engine started, readiness never landed, so nothing was installed
/// and the slot reads empty while the ticket is still live.
fn uninstalled_spawn_row(state: &mut ActorState, name: &str, ticket: &SpawnTicket) -> JobId {
    seat_spawn_row(state, name, SwapState::Ready, ticket, None)
}

/// The slot's reported occupant ticket, as the watch reads it.
fn seated_occupant_ticket(state: &ActorState) -> Option<SpawnTicket> {
    state.slots[0]
        .occupant
        .as_ref()
        .map(|occupant| occupant.ticket.clone())
}

/// Runs one job to terminalization on a release nobody proved, exactly as the actor
/// does: the real `spawn_kill_confirm` withholds its evidence until the real
/// deadline, and the real `Released(Err)` event is applied on the actor's turn. The
/// late window opens between the two, so every poll recorded after the flip belongs
/// to the watch and to nothing else.
async fn drive_to_unproven_terminal(
    state: &mut ActorState,
    runtime: &LateWatchRuntime,
    job_id: JobId,
    arm: Arm,
    answer: LateAnswer,
    rx: &mut mpsc::UnboundedReceiver<super::ActorMsg>,
) {
    use super::SchedulerError;
    match arm {
        Arm::Failed => state.fail_job(
            job_id,
            SchedulerError::EngineFailed("engine never readied".to_string()),
            "engine not ready",
        ),
        Arm::CancelRequested => state.cancel_job(job_id, "unloaded by request"),
    }
    let message = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the reap answers inside the test deadline")
        .expect("the actor channel is open");
    let ActorMsg::Event { job, event } = message else {
        panic!("expected the reap's release event");
    };
    assert_eq!(job, job_id, "the reap answers its own job");
    runtime.open_late_window(answer);
    state.apply_event(job_id, event);
}

/// Waits for the detached watch to record its polls — the only honest seam to a
/// task the actor does not own (that is the point of the bounded late-release watch).
async fn await_late_polls(
    runtime: &LateWatchRuntime,
    want: impl Fn(usize) -> bool,
) -> Vec<(u16, u64, &'static str)> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let polls = runtime.late_polls();
        if want(polls.len()) {
            return polls;
        }
        assert!(
            Instant::now() < deadline,
            "the late watch never polled as expected; last {polls:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Late-release warns for ONE model: the capture is process-global and lib tests
/// run in parallel, so the model name is the quarantine.
fn late_release_warns(capture: &Arc<Mutex<Vec<CapturedWarn>>>, model: &str, needle: &str) -> usize {
    capture
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(message, fields)| {
            message.contains(needle)
                && fields
                    .get("model")
                    .is_some_and(|m| m.trim_matches('"') == model)
        })
        .count()
}

/// The D9 warm for every wording the pins below read, on sentinel names nothing
/// else looks at. A zero-length release window expires both the reap's deadline and
/// the watch's own bound on the first poll, so proving delivery costs no wall clock.
#[tokio::test]
async fn every_late_release_wording_is_warmed_and_delivered() {
    let capture = install_warn_capture();
    for (name, answer, needle) in [
        (
            "latewarm-confirmed",
            LateAnswer::Confirmed,
            "no runtime entry for this generation remains",
        ),
        (
            "latewarm-superseded",
            LateAnswer::Superseded,
            "ticket was superseded",
        ),
        (
            "latewarm-stillheld",
            LateAnswer::StillHeld,
            "the next load on this port or the boot reclaim must reap it",
        ),
    ] {
        let runtime = Arc::new(LateWatchRuntime::new());
        let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 0);
        let ticket = SpawnTicket {
            generation: 3,
            kind: ResourceKind::ChildProcess,
            pid: 9_003,
            port: 19_400,
        };
        let job_id = uninstalled_spawn_row(&mut state, name, &ticket);
        drive_to_unproven_terminal(&mut state, &runtime, job_id, Arm::Failed, answer, &mut rx)
            .await;
        await_late_polls(&runtime, |count| count >= 1).await;
        assert_eq!(
            late_release_warns(&capture, name, needle),
            1,
            "the {needle:?} callsite was warmed but never delivered"
        );
    }
}

/// The `Failed` arm: a load that failed readiness and whose reap never proved the
/// release terminalizes as it always did, and its ticket gets exactly one bounded
/// late watch that records the confirmation the reap missed and says so once.
/// Falsifier: drop the hook from the `Failed` arm and the late-window poll count
/// stays 0 (the silent-watch case); re-kill in the watch and the kill count is 2.
#[tokio::test]
async fn a_failure_the_reap_could_not_prove_watches_its_ticket_late() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let ticket = SpawnTicket {
        generation: 11,
        kind: ResourceKind::ChildProcess,
        pid: 7_101,
        port: 19_411,
    };
    let job_id = uninstalled_spawn_row(&mut state, "lategreen-row", &ticket);

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::Failed,
        LateAnswer::Confirmed,
        &mut rx,
    )
    .await;

    let job = &state.jobs[&job_id];
    assert!(
        job.terminal,
        "an unproven release terminalizes; state {:?}, error {:?}",
        job.state, job.error
    );
    assert_eq!(
        job.state,
        SwapState::Idle,
        "the funnel still walks Failed -> Reap -> Idle"
    );
    assert!(
        runtime.poll_count() > runtime.late_polls().len(),
        "the job's own reap polled for its release first"
    );
    let polls = await_late_polls(&runtime, |count| count >= 1).await;
    assert_eq!(
        polls,
        vec![(19_411, 11, "confirmed")],
        "the watch asks after its own ticket once the reap is gone"
    );
    // One bounded watch: the proof ends it, and no second warn follows.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(runtime.late_polls(), polls, "the watch stops on the proof");
    assert_eq!(
        runtime.kill_count(),
        1,
        "the job's own reap issued the only kill: the watch never re-kills"
    );
    assert_eq!(
        late_release_warns(
            &capture,
            "lategreen-row",
            "no runtime entry for this generation remains"
        ),
        1,
        "the late confirmation is named once, not silently"
    );
}

/// The `CancelRequested` arm: the same shape reached by an operator's cancel. The
/// row keeps its plain cancel wording (the slot had no occupant to keep installed),
/// and its orphan gets the one late watch.
#[tokio::test]
async fn a_cancel_the_reap_could_not_prove_watches_its_ticket_late() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let ticket = SpawnTicket {
        generation: 12,
        kind: ResourceKind::ChildProcess,
        pid: 7_102,
        port: 19_412,
    };
    let job_id = uninstalled_spawn_row(&mut state, "latecancel-row", &ticket);

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::CancelRequested,
        LateAnswer::Confirmed,
        &mut rx,
    )
    .await;

    let job = &state.jobs[&job_id];
    assert!(job.terminal, "the cancel terminalizes");
    assert_eq!(job.state, SwapState::Idle);
    assert_eq!(job.outcome.as_deref(), Some("cancelled"));
    assert!(
        job.error
            .as_ref()
            .is_some_and(|error| error.to_string().contains("unloaded by request")),
        "the row keeps the cancel's own error: {:?}",
        job.error
    );
    let polls = await_late_polls(&runtime, |count| count >= 1).await;
    assert_eq!(polls, vec![(19_412, 12, "confirmed")]);
    assert_eq!(runtime.kill_count(), 1, "the cancel's reap: the only kill");
    assert_eq!(
        late_release_warns(
            &capture,
            "latecancel-row",
            "no runtime entry for this generation remains"
        ),
        1
    );
}

/// A shape the first cut's comment got wrong: a cancel that reached
/// `Serving` and whose own reap could not prove the death keeps G12's posture — the
/// occupant stays installed and `/har/status` still reports the model serving — so no
/// late watch is armed on that ticket and no warn contradicts the page. Falsifier:
/// arm the watch on any spawned ticket (the pre-scope rule) and both the late poll
/// and the confirmation warn appear here.
#[tokio::test]
async fn a_cancel_of_an_installed_occupant_arms_no_late_watch() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let ticket = SpawnTicket {
        generation: 51,
        kind: ResourceKind::ChildProcess,
        pid: 7_151,
        port: 19_451,
    };
    let job_id = serving_occupant_row(&mut state, "lateinstalled-row", &ticket);

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::CancelRequested,
        LateAnswer::Confirmed,
        &mut rx,
    )
    .await;

    let job = &state.jobs[&job_id];
    assert!(job.terminal, "the cancel terminalizes as it always did");
    assert_eq!(job.state, SwapState::Idle);
    assert_eq!(job.outcome.as_deref(), Some("cancelled"));
    let installed = seated_occupant_ticket(&state)
        .expect("an unproven release never clears an installed occupant");
    assert_eq!(
        (installed.port, installed.generation),
        (19_451, 51),
        "the slot keeps reporting THIS spawn as serving, so the watch stays out"
    );
    assert_eq!(
        runtime.kill_count(),
        1,
        "the cancel's reap: still the only kill"
    );
    assert!(
        runtime.poll_count() > 0,
        "the reap polled for its own release inside its window"
    );
    // The late window is open and scripted Confirmed: a watch would poll on its first
    // pass and warn. Neither may happen.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        runtime.late_polls().is_empty(),
        "no late watch on an installed ticket: {:?}",
        runtime.late_polls()
    );
    for needle in [
        "no runtime entry for this generation remains",
        "the next load on this port or the boot reclaim must reap it",
        "ticket was superseded",
    ] {
        assert_eq!(
            late_release_warns(&capture, "lateinstalled-row", needle),
            0,
            "the page says serving; {needle:?} must not be said about it"
        );
    }
}

/// The other half of the watch scope: an occupant that is NOT this ticket — a
/// restart's predecessor still installed while its replacement died — must not blind
/// the watch. The engine the slot reports is a different spawn; this orphan is still
/// nobody's to reap. Falsifier: blind the watch on ANY occupant and the late poll
/// count here stays 0, which is the silent-watch case again.
#[tokio::test]
async fn a_slot_holding_another_generation_still_arms_the_late_watch() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let dead_spawn = SpawnTicket {
        generation: 61,
        kind: ResourceKind::ChildProcess,
        pid: 7_161,
        port: 19_461,
    };
    let predecessor = SpawnTicket {
        generation: 60,
        kind: ResourceKind::ChildProcess,
        pid: 7_160,
        port: 19_461,
    };
    let job_id = seat_spawn_row(
        &mut state,
        "latetwin-row",
        SwapState::Ready,
        &dead_spawn,
        Some(seated_occupant("latetwin-row", &predecessor)),
    );

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::Failed,
        LateAnswer::Confirmed,
        &mut rx,
    )
    .await;

    let polls = await_late_polls(&runtime, |count| count >= 1).await;
    assert_eq!(
        polls,
        vec![(19_461, 61, "confirmed")],
        "the watch names the generation it owns, never the installed one"
    );
    assert!(
        !runtime.generations_polled().contains(&60),
        "the predecessor's generation is the slot's business, not this watch's: {:?}",
        runtime.generations_polled()
    );
    assert_eq!(runtime.kill_count(), 1, "the job's own reap: the only kill");
    assert_eq!(
        late_release_warns(
            &capture,
            "latetwin-row",
            "no runtime entry for this generation remains"
        ),
        1
    );
}

/// The successor leg: once a newer spawn owns the port, `release_confirmed` reports
/// `StaleTicket` and the watch stops at once. It may not re-kill, may not keep
/// polling, and may never address the newer generation — an unowned kill wearing a
/// harness ticket is the one thing worse than the leak it watches.
#[tokio::test]
async fn a_superseded_ticket_ends_the_late_watch_without_touching_the_successor() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let ticket = SpawnTicket {
        generation: 21,
        kind: ResourceKind::ChildProcess,
        pid: 7_111,
        port: 19_413,
    };
    let job_id = uninstalled_spawn_row(&mut state, "latesupersede-row", &ticket);

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::Failed,
        LateAnswer::Superseded,
        &mut rx,
    )
    .await;

    let polls = await_late_polls(&runtime, |count| count >= 1).await;
    assert_eq!(
        polls,
        vec![(19_413, 21, "superseded")],
        "the stale answer ends the watch on its first poll"
    );
    assert_eq!(runtime.kill_count(), 1, "the watch never re-kills");
    assert!(
        !runtime.generations_polled().contains(&22),
        "generation 22 is the successor's: the watch may not even ask after it: {:?}",
        runtime.generations_polled()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        runtime.late_polls(),
        polls,
        "the watch stops on the stale answer"
    );
    assert_eq!(
        late_release_warns(&capture, "latesupersede-row", "ticket was superseded"),
        1,
        "the stop is named once"
    );
    assert_eq!(
        late_release_warns(
            &capture,
            "latesupersede-row",
            "no runtime entry for this generation remains"
        ),
        0,
        "a superseded ticket never claims a release it never saw"
    );
}

/// The bound: a release that is never proven gets exactly one more
/// `release_timeout_s` of watching, then one warn naming port, generation and model
/// and the recovery that is left. Nothing here grows into a second reap task.
#[tokio::test]
async fn an_unproven_release_watches_one_window_and_then_reports_and_stops() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let ticket = SpawnTicket {
        generation: 31,
        kind: ResourceKind::ChildProcess,
        pid: 7_121,
        port: 19_414,
    };
    let job_id = uninstalled_spawn_row(&mut state, "latebound-row", &ticket);

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::Failed,
        LateAnswer::StillHeld,
        &mut rx,
    )
    .await;

    let started = await_late_polls(&runtime, |count| count >= 2).await;
    assert!(
        started.iter().all(|(_, _, verdict)| *verdict == "pending"),
        "the watch only asks, it never acts: {started:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let needle = "the next load on this port or the boot reclaim must reap it";
    while late_release_warns(&capture, "latebound-row", needle) == 0 {
        assert!(
            Instant::now() < deadline,
            "the watch never reported its own deadline"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let stopped = runtime.late_polls().len();
    assert!(stopped > started.len(), "the watch polled to its bound");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        runtime.late_polls().len(),
        stopped,
        "the task is bounded: it stops after one release_timeout_s"
    );
    assert_eq!(runtime.kill_count(), 1, "the watch never re-kills");
    assert_eq!(
        late_release_warns(&capture, "latebound-row", needle),
        1,
        "the residue is named once"
    );
    assert_eq!(
        late_release_warns(
            &capture,
            "latebound-row",
            "no runtime entry for this generation remains"
        ),
        0
    );
}

/// A `release_confirmed` that never returns cannot hold the watch past its
/// one window either. The hard outer timeout (the same window, plus one poll of slack
/// so the in-loop check stays the ordinary exit) ends the task and emits that single
/// bound warning. Falsifier: drop the outer timeout and no warning ever arrives — the
/// task stays parked inside the wedged runtime call forever.
#[tokio::test]
async fn a_wedged_release_check_cannot_outlive_the_bound() {
    let capture = install_warn_capture();
    let runtime = Arc::new(LateWatchRuntime::new());
    let (mut state, mut rx) = late_watch_state(Arc::clone(&runtime), 1);
    let ticket = SpawnTicket {
        generation: 71,
        kind: ResourceKind::ChildProcess,
        pid: 7_171,
        port: 19_471,
    };
    let job_id = uninstalled_spawn_row(&mut state, "latewedged-row", &ticket);

    drive_to_unproven_terminal(
        &mut state,
        &runtime,
        job_id,
        Arm::Failed,
        LateAnswer::Wedged,
        &mut rx,
    )
    .await;

    let polls = await_late_polls(&runtime, |count| count >= 1).await;
    assert_eq!(
        polls,
        vec![(19_471, 71, "wedged")],
        "the watch is stuck inside the single call it made"
    );
    let needle = "the next load on this port or the boot reclaim must reap it";
    // The hard bound is 1 window + 1 poll (1.05 s here); the 25 s ceiling only
    // tolerates parallel-suite scheduling noise, it never weakens the falsifier:
    // without the outer timeout this warning is never emitted at all.
    let deadline = Instant::now() + Duration::from_secs(25);
    while late_release_warns(&capture, "latewedged-row", needle) == 0 {
        assert!(
            Instant::now() < deadline,
            "the hard timeout never fired: the wedged check outlived the bound"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        runtime.kill_count(),
        1,
        "wedged or not, the watch never re-kills"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        runtime.late_polls().len(),
        1,
        "the task ended with the timeout: no poll past the bound"
    );
    assert_eq!(
        late_release_warns(&capture, "latewedged-row", needle),
        1,
        "the spent window is named exactly once, from the hard timeout"
    );
    assert_eq!(
        late_release_warns(
            &capture,
            "latewedged-row",
            "no runtime entry for this generation remains"
        ),
        0,
        "a wedged check proves nothing about the entry, so nothing is claimed"
    );
}
