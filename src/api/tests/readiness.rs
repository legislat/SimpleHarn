use super::*;

/// A load that misses its readiness deadline answers its own named
/// code `readiness_timeout`, names the deadline, and sanitizes the probe's
/// text; every neighboring class keeps its current code. Legs: the composed
/// `spawn_probe_task` shape maps with the URL/OS text stripped (also with the
/// container publish annotation appended), a `health answered` outcome keeps
/// only the status, a container annotation after it never reaches the wire;
/// an unrelated `EngineFailed`/`Spawn` reason stays `upstream`; a live-but-
/// slow engine (real reqwest timeout against a hanging listener) stays the
/// plain `upstream` timeout sentence and never a readiness code. Falsifiers:
/// map every `EngineFailed` to the new code and the neighbor legs fail; drop
/// the URL leg's leak asserts and a raw-reason pass-through returns; cut only
/// the prefix (not the separator) and the negative shapes fail.
#[tokio::test]
async fn a_readiness_deadline_answers_its_own_code_and_the_other_classes_keep_theirs() {
    // (a) the shape `spawn_probe_task` composes: deadline + refused-URL probe.
    let reason = "readiness deadline 300s exceeded; last: no answer yet: \
                  error sending request for url (http://127.0.0.1:18110/health)";
    let mapped = ApiError::from(crate::scheduler::SchedulerError::EngineFailed(
        reason.to_string(),
    ));
    assert_eq!(mapped.code(), "readiness_timeout");
    assert_eq!(mapped.status(), StatusCode::BAD_GATEWAY);
    let body = error_body(&mapped);
    assert_eq!(body["code"], json!("readiness_timeout"));
    assert_eq!(body["error"]["code"], json!("readiness_timeout"));
    assert_eq!(
        body["message"],
        json!("readiness deadline 300 s exceeded; the engine did not answer the health probe")
    );
    let wire = body.to_string();
    for leak in [
        "127.0.0.1",
        "18110",
        "error sending request",
        "os error",
        "/health",
    ] {
        assert!(
            !wire.contains(leak),
            "the client body must not carry the probe's OS detail {leak:?}: {wire}"
        );
    }
    // (b) the same shape with the container publish annotation appended: the
    // generic arm already collapses it, and a `health answered` outcome with
    // the same annotation must keep only the status.
    let annotated = "readiness deadline 600s exceeded; last: health answered 503 Service \
                     Unavailable; container ninfer-a is published on 127.0.0.1:55999";
    let mapped = ApiError::from(crate::scheduler::SchedulerError::EngineFailed(
        annotated.to_string(),
    ));
    assert_eq!(mapped.code(), "readiness_timeout");
    let body = error_body(&mapped).to_string();
    assert!(
        body.contains("the engine health endpoint answered 503 Service Unavailable"),
        "the health status stays human-readable: {body}"
    );
    for leak in ["127.0.0.1", "55999", "ninfer-a", "published"] {
        assert!(
            !body.contains(leak),
            "the publish annotation must stay in the log, not the wire: {leak:?} in {body}"
        );
    }
    // (c) reasons that are NOT this shape keep the generic class.
    for other in [
        "engine process exited early (exit code 1)",
        "engine died: readiness deadline 5s exceeded; last: no answer yet: x",
        "readiness deadline 300s but the probe never answered at all",
        "spawn failed: the engine binary refused to start",
    ] {
        assert_eq!(
            ApiError::from(crate::scheduler::SchedulerError::EngineFailed(
                other.to_string()
            ))
            .code(),
            "upstream",
            "not the composed shape, not the readiness code: {other}"
        );
    }
    assert_eq!(
        ApiError::from(crate::scheduler::SchedulerError::Spawn(
            "no adapter registered for engine `ghost`".to_string()
        ))
        .code(),
        "upstream"
    );
    // (d) live-but-slow: a REAL reqwest timeout against an engine that accepted
    // the connection and never answers is the plain upstream class, never
    // `readiness_timeout` and never death.
    let engine = std::net::TcpListener::bind("127.0.0.1:0").expect("hanging engine");
    let port = engine.local_addr().expect("addr").port();
    let hang = std::thread::spawn(move || {
        let held = engine.accept().expect("one connection");
        // Hold the established connection open past the client deadline: a
        // refused or immediately closed socket would be a different class.
        std::thread::sleep(Duration::from_secs(1));
        drop(held);
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .expect("client");
    let error = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&json!({ "model": "slow" }))
        .send()
        .await
        .expect_err("the hanging engine never answers within 300 ms");
    let classified = classify_send_failure(&error, port).expect_err("a timeout is never a restart");
    assert_eq!(classified.code(), "upstream");
    assert!(
        !matches!(classified, ApiError::ReadinessTimeout { .. }),
        "a live-but-slow engine never takes the readiness class"
    );
    let _ = hang;
}

/// The negative-shape guard for readiness timeouts: `readiness_timeout_answer`
/// recognizes ONLY
/// the exact composed shape — a reason that merely mentions the words, or
/// carries the shape without the prefix, must answer `None` so it keeps its
/// current class. Falsifier: matching on `contains("readiness deadline")`
/// instead of prefix+separator sends these reasons into the new code.
#[test]
fn only_the_exact_deadline_shape_maps_to_the_readiness_answer() {
    for not_a_shape in [
        "",
        "readiness deadline 5s exceeded",
        "prefix noise readiness deadline 5s exceeded; last: no answer yet: x",
        "readiness deadline  exceeded; last: no answer yet: x",
    ] {
        if not_a_shape == "readiness deadline  exceeded; last: no answer yet: x" {
            // Degenerate-but-shape-matching: the figure stays verbatim, the
            // sentence stays honest, and nothing crashes or leaks.
            let answer = readiness_timeout_answer(not_a_shape).expect("shape matches");
            assert!(
                answer.starts_with("readiness deadline  exceeded;"),
                "{answer}"
            );
            assert!(!answer.contains("no answer yet"), "{answer}");
            continue;
        }
        assert!(
            readiness_timeout_answer(not_a_shape).is_none(),
            "not the exact shape: {not_a_shape:?}"
        );
    }
}

/// Readiness timeout over the wire: `/har/load` of a model whose readiness
/// probe never
/// lands (deadline 1 s) answers the named code through the real router —
/// 502 `readiness_timeout`, the deadline figure named, the probe's engine URL
/// and reqwest text absent from the body. Falsifier: revert the `From`
/// mapping to a blanket `Upstream` and the `code` leg answers `upstream`.
#[tokio::test]
async fn a_load_that_misses_its_readiness_deadline_answers_the_named_code() {
    let (api, runtime) = scripted_live_state(
        aux_config_on_reserved_ports(1, 1),
        &[("rc01-slow", "engine: mock\ncategory: aux\nvram_gb: 3\n")],
    );
    runtime
        .probe_ready
        .store(false, std::sync::atomic::Ordering::Release);
    let (status, body) = post_control(&api, "/har/load", json!({ "model": "rc01-slow" })).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], json!("readiness_timeout"), "{body}");
    assert_eq!(
        body["message"],
        json!("readiness deadline 1 s exceeded; the engine did not answer the health probe")
    );
    let wire = body.to_string();
    for leak in [
        "127.0.0.1",
        "18111",
        "no answer yet",
        "error sending request",
    ] {
        assert!(
            !wire.contains(leak),
            "the /har/load refusal carries no probe OS detail {leak:?}: {wire}"
        );
    }
    // The failed load left no resident behind: its slot is empty again.
    let (status, doc) = get_control(&api, "/har/status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        doc["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .all(|slot| slot["model"] != json!("rc01-slow")),
        "a load that never reached readiness may not occupy a slot: {doc}"
    );
}
