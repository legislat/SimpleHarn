use super::*;

/// Seats the honest `Serving` shape [`ActorState::install`] leaves behind: a
/// terminal load row with outcome `loaded` on a slot whose job has cleared.
fn seat_serving_resident(state: &mut ActorState, name: &str, ticket: &SpawnTicket) -> JobId {
    let job_id = serving_occupant_row(state, name, ticket);
    {
        let row = state.jobs.get_mut(&job_id).expect("seated row");
        row.terminal = true;
        row.outcome = Some("loaded".to_string());
    }
    state.slots[0].job = None;
    job_id
}

/// Drains the armed unload job's release event and applies it on the actor's
/// turn — the commit point that frees the slot and terminalizes the victim row.
async fn drive_death_release(
    state: &mut ActorState,
    rx: &mut mpsc::UnboundedReceiver<ActorMsg>,
) -> JobId {
    let message = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the reap confirms inside the test deadline")
        .expect("the actor channel is open");
    let ActorMsg::Event { job, event } = message else {
        panic!("expected the death reap's release event");
    };
    state.apply_event(job, event);
    job
}

/// A resident the proxy witnessed dead leaves `Serving` behind only
/// until the death report runs. After it: the slot is empty, the owning load
/// row is terminal and no longer claims it serves, and the release row names
/// the cause of death. Falsifier: make the handler's match always take the
/// `None` arm (never arming `begin_unload_slot`) and the slot stays `serving`
/// with a `Serving` load row — the live proof's `/har/status` shape — while
/// the release event never arrives.
#[tokio::test]
async fn a_witnessed_engine_death_empties_its_slot_and_terminalizes_its_load_row() {
    let (mut state, runtime, mut rx) = death_state();
    let ticket = fold_ticket(19_555);
    let load_job = seat_serving_resident(&mut state, "rc02-dead", &ticket);
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    state.on_report_dead_engine(
        "rc02-dead".to_string(),
        CategoryId::Work,
        ticket.port,
        reply_tx,
    );
    assert_eq!(
        reply_rx
            .try_recv()
            .expect("the accept reply")
            .expect("the command accepted"),
        Reply::Unloaded {
            unloading: 1,
            jobs_cancelled: 0,
        },
        "the report arms exactly one release and cancels nothing"
    );
    let unload_job = drive_death_release(&mut state, &mut rx).await;
    assert_eq!(
        runtime.killed(),
        vec![(19_555, 7)],
        "one reap under the dead resident's own ticket"
    );
    assert!(
        state.slots[0].occupant.is_none(),
        "the confirmed release freed the slot"
    );
    let row = &state.jobs[&load_job];
    assert_eq!(
        row.state,
        SwapState::Idle,
        "the dead resident's row left Serving"
    );
    assert_eq!(row.outcome.as_deref(), Some("unloaded"));
    assert!(
        row.terminal,
        "no live load row may survive the death report"
    );
    assert!(row.error.is_none(), "a release is not a fault");
    let unload_row = &state.jobs[&unload_job];
    assert!(unload_row.terminal);
    assert_eq!(
        unload_row.outcome.as_deref(),
        Some("engine died"),
        "the ledger names the cause of death"
    );
    let snapshot = state.snapshot();
    assert_eq!(snapshot.slots[0].state, "empty");
    assert!(snapshot.slots[0].model.is_none());
    assert!(
        snapshot
            .jobs
            .iter()
            .filter(|row| row.model == "rc02-dead" && row.kind == JobKind::Load)
            .all(|row| row.terminal && row.state != SwapState::Serving),
        "/har/status may keep no Serving load row for a dead resident: {:?}",
        snapshot.jobs
    );
}

