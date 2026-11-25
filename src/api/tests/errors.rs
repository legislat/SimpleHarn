use super::*;

#[test]
fn a_dead_actor_maps_to_the_named_scheduler_unavailable_class() {
    let error = ApiError::from(SchedulerError::ActorGone);
    assert_eq!(error.code(), "scheduler_unavailable");
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        error.to_string().contains("scheduler is not running"),
        "the generic message names the stopped scheduler: {error}"
    );
}

#[test]
fn a_scheduler_invariant_violation_maps_to_a_generic_internal_error() {
    let reason = "internal detail that must stay out of the client body";
    let mapped = ApiError::from(SchedulerError::InvariantViolation {
        reason: reason.to_string(),
    });

    assert_eq!(mapped.code(), "invariant_violation");
    assert_eq!(mapped.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = error_body(&mapped);
    let message =
        "internal error: a scheduler invariant was violated; the reason is in the harness log";
    assert_eq!(body["code"], "invariant_violation");
    assert_eq!(body["error"]["code"], "invariant_violation");
    assert_eq!(body["message"], message);
    assert_eq!(body["error"]["message"], message);

    let rendered = body.to_string();
    assert!(
        !rendered.contains(reason),
        "the detailed invariant reason must not reach the client: {rendered}"
    );
}

#[tokio::test]
async fn a_dead_actor_answers_status_with_a_degraded_document_not_a_500() {
    let api = unit_state();
    api.scheduler
        .health()
        .mark_down_for_test("scheduler stopped");
    let doc = status_document(&api)
        .await
        .expect("degraded status document");
    assert_eq!(doc["scheduler_state"], json!("down"));
    assert!(
        doc["scheduler_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("restart the harness")),
        "the degraded reason names the recovery: {}",
        doc["scheduler_reason"]
    );
    assert_eq!(doc["slots"], json!([]));
    assert_eq!(doc["jobs"], json!([]));
    let merged = merged_state(&api).await.expect("degraded merged state");
    assert_eq!(merged["scheduler_state"], json!("down"));
    assert_eq!(merged["models"], json!([]));
}

struct NanWitness;

#[async_trait::async_trait]
impl VramWitness for NanWitness {
    async fn sample(&self) -> Result<MemorySample, String> {
        Ok(MemorySample {
            total_gib: f64::NAN,
            used_gib: 1.0,
            processes: Vec::new(),
            attribution: AttributionQuality::Unavailable,
        })
    }

    fn source(&self) -> String {
        "nan-test".to_string()
    }
}

#[tokio::test]
async fn a_non_finite_witness_total_is_treated_as_unavailable() {
    let mut api = unit_state();
    api.witness = Arc::new(NanWitness);
    let doc = render_status_document(&api, &SchedulerStatus::empty()).await;
    assert_eq!(doc["mem"]["total_basis"], json!("configured"));
    assert_eq!(doc["mem"]["unavailable_code"], json!("non_finite"));
    assert_eq!(doc["mem"]["observed_used_gb"], json!(null));
}

#[test]
fn error_body_carries_flat_and_openai_envelope() {
    // A2: both shapes present and consistent.
    let err = ApiError::NotFound {
        what: "model `x`".to_string(),
    };
    let body = error_body(&err);
    assert_eq!(body["code"], "not_found");
    assert_eq!(body["message"], "not found: model `x`");
    assert_eq!(body["error"]["type"], "simpleharn_error");
    assert_eq!(body["error"]["code"], "not_found");
    assert_eq!(body["error"]["message"], body["message"]);
    // An error without a wait hint grows neither field (falsifier: render
    // the hint for every class and the `not_found` body goes red here).
    assert!(body.get("queue_position").is_none());
    assert!(body.get("expected_release_s").is_none());
}

#[test]
fn error_body_renders_the_wait_hint_as_flat_fields_only() {
    // Both fields present: place + estimate at the flat top level, seconds
    // rounded to one decimal, the OpenAI sub-object untouched.
    let err = ApiError::Unavailable {
        reason: "insufficient VRAM".to_string(),
        wait_hint: Some(crate::budget::WaitHint {
            expected_release_s: Some(4.249),
            queue_position: Some(2),
        }),
    };
    let body = error_body(&err);
    assert_eq!(body["queue_position"], json!(2));
    assert_eq!(body["expected_release_s"], json!(4.2));
    assert_eq!(body["error"]["message"], body["message"]);
    assert_eq!(body["error"]["type"], "simpleharn_error");
    assert!(body["error"].get("queue_position").is_none());
    assert!(body["error"].get("expected_release_s").is_none());

    // A refusal that got no queue place renders an explicit null, and a
    // hint without any release sample omits the estimate key entirely
    // (falsifier: invent `expected_release_s: 0.0` and this leg goes red —
    // the harness must stay silent when it has no basis).
    let no_basis = ApiError::BelowLaunchFloor {
        reason: "launch floor not met".to_string(),
        wait_hint: Some(crate::budget::WaitHint::default()),
    };
    let body = error_body(&no_basis);
    assert_eq!(body["code"], "below_launch_floor");
    assert_eq!(
        body.get("queue_position"),
        Some(&Value::Null),
        "no place is a stated null, not a missing key: {body}"
    );
    assert!(body.get("expected_release_s").is_none(), "{body}");
}

/// The `whole` refusal's wire shape: it rides the
/// existing `unavailable` wait posture — no new code, no `min_free`
/// figure — and the B2 hint fields behave exactly as on `Insufficient`.
/// Falsifier: map it to `below_launch_floor` instead and the code pin
/// goes red.
#[test]
fn a_whole_machine_refusal_travels_as_unavailable() {
    let admission = crate::budget::AdmissionError::WholeMachineBusy {
        required: crate::budget::Gibibytes(68.0),
        free: crate::budget::Gibibytes(40.0),
        reserve: crate::budget::Gibibytes(2.0),
        facts: Box::new(crate::budget::BudgetFacts::default()),
        blockers: vec![crate::budget::BlockerInfo {
            model: "aux-resident".to_string(),
            category: "AUX".to_string(),
            reason: "whole machine requested".to_string(),
        }],
        wait_hint: crate::budget::WaitHint {
            expected_release_s: None,
            queue_position: Some(3),
        },
    };
    let msg = admission.to_string();
    assert!(
        msg.contains("whole machine requested") && msg.contains("aux-resident"),
        "{msg}"
    );
    assert!(
        msg.contains("at least 68.0 GiB free required"),
        "the refusal must print the target it needs (B5R-02): {msg}"
    );
    let err = ApiError::from(SchedulerError::Admission(admission));
    assert_eq!(err.code(), "unavailable");
    assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = error_body(&err);
    assert_eq!(body["code"], "unavailable");
    assert_eq!(body["queue_position"], json!(3));
    assert!(
        body.get("expected_release_s").is_none(),
        "no release basis stays silent, never zero: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("whole machine requested")),
        "{body}"
    );
}

#[test]
fn status_codes_come_from_types_not_text() {
    // A model literally named "not found" must not change any mapping (E1).
    let err = ApiError::NotFound {
        what: "not found model".to_string(),
    };
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    assert_eq!(err.code(), "not_found");
    assert_eq!(
        ApiError::BadRequest {
            reason: "x".to_string()
        }
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        ApiError::Unavailable {
            reason: "x".to_string(),
            wait_hint: None,
        }
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        ApiError::Upstream {
            reason: "x".to_string()
        }
        .status(),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(
        ApiError::Conflict {
            reason: "x".to_string()
        }
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        ApiError::NotImplemented {
            reason: "x".to_string()
        }
        .status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        ApiError::Internal {
            reason: "x".to_string()
        }
        .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn scheduler_errors_map_to_typed_statuses() {
    assert_eq!(
        ApiError::from(SchedulerError::UnknownModel("m".to_string())).status(),
        StatusCode::NOT_FOUND
    );
    let admission = SchedulerError::Admission(crate::budget::AdmissionError::Insufficient {
        need: crate::budget::Gibibytes(50.0),
        free: crate::budget::Gibibytes(20.0),
        reserve: crate::budget::Gibibytes(2.0),
        facts: Box::new(crate::budget::BudgetFacts {
            budget: Some(crate::budget::Gibibytes(20.0)),
            observed_free: None,
            ..Default::default()
        }),
        victims_considered: Vec::new(),
        blockers: Vec::new(),
        wait_hint: crate::budget::WaitHint {
            expected_release_s: Some(4.0),
            queue_position: Some(2),
        },
    });
    let mapped = ApiError::from(admission);
    assert_eq!(mapped.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        mapped.to_string().contains("50.0 GiB"),
        "numbers must travel: {mapped}"
    );
    assert_eq!(
        mapped.wait_hint(),
        Some(crate::budget::WaitHint {
            expected_release_s: Some(4.0),
            queue_position: Some(2),
        }),
        "the decision's wait hint rides the typed error, not only the message"
    );
    assert_eq!(
        ApiError::from(SchedulerError::Spawn("boom".to_string())).status(),
        StatusCode::BAD_GATEWAY
    );
    // A dead witness is its own client-visible class: "the card is unreadable"
    // must not be mistaken for "the card is full".
    let witness_down =
        SchedulerError::Admission(crate::budget::AdmissionError::WitnessUnavailable {
            reason: "nvidia-smi timed out".to_string(),
        });
    let mapped = ApiError::from(witness_down);
    assert_eq!(mapped.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(mapped.code(), "witness_unavailable");
    assert!(
        mapped.to_string().contains("nvidia-smi timed out"),
        "the witness's reason must travel: {mapped}"
    );
}

/// The RAM refusal is a plain over-commit wait on its own resource
/// (over-commit on host RAM): `unavailable` (503), the numbers in
/// the message, the decision's wait hint on the typed error — the same
/// shape `Insufficient` has, judged on host RAM.
#[test]
fn the_ram_refusal_maps_to_unavailable_with_its_numbers() {
    let admission = SchedulerError::Admission(crate::budget::AdmissionError::InsufficientRam {
        need: crate::budget::Gibibytes(24.0),
        free: crate::budget::Gibibytes(6.0),
        reserve: crate::budget::Gibibytes(8.0),
        facts: Box::new(crate::budget::BudgetFacts::default()),
        victims_considered: Vec::new(),
        blockers: Vec::new(),
        wait_hint: crate::budget::WaitHint {
            expected_release_s: None,
            queue_position: Some(3),
        },
    });
    let mapped = ApiError::from(admission);
    assert_eq!(mapped.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(mapped.code(), "unavailable");
    let msg = mapped.to_string();
    assert!(
        msg.contains("need 24.0 GiB")
            && msg.contains("free 6.0 GiB")
            && msg.contains("reserve 8.0 GiB"),
        "the RAM numbers must travel: {msg}"
    );
    assert_eq!(
        mapped.wait_hint().and_then(|h| h.queue_position),
        Some(3),
        "the decision's queue place rides the typed error"
    );
}

/// The two harness-failure classes carry OS-level failure
/// text — paths into model yamls, log files, the engine exe. The class stays
/// legible (`code` and message prefix unchanged, both envelope shapes equal),
/// but the client body must not carry the detail; `error_body` sends it to
/// `tracing::error!` instead (the live 500 with a capturing subscriber is
/// pinned in `tests/api_surface/control_plane.rs`). The request-side classes
/// still state their own numbers verbatim: the withholding is scoped, not a
/// mute.
#[test]
fn harness_failure_bodies_hide_the_os_detail_but_keep_the_class() {
    let leaked = "cannot read C:\\ModelConf\\qwen3.yaml: Access is denied. (os error 5)";
    for (error, prefix) in [
        (
            ApiError::Internal {
                reason: leaked.to_string(),
            },
            "internal error:",
        ),
        (
            ApiError::Upstream {
                reason: leaked.to_string(),
            },
            "upstream failure:",
        ),
    ] {
        let body = error_body(&error);
        assert_eq!(body["code"], error.code());
        assert_eq!(body["error"]["code"], error.code());
        assert_eq!(body["error"]["message"], body["message"]);
        let message = body["message"].as_str().expect("string message");
        assert!(
            message.starts_with(prefix),
            "{prefix} must survive: {message}"
        );
        assert!(
            message.contains("harness log"),
            "the body must say where the full reason lives: {message}"
        );
        for secret in ["cannot read", "ModelConf", "qwen3.yaml", "os error"] {
            assert!(
                !body.to_string().contains(secret),
                "the client body may not carry OS detail ({secret}): {body}"
            );
        }
    }
    let numbers = ApiError::Unavailable {
        reason: "the load needs 18.0 GiB free; 9.4 GiB is".to_string(),
        wait_hint: None,
    };
    let body = error_body(&numbers);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("18.0 GiB free; 9.4 GiB")),
        "admission numbers are the client's business: {body}"
    );
}

/// The ring is the operator's only "why did the last load
/// fail" channel and the guard its one request producer, so a client id must
/// not become an eviction channel or a megabyte echo. The useful half (a
/// short printable id echoes) is pinned by
/// `request_ids_are_echoed_or_generated`; this pins the wall — an oversized
/// id, or one with a space or a tab, regenerates a `req-` id in the response
/// header AND in the `request complete` line, and the rejected text never
/// reaches the ring. Falsifier: drop the `.filter(accepted_request_id)` in
/// `guard` and the three regenerated legs echo verbatim red.
#[tokio::test]
async fn client_request_ids_are_capped_and_sanitized_before_echo_and_log() {
    crate::observability::set_extended_logging(false);
    let api = Arc::new(unit_state());
    warm_capture_lines(&["request complete"], || async {
        let response = control_router(Arc::clone(&api))
            .oneshot(
                Request::get("/health")
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("warm-up request"),
            )
            .await
            .expect("in-process warm-up");
        assert_eq!(response.status(), StatusCode::OK);
    })
    .await;
    let session = crate::tray::testlog::Session::start();
    let echoed_id = |builder: axum::http::request::Builder| {
        let api = Arc::clone(&api);
        async move {
            let response = control_router(api)
                .oneshot(builder.body(Body::empty()).expect("probe request"))
                .await
                .expect("in-process probe");
            assert_eq!(response.status(), StatusCode::OK);
            response
                .headers()
                .get(&X_REQUEST_ID)
                .expect("request id header")
                .to_str()
                .expect("ascii id")
                .to_string()
        }
    };
    let printable_64 = "i".repeat(REQUEST_ID_MAX_CHARS);
    assert_eq!(
        echoed_id(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .header("x-request-id", &printable_64)
        )
        .await,
        printable_64,
        "a printable id at the cap still echoes verbatim"
    );
    let oversized = "a".repeat(REQUEST_ID_MAX_CHARS + 1);
    let rejected_ids = [&oversized[..], "has space", "tab\there"];
    for rejected in &rejected_ids {
        let echoed = echoed_id(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .header("x-request-id", *rejected),
        )
        .await;
        assert!(
            echoed.starts_with("req-") && echoed != *rejected,
            "a {rejected:?} id must regenerate, not echo or truncate into an \
                 id-shaped echo: {echoed:?}"
        );
    }
    let lines = session.stop();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("request complete") && line.contains(&printable_64)),
        "the accepted id is the id in the ring line: {lines:?}"
    );
    for rejected in &rejected_ids {
        assert!(
            lines.iter().all(|line| !line.contains(*rejected)),
            "the rejected id {rejected:?} must never reach the ring: {lines:?}"
        );
    }
}
