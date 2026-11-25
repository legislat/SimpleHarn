use super::*;

/// Witnessed-death terminalization over the wire: a proxied request that
/// witnesses its engine's port
/// refuse TWO connections terminalizes what it witnessed. The first refusal
/// still takes the crash-retry (reap + reload + one retry, unchanged);
/// the successor's refusal answers an honest typed 502 AND leaves `/har/status`
/// with no serving slot, no Serving row and an `engine died` ledger line for
/// the dead model — while the category's OTHER resident keeps serving
/// untouched. Falsifiers: drop the death report from the exhausted arm and
/// the doomed model still reads `serving` after the loop settles; report on
/// the FIRST refusal and the 200-after-retry contract (and this pin's spawn
/// count of three) goes red; name the wrong port and the sibling's slot dies.
#[tokio::test]
async fn a_proxy_request_that_witnessed_its_engine_death_terminalizes_it() {
    let (api, runtime) = scripted_live_state(
        aux_config_on_reserved_ports(2, 120),
        &[
            ("rc02-doomed", "engine: mock\ncategory: aux\nvram_gb: 3\n"),
            ("rc02-alive", "engine: mock\ncategory: aux\nvram_gb: 3\n"),
        ],
    );
    for model in ["rc02-doomed", "rc02-alive"] {
        let (status, body) = post_control(&api, "/har/load", json!({ "model": model })).await;
        assert_eq!(status, StatusCode::OK, "load {model}: {body}");
    }
    assert_eq!(runtime.spawn_count(), 2, "exactly one engine per load");
    // Nothing listens on the planned engine ports: every forward refuses.
    let (status, body) = post_aux_chat(&api, chat_body("rc02-doomed")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], json!("upstream"), "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("upstream failure"),
        "the exhausted crash-retry answers the honest upstream sentence: {body}"
    );
    assert_eq!(runtime.spawn_count(), 3, "exactly one reload, no more");
    // The witnessed death: two reaps of the doomed slot's own port — the
    // crash-retry's corpse and the reported successor — and never the
    // sibling's port.
    let killed = runtime.killed();
    assert_eq!(
        killed.len(),
        2,
        "corpse + successor, both under the doomed slot: {killed:?}"
    );
    assert_eq!(killed[0].0, killed[1].0, "one slot, one port: {killed:?}");
    assert_ne!(
        killed[0].1, killed[1].1,
        "two generations of the same slot: {killed:?}"
    );
    // Settle: the status stops claiming a dead resident.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let doc = loop {
        let (status, doc) = get_control(&api, "/har/status").await;
        assert_eq!(status, StatusCode::OK);
        let doomed_serving = doc["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .any(|slot| slot["model"] == json!("rc02-doomed"));
        let doomed_live_row = doc["jobs"].as_array().expect("jobs").iter().any(|row| {
            row["model"] == json!("rc02-doomed") && !row["terminal"].as_bool().unwrap_or(false)
        });
        if !doomed_serving && !doomed_live_row {
            break doc;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the death report never reached /har/status: {doc}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let jobs = doc["jobs"].as_array().expect("jobs");
    assert!(
        jobs.iter()
            .filter(|row| row["model"] == json!("rc02-doomed") && row["kind"] == json!("Load"))
            .all(|row| row["state"] != json!("Serving")),
        "no load row of the dead resident may still claim Serving: {doc}"
    );
    assert!(
        jobs.iter().any(|row| row["model"] == json!("rc02-doomed")
            && row["kind"] == json!("Unload")
            && row["outcome"] == json!("engine died")),
        "the ledger names the witnessed death: {doc}"
    );
    // The sibling is untouched: still serving, its engine never reaped.
    let alive_slot = doc["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .find(|slot| slot["model"] == json!("rc02-alive"))
        .expect("the sibling keeps serving");
    assert_eq!(alive_slot["state"], json!("serving"));
    let alive_port = alive_slot["port"].as_u64().expect("alive port");
    assert!(
        killed.iter().all(|(port, _)| *port as u64 != alive_port),
        "the death report never touched the sibling's port: {killed:?} vs {alive_port}"
    );
    assert!(
        jobs.iter()
            .filter(|row| row["model"] == json!("rc02-alive") && row["kind"] == json!("Load"))
            .any(|row| row["state"] == json!("Serving") && row["outcome"] == json!("loaded")),
        "the sibling's row still honestly serves: {doc}"
    );
}

/// The never-death leg: an engine that ACCEPTS the connection and drops
/// it before any response head is a plain forward failure — the `Other` class.
/// It answers 502 and NOTHING is terminalized: the resident stays serving,
/// nothing is reaped, nothing reloads, and the ledger gains no unload row.
/// Together with the timeout leg in
/// [`a_readiness_deadline_answers_its_own_code_and_the_other_classes_keep_theirs`]
/// (live-but-slow never restarts) this pins that only witnessed refused-
/// connects terminalize. Falsifier: classify a connection-reset before the
/// head as death (or report on any 502) and the resident here goes red.
#[tokio::test]
async fn an_engine_that_drops_the_connection_serves_502_but_stays_serving() {
    let engine = std::net::TcpListener::bind("127.0.0.1:0").expect("closed-before-answer engine");
    let port = engine.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        // The `Other` class engine: accepts, then drops the socket before any
        // response head — a reset/closed connection, never a refused connect.
        while engine.accept().is_ok() {}
    });
    let mut config = aux_config_on_reserved_ports(1, 120);
    config
        .category_mut(CategoryId::Aux)
        .expect("AUX is configured")
        .engine_ports = vec![port];
    let (api, runtime) = scripted_live_state(
        config,
        &[("rc02-closed", "engine: mock\ncategory: aux\nvram_gb: 3\n")],
    );
    let (status, body) = post_control(&api, "/har/load", json!({ "model": "rc02-closed" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post_aux_chat(&api, chat_body("rc02-closed")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], json!("upstream"), "{body}");
    let (doc_status, doc) = get_control(&api, "/har/status").await;
    assert_eq!(doc_status, StatusCode::OK);
    let serving = doc["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .any(|slot| slot["model"] == json!("rc02-closed") && slot["state"] == json!("serving"));
    assert!(
        serving,
        "a dropped connection before the head is not a witnessed death: {doc}"
    );
    assert!(runtime.killed().is_empty(), "nothing was reaped");
    assert_eq!(runtime.spawn_count(), 1, "nothing was reloaded");
    assert!(
        doc["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .all(|row| !(row["model"] == json!("rc02-closed") && row["kind"] == json!("Unload"))),
        "the Other class arms no unload: {doc}"
    );
}

/// The structural seam of witnessed-death terminalization (the repo's
/// established `include_str!`
/// pattern): the death command is reachable ONLY after the crash-retry is
/// exhausted — classification first, one restart, and only the second refused
/// connect reports. Falsifier: move the `report_dead_engine` call above the
/// restart arm (death on the first refusal) and the order pin fails; call it
/// twice and the count pin fails.
#[test]
fn the_death_report_is_reachable_only_from_the_exhausted_crash_retry() {
    let source = include_str!("../proxy.rs");
    let at = |needle: &str| {
        source
            .find(needle)
            .unwrap_or_else(|| panic!("the proxy loop no longer contains {needle:?}"))
    };
    let classify = at("classify_send_failure(&error, port)?;");
    let bump = at("attempts += 1;");
    let exhausted = at("if attempts >= 2");
    let report = at(".report_dead_engine(");
    let answer = at("unreachable after crash-retry");
    let restart = at(".restart(&model, listener.category, port)");
    assert!(
        classify < bump
            && bump < exhausted
            && exhausted < report
            && report < answer
            && answer < restart,
        "classify → count → exhaustion → report → typed 502, restart only for the \
         first refusal: classify={classify} bump={bump} exhausted={exhausted} \
         report={report} answer={answer} restart={restart}"
    );
    assert_eq!(
        source.matches(".report_dead_engine(").count(),
        1,
        "exactly one death witness in the proxy loop"
    );
}