/// The death report's refusal legs: a report naming a port no job-free slot of the
/// category serves — the witness was stale, a successor took another port —
/// arms nothing, touches no row and reaps nobody, and an already-emptied slot
/// answers the same report idempotently. Falsifier: matching on the model
/// name alone (ignoring the port) reaps a successor the request never saw,
/// and turning the empty-slot reply into a fake acceptance goes red here.
#[tokio::test]
async fn a_death_report_naming_a_port_no_job_free_slot_serves_arms_nothing() {
    let (mut state, runtime, mut rx) = death_state();
    let ticket = fold_ticket(19_556);
    let load_job = seat_serving_resident(&mut state, "rc02-port", &ticket);
    let jobs_before = state.jobs.len();
    for wrong_port in [ticket.port + 1, 0] {
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        state.on_report_dead_engine(
            "rc02-port".to_string(),
            CategoryId::Work,
            wrong_port,
            reply_tx,
        );
        assert_eq!(
            reply_rx
                .try_recv()
                .expect("the accept reply")
                .expect("the command accepted"),
            Reply::Unloaded {
                unloading: 0,
                jobs_cancelled: 0,
            },
            "a report for a port the slot does not serve moves nothing (port {wrong_port})"
        );
    }
    assert!(
        state.slots[0].occupant.is_some(),
        "the resident stands untouched"
    );
    assert_eq!(state.jobs.len(), jobs_before, "no job was armed");
    assert!(runtime.killed().is_empty(), "nothing was reaped");
    assert_eq!(state.jobs[&load_job].state, SwapState::Serving);
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    state.slots[0].occupant = None;
    state.on_report_dead_engine(
        "rc02-port".to_string(),
        CategoryId::Work,
        ticket.port,
        reply_tx,
    );
    assert_eq!(
        reply_rx
            .try_recv()
            .expect("the accept reply")
            .expect("the command accepted"),
        Reply::Unloaded {
            unloading: 0,
            jobs_cancelled: 0,
        },
        "an already-emptied slot answers the report idempotently"
    );
    assert_eq!(state.jobs.len(), jobs_before);
    assert!(
        rx.try_recv().is_err(),
        "an unarmed report spawns no release event"
    );
}

/// The death report's blast-radius leg: the report names ONE resident. A sibling
/// occupant in another slot of the same category — its own ticket, its own
/// port — keeps its slot, its row and its engine. Falsifier: unloading the
/// whole category on a death report kills the sibling's occupant and turns
/// this pin red on the kill list and the sibling's still-`Serving` row.
#[tokio::test]
async fn a_death_report_never_touches_another_slots_resident() {
    let (mut state, runtime, mut rx) = death_state();
    let dead_ticket = fold_ticket(19_557);
    let survivor_ticket = SpawnTicket {
        generation: 8,
        kind: ResourceKind::ChildProcess,
        pid: 4_300,
        port: 19_558,
    };
    let dead_job = seat_serving_resident(&mut state, "rc02-victim", &dead_ticket);
    let survivor_job = seat_serving_resident(&mut state, "rc02-survivor", &survivor_ticket);
    // `seat_serving_resident` re-seats slot 0 for every call; rebuild both
    // residents side by side in the same category, each row pointing at its
    // own slot.
    state.slots[0] = Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: dead_ticket.port,
        occupant: Some(seated_occupant("rc02-victim", &dead_ticket)),
        job: None,
    };
    state.slots.push(Slot {
        category: CategoryId::Work,
        index: 1,
        engine_port: survivor_ticket.port,
        occupant: Some(seated_occupant("rc02-survivor", &survivor_ticket)),
        job: None,
    });
    state
        .jobs
        .get_mut(&survivor_job)
        .expect("survivor row")
        .slot = Some(1);
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    state.on_report_dead_engine(
        "rc02-victim".to_string(),
        CategoryId::Work,
        dead_ticket.port,
        reply_tx,
    );
    assert_eq!(
        reply_rx
            .try_recv()
            .expect("the accept reply")
            .expect("the command accepted"),
        Reply::Unloaded {
            unloading: 1,
            jobs_cancelled: 0,
        },
        "exactly one slot's death is armed"
    );
    drive_death_release(&mut state, &mut rx).await;
    assert_eq!(
        runtime.killed(),
        vec![(19_557, 7)],
        "only the dead resident's ticket was reaped"
    );
    assert!(state.slots[0].occupant.is_none());
    assert_eq!(state.jobs[&dead_job].state, SwapState::Idle);
    let survivor = state.slots[1]
        .occupant
        .as_ref()
        .expect("the sibling keeps its slot");
    assert_eq!(survivor.model, "rc02-survivor");
    assert_eq!(survivor.ticket, survivor_ticket);
    let survivor_row = &state.jobs[&survivor_job];
    assert_eq!(survivor_row.state, SwapState::Serving);
    assert_eq!(survivor_row.outcome.as_deref(), Some("loaded"));
}
