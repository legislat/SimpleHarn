//! The proxy and the gate on the wire: pass-through and query fidelity,
//! engine-id rewrites, crash-retry, no-model fallback, upstream failure
//! classes, and the admission refusals, wait hints, parks, and eviction
//! rows the scheduler produces.

use super::*;
use axum::http::Method;
use simpleharn::api::classify_send_failure;
use simpleharn::config::CategoryConfig;

#[tokio::test]
async fn proxy_forwards_with_model_rewrite_and_streams_the_response() {
    let (control, work, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(runtime.server_count(), 1);

    let (status, body) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"QWEN3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["echo"], true);
    assert_eq!(body["path"], "/v1/chat/completions");
    // Alias resolved to the harness name, then rewritten to the engine's
    // self-reported id (F-4: the mock reports `mock-model`).
    assert_eq!(body["body"]["model"], "mock-model");

    // FINDINGS-18 [4]: the "streams" half used to assert nothing. The proxy
    // contract (src/api/mod.rs `proxy`: upstream headers copied + the body handed
    // through `Body::from_stream`) is byte-for-byte pass-through — a regression
    // that buffered and re-serialised the JSON, or dropped the upstream
    // `content-type`, passed the assertions above. The mock answers a
    // `"stream": true` request with a fixed SSE literal; this asks for exactly
    // that literal and demands the content-type and the bytes come back
    // untouched.
    let response = work
        .clone()
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"QWEN3","stream":true}"#))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "the upstream content-type must survive the pass-through, got: {content_type}"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    assert_eq!(
        bytes.as_ref(),
        mock_runtime::SSE_BODY.as_bytes(),
        "the streamed body must arrive byte-for-byte, not re-serialised"
    );
}

#[tokio::test]
async fn polling_an_empty_slot_never_loads_a_model() {
    // "Reads never wake" on the REAL router (operator ruling 2026-11-04,
    // Variant A pins the status quo): `GET /v1/models` and `GET /health` answer
    // from the catalog and static state and never admit, so the scheduler stays
    // untouched and the empty slot stays empty. Falsifiers: (1) wire any read
    // route to `proxy` and the polling legs below go red — a ledger row appears
    // and an engine boots while nothing was asked to load; (2) stop the
    // inference door from waking and the positive-control leg below goes red;
    // (3) a fourth waking path added to `wakes_slot` without the route table
    // fails the src-side pin `the_wake_set_is_exactly_the_proxy_routes`.
    let (control, work, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );

    // Baseline before the first polling round (B4R-02): the ledger must start
    // empty, so a boot-time mint fails here with its own honest message
    // instead of being misattributed to the polling legs below.
    let (status, baseline) = get_json(&control, "/har/jobs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        baseline["jobs"].as_array().map(Vec::len),
        Some(0),
        "pre-poll baseline: before the first polling round the ledger must already be empty: {baseline}"
    );

    // Poll like a status page does — catalog, health, and the method variants a
    // curious client can aim at the inference paths. None of them may admit.
    for round in 0..3 {
        let (status, body) = get_json(&work, "/v1/models").await;
        assert_eq!(status, StatusCode::OK, "round {round}: {body}");
        assert_eq!(body["data"].as_array().map(Vec::len), Some(1));

        let (status, _) = get_json(&work, "/health").await;
        assert_eq!(status, StatusCode::OK, "round {round} health");

        for method in [Method::HEAD, Method::OPTIONS] {
            for path in ["/v1/chat/completions", "/v1/completions", "/v1/embeddings"] {
                let response = work
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(path)
                            .header("host", "127.0.0.1:8708")
                            .body(Body::empty())
                            .expect("static request"),
                    )
                    .await
                    .expect("in-process request");
                assert_eq!(
                    response.status(),
                    StatusCode::METHOD_NOT_ALLOWED,
                    "round {round}: {method} {path} must be refused by routing, before any ensure"
                );
                let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("bounded body");
            }
        }

        let (status, body) = get_json(&control, "/har/jobs").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["jobs"].as_array().map(Vec::len),
            Some(0),
            "round {round}: polling must not mint a job: {body}"
        );
        let (_, doc) = get_json(&control, "/har/status").await;
        let work_slot = doc["slots"]
            .as_array()
            .expect("slots array")
            .iter()
            .find(|slot| slot["category"] == "WORK")
            .expect("WORK slot present");
        assert_eq!(work_slot["state"], "empty", "round {round}: {doc}");
        assert!(
            work_slot["model"].is_null(),
            "round {round}: the empty slot must stay empty: {doc}"
        );
    }
    assert_eq!(runtime.server_count(), 0, "no poll may boot an engine");

    // Positive control: the inference door DOES wake — the same empty slot
    // takes a load the moment a real inference POST arrives.
    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let (status, body) = get_json(&control, "/har/jobs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["jobs"].as_array().map(Vec::len),
        Some(1),
        "the inference POST must mint exactly one ledger row: {body}"
    );
    assert_eq!(body["jobs"][0]["state"], "Serving", "body: {body}");
    assert_eq!(runtime.server_count(), 1, "the wake loaded the model");
}

#[tokio::test]
async fn proxy_forwards_the_query_string_the_client_sent() {
    // FINDINGS-06 [10]: the forward used `uri.path()`, which drops the query, so a
    // client sending `?api-key=` or an engine-specific flag got a DIFFERENT request
    // at the engine than the one it sent. The mock echoes the exact request target
    // it received, so the assertion is about the wire, not about intent.
    let (control, work, _, _, _runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;

    let (status, body) = post_json(
        &work,
        "/v1/chat/completions?a=1&b=2",
        r#"{"model":"qwen3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["path"], "/v1/chat/completions?a=1&b=2",
        "the query string is part of the request the client sent"
    );

    // An empty query is still a query: the bare `?` must survive the join.
    let (status, body) = post_json(
        &work,
        "/v1/chat/completions?",
        r#"{"model":"qwen3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["path"], "/v1/chat/completions?");

    // No query still forwards cleanly and byte-identically to the pinned target
    // of `proxy_forwards_with_model_rewrite_and_streams_the_response` — the fix
    // must not introduce a `?` or a double slash where the client sent none.
    let (status, body) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"qwen3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["path"], "/v1/chat/completions");
}

#[tokio::test]
async fn proxy_passes_the_engines_content_encoding_through_verbatim() {
    // FINDINGS-06 [9]: the strip list dropped `content-encoding` from a body the
    // proxy never decodes (reqwest is built without gzip/brotli/deflate/zstd), so
    // an engine that answers compressed handed the client gzip bytes labelled as
    // identity — undecodable. The pass-through is honest now: header and bytes
    // cross unchanged. Request-direction pass-through is structurally impossible
    // and not claimed: the proxy parses, rewrites and re-serialises the JSON body
    // and forwards no client headers (FINDINGS-06 "verified clean").
    let (control, work, _, _, _runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;

    let response = work
        .clone()
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"qwen3","mock_encoded_reply":true}"#))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-encoding")
            .and_then(|value| value.to_str().ok()),
        Some("gzip"),
        "an undecoded body keeps the encoding the engine declared for it"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    assert_eq!(
        bytes.as_ref(),
        mock_runtime::ENCODED_BODY,
        "the opaque encoded body must arrive byte-for-byte, not decoded or re-framed"
    );
}

#[tokio::test]
async fn proxy_strips_the_engines_hop_by_hop_response_headers() {
    // R2-06-02: the engine→client pass-through used to drop only
    // `transfer-encoding`, `content-length` and `connection`, forwarding the rest
    // of the RFC 9110 §7.6.1 hop-by-hop set: a survived `trailer:` advertises a
    // trailer this harness never re-frames, `upgrade:` a protocol switch it never
    // relays, and a header named by the (dropped) `connection` value is forwarded
    // while the header that made it hop-by-hop is not — a pair that contradicts
    // itself. The strip list now covers all of it. Falsifier: revert to the
    // three-name `matches!` arm and every absent-header leg below fails with the
    // engine's value present.
    let (control, work, _, _, _runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;

    let response = work
        .clone()
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"qwen3","mock_hop_by_hop":true}"#))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::OK);
    for hop_by_hop in [
        "connection",
        "keep-alive",
        "upgrade",
        "te",
        "trailer",
        "proxy-authenticate",
        "proxy-authorization",
        // Enumerated by the engine's `connection: keep-alive, X-Hop-Named` value:
        // the `connection` header that makes a name hop-by-hop never crosses, so
        // neither may the names it enumerates.
        "x-hop-named",
    ] {
        assert!(
            response.headers().get(hop_by_hop).is_none(),
            "`{hop_by_hop}` belongs to the engine↔harness hop and may not reach \
             the client: {:?}",
            response.headers()
        );
    }
    assert_eq!(
        response
            .headers()
            .get("x-engine-marker")
            .and_then(|value| value.to_str().ok()),
        Some("kept"),
        "an ordinary end-to-end header must still pass through"
    );
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json"),
        "the body-describing header survives (FINDINGS-06 [9] posture unchanged)"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        r#"{"hop_by_hop":true}"#,
        "stripping headers must not touch the body"
    );
}

#[tokio::test]
async fn proxy_rewrites_each_occupant_to_its_own_engine_id() {
    // FINDINGS-20 [6]: every mock engine used to report the one constant `mock-model`
    // at `/v1/models`, so every serve_id assertion in the suite compared the same
    // string and a product bug that forwarded slot A's id for slot B's request — the
    // crash-retry staleness class fixed in b540c30 — was invisible. These two engines
    // report DISTINCT ids; each request must be rewritten to ITS OWN engine's id.
    let adapters: Vec<(&str, Arc<dyn EngineAdapter>)> = vec![
        (
            "mock-id-a",
            Arc::new(mock_runtime::MockIdAdapter {
                engine: "mock-id-a",
                serve_id: "engine-alpha",
            }),
        ),
        (
            "mock-id-b",
            Arc::new(mock_runtime::MockIdAdapter {
                engine: "mock-id-b",
                serve_id: "engine-beta",
            }),
        ),
    ];
    let mut config = AppConfig::default();
    config
        .category_mut(CategoryId::Aux)
        .expect("default config has AUX")
        .max_models = 2;
    let (control, _, _, aux, runtime, _modelconf) = build_app(
        config,
        &[
            ("alpha", "mock-id-a", CategoryId::Aux, Some(3.0)),
            ("beta", "mock-id-b", CategoryId::Aux, Some(3.0)),
        ],
        &adapters,
    );
    for name in ["alpha", "beta"] {
        let body = format!(r#"{{"model":"{name}"}}"#);
        let (status, reply) = post_json(&control, "/har/load", &body).await;
        assert_eq!(status, StatusCode::OK, "load {name}: {reply}");
    }
    assert_eq!(runtime.server_count(), 2, "both occupants serve");

    for (model, expect_id) in [("alpha", "engine-alpha"), ("beta", "engine-beta")] {
        let body = format!(r#"{{"model":"{model}","messages":[]}}"#);
        let (status, reply) = post_json(&aux, "/v1/chat/completions", &body).await;
        assert_eq!(status, StatusCode::OK, "proxy {model}: {reply}");
        assert_eq!(
            reply["body"]["model"].as_str(),
            Some(expect_id),
            "{model}'s request must carry {model}'s OWN engine id — a cross-slot \
             serve_id mix-up is exactly what one shared mock id hid: {reply}"
        );
    }
}

#[tokio::test]
async fn a_silent_engine_falls_back_to_the_harness_name_on_the_wire() {
    // FINDINGS-20 [6], the `serve_id == None` half: when an engine answers nothing at
    // `/v1/models`, discovery yields `None` and the proxy forwards the HARNESS NAME
    // (documented `serve_id.unwrap_or_else` fallback at src/api/mod.rs) — previously
    // untestable because no mock mode could make discovery fail. The mock
    // `mock-nodiscovery` 404s `/v1/models`; the rewritten `model` field must be the
    // harness name the operator gave, never an invented engine id.
    let (control, work, _, _, _runtime, _modelconf) = app(
        AppConfig::default(),
        &[("quiet", "mock-nodiscovery", CategoryId::Work, Some(4.0))],
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"quiet"}"#).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a silent engine still loads: {body}"
    );

    let (status, reply) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"quiet","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "proxy: {reply}");
    assert_eq!(
        reply["body"]["model"].as_str(),
        Some("quiet"),
        "no discovered id means the documented harness-name fallback on the wire: {reply}"
    );
}

#[tokio::test]
async fn crash_retry_re_resolves_by_name_after_the_id_rewrite() {
    // T10.1 regression (T-suite Run 4, 2026-10-27). `proxy` rewrites `body["model"]`
    // to the engine's self-reported id - `mock-model` here, the gguf PATH for llama -
    // before forwarding. The crash-retry then re-resolved the target from that
    // rewritten body, so it searched the catalog for a model *named* by engine id and
    // 404'd: the restart succeeded, the request still failed.
    let (control, work, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock-dies-once", CategoryId::Work, Some(4.0))],
    );
    let (load_status, load_body) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(
        load_status,
        StatusCode::OK,
        "the first load must succeed before anything is killed: {load_body}"
    );
    assert_eq!(runtime.spawn_count(), 1, "one engine started");
    let (_, loaded) = get_json(&control, "/har/status").await;
    let port = u16::try_from(
        loaded["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["model"] == "qwen3")
            .and_then(|slot| slot["port"].as_u64())
            .expect("the serving slot reports its engine port"),
    )
    .expect("port fits u16");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !runtime.is_dead(port).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(runtime.is_dead(port).await, "the engine must really die");

    let (status, body) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"qwen3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "crash-retry must answer: {body}");
    assert_eq!(body["echo"], true, "the restarted engine answered");
    assert_eq!(
        body["body"]["model"], "mock-model",
        "the retried request still carries the engine's own id (F-4)"
    );
    assert_eq!(runtime.spawn_count(), 2, "exactly one restart");
    assert_eq!(
        runtime.server_count(),
        1,
        "the corpse is gone, one live engine"
    );
}

#[tokio::test]
async fn crash_retry_re_sends_the_successor_discovered_id() {
    // R2-18-04: the leg above cannot tell the NEW engine's id from the DEAD
    // engine's - both `dies-once` generations used to answer the same constant
    // `mock-model` - so a retry that kept the pre-restart `serve_id` instead of
    // re-deriving it (src/api/mod.rs `ensure_named` → `new_serve_id`) stayed
    // green, the FINDINGS-20 [6] shared-id blindness surviving in exactly this
    // test. Here the corpse reports `engine-gen1` and the successor `engine-gen2`
    // (the mock's per-spawn id sequence, R2-18-04's `MockGenerationIdAdapter`):
    // forwarding the corpse's id now goes RED, and the retried wire must carry
    // the SUCCESSOR's self-reported id.
    let adapters: Vec<(&str, Arc<dyn EngineAdapter>)> = vec![(
        "mock-dies-once",
        Arc::new(mock_runtime::MockGenerationIdAdapter {
            engine: "mock-dies-once",
            ids: "engine-gen1|engine-gen2",
        }),
    )];
    let (control, work, _, _, runtime, _modelconf) = build_app(
        AppConfig::default(),
        &[("qwen3", "mock-dies-once", CategoryId::Work, Some(4.0))],
        &adapters,
    );
    let (load_status, load_body) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(
        load_status,
        StatusCode::OK,
        "the first load must succeed before anything is killed: {load_body}"
    );
    assert_eq!(runtime.spawn_count(), 1, "one engine started");
    let (_, loaded) = get_json(&control, "/har/status").await;
    let port = u16::try_from(
        loaded["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["model"] == "qwen3")
            .and_then(|slot| slot["port"].as_u64())
            .expect("the serving slot reports its engine port"),
    )
    .expect("port fits u16");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !runtime.is_dead(port).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(runtime.is_dead(port).await, "the engine must really die");

    let (status, body) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"qwen3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "crash-retry must answer: {body}");
    assert_eq!(body["echo"], true, "the restarted engine answered");
    assert_eq!(
        body["body"]["model"].as_str(),
        Some("engine-gen2"),
        "the retried request must carry the SUCCESSOR's discovered id; `engine-gen1` \
         here is the dead engine's id re-sent - the exact R2-18-04 regression: {body}"
    );
    assert_eq!(runtime.spawn_count(), 2, "exactly one restart");
    assert_eq!(runtime.server_count(), 1, "the corpse is gone");
}

#[tokio::test]
async fn crash_retry_falls_back_to_the_harness_name_when_the_successor_is_silent() {
    // R2-18-04, the other half: when the successor discovers NOTHING at
    // `/v1/models`, the retried request must carry the documented harness-name
    // fallback (the same `serve_id.unwrap_or_else` the first forward uses) - never
    // the DEAD engine's id. The corpse therefore reports a real id
    // (`engine-gen1`) that differs from the name: a retry reusing the first
    // attempt's `serve_id` sends `engine-gen1` and goes RED here, which is exactly
    // the harm the shared-constant id could never show.
    let adapters: Vec<(&str, Arc<dyn EngineAdapter>)> = vec![(
        "mock-dies-once",
        Arc::new(mock_runtime::MockGenerationIdAdapter {
            engine: "mock-dies-once",
            ids: "engine-gen1|none",
        }),
    )];
    let (control, work, _, _, runtime, _modelconf) = build_app(
        AppConfig::default(),
        &[("qwen3", "mock-dies-once", CategoryId::Work, Some(4.0))],
        &adapters,
    );
    let (load_status, load_body) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(
        load_status,
        StatusCode::OK,
        "a silent-later engine loads fine now: {load_body}"
    );
    assert_eq!(runtime.spawn_count(), 1, "one engine started");
    let (_, loaded) = get_json(&control, "/har/status").await;
    let port = u16::try_from(
        loaded["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["model"] == "qwen3")
            .and_then(|slot| slot["port"].as_u64())
            .expect("the serving slot reports its engine port"),
    )
    .expect("port fits u16");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !runtime.is_dead(port).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(runtime.is_dead(port).await, "the engine must really die");

    let (status, body) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"qwen3","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "crash-retry must answer: {body}");
    assert_eq!(body["echo"], true, "the restarted engine answered");
    assert_eq!(
        body["body"]["model"].as_str(),
        Some("qwen3"),
        "a successor that discovers nothing means the HARNESS NAME on the retried \
         wire; `engine-gen1` here forwards the dead engine's id over a live engine - \
         the R2-18-04 regression: {body}"
    );
    assert_eq!(runtime.spawn_count(), 2, "exactly one restart");
    assert_eq!(runtime.server_count(), 1, "the corpse is gone");
}

#[tokio::test]
async fn no_model_fallback_serves_the_loaded_occupant() {
    let (control, work, _, _, _, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;

    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["body"]["model"], "mock-model",
        "fallback serves the loaded model (rewritten to its engine id)"
    );
}

#[tokio::test]
async fn no_model_fallback_loads_the_category_default() {
    // R2-18-05: with one RP entry the old fixture could not tell "loads the
    // configured default_model" from "loads the only entry of the category". The
    // decoy carries a distinct name AND a distinct engine-reported id, so a
    // fallback that ignored the default's value (while keeping the
    // default-existence gate) serves `engine-decoy` and goes red below.
    let mut config = AppConfig::default();
    config
        .category_mut(CategoryId::Rp)
        .expect("default config has RP")
        .default_model = Some("default-model".to_string());
    let adapters: Vec<(&str, Arc<dyn EngineAdapter>)> = vec![
        (
            "mock-id-default",
            Arc::new(mock_runtime::MockIdAdapter {
                engine: "mock-id-default",
                serve_id: "engine-default",
            }),
        ),
        (
            "mock-id-decoy",
            Arc::new(mock_runtime::MockIdAdapter {
                engine: "mock-id-decoy",
                serve_id: "engine-decoy",
            }),
        ),
    ];
    let (control, _, rp, _, runtime, _modelconf) = build_app(
        config,
        &[
            (
                "default-model",
                "mock-id-default",
                CategoryId::Rp,
                Some(4.0),
            ),
            ("decoy-rp", "mock-id-decoy", CategoryId::Rp, Some(4.0)),
        ],
        &adapters,
    );

    let (status, body) = post_json(&rp, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["body"]["model"].as_str(),
        Some("engine-default"),
        "the no-model request must reach the CONFIGURED default's engine, never \
         the decoy's: {body}"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "the default model was loaded on demand"
    );

    let (_, status) = get_json(&control, "/har/status").await;
    let rp_slot = status["slots"]
        .as_array()
        .expect("slots array")
        .iter()
        .find(|slot| slot["category"] == "RP")
        .expect("RP slot present");
    assert_eq!(
        rp_slot["model"], "default-model",
        "the loaded row is the configured default, not the decoy: {status}"
    );
}

#[tokio::test]
async fn no_model_without_default_is_400_naming_the_listener() {
    let (_, work, _, _, _, _) = app(AppConfig::default(), &[]);
    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("WORK") && message.contains("default_model"),
        "was: {message}"
    );
}

#[tokio::test]
async fn admission_refusal_via_proxy_is_503_with_numbers() {
    let (control, _, rp, _, _, _) = app(
        AppConfig::default(),
        &[
            ("big-a", "mock", CategoryId::Work, Some(50.0)),
            ("big-b", "mock", CategoryId::Rp, Some(50.0)),
        ],
    );
    post_json(&control, "/har/load", r#"{"model":"big-a"}"#).await;

    let (status, body) = post_json(&rp, "/v1/chat/completions", r#"{"model":"big-b"}"#).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("need 50.0 GiB") && message.contains("free 20.0 GiB"),
        "was: {message}"
    );
}

#[tokio::test]
async fn last_error_is_the_newest_failed_load_and_survives_unrelated_success() {
    // A3 (API wiring): a genuine engine failure pins `last_error` (502, names the
    // model + readiness); a newer failure takes the headline; an unrelated
    // successful load of a DIFFERENT model does not mask a real fault (finding 2).
    // The pure derivation rules (same-model clear, cancel/unload skip) are pinned
    // by the `headline_fault` unit test in src/scheduler/mod.rs.
    let config = AppConfig {
        load_timeout_s: 2,
        ..AppConfig::default()
    };
    let (control, _, _, _, _, _) = app(
        config,
        &[
            ("bad", "mock-never", CategoryId::Work, Some(4.0)),
            ("bad2", "mock-never", CategoryId::Work, Some(4.0)),
            ("good", "mock", CategoryId::Work, Some(4.0)),
        ],
    );

    let (status, body) = post_json(&control, "/har/load", r#"{"model":"bad"}"#).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body: {body}");
    let (_, doc) = get_json(&control, "/har/status").await;
    let first = doc["last_error"].as_str().unwrap_or_default().to_string();
    assert!(
        first.contains("bad@WORK") && first.contains("ready"),
        "the failed load pins last_error: {first}"
    );

    let (status, _) = post_json(&control, "/har/load", r#"{"model":"bad2"}"#).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "second genuine failure");
    let (_, doc) = get_json(&control, "/har/status").await;
    let newest = doc["last_error"].as_str().unwrap_or_default();
    assert!(
        newest.contains("bad2@WORK"),
        "newest failure wins: {newest}"
    );

    let (status, _) = post_json(&control, "/har/load", r#"{"model":"good"}"#).await;
    assert_eq!(status, StatusCode::OK, "the good model loads");
    let (_, doc) = get_json(&control, "/har/status").await;
    assert!(
        doc["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("bad2@WORK"),
        "an unrelated success must not mask a real fault: {:?}",
        doc["last_error"]
    );
}

#[tokio::test]
async fn below_launch_floor_gets_its_own_error_code() {
    // 503 with a distinct code: clients can tell "card busy with foreign work,
    // refused before spawn" (greedy guard) from plain admission over-commit.
    let config = AppConfig {
        vram_total_gb: 10.0,
        vram_reserve_gb: 2.0,
        load_timeout_s: 5,
        ..AppConfig::default()
    };
    let (control, _, _, _, _, dir) =
        app(config, &[("filler", "mock", CategoryId::Work, Some(4.0))]);
    std::fs::write(
        dir.join("guarded.yaml"),
        "name: guarded\nengine: mock\ncategory: RP\nvram_gb: 2\n\
         launch_only_with_at_least_vram_free: 40\n",
    )
    .expect("seed guarded model");
    let (status, _) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post_json(&control, "/har/load", r#"{"model":"guarded"}"#).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    assert_eq!(body["code"], "below_launch_floor");
    assert_eq!(body["error"]["code"], "below_launch_floor");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("launch floor"),
        "message explains the floor: {body}"
    );
}

#[tokio::test]
async fn a_first_refusal_names_no_estimate_and_no_place() {
    // The honesty floor for the wait hint: a refusal before ANY driver-confirmed
    // release omits `expected_release_s` (falsifier: fabricate a default 0.0
    // estimate in `error_body` and this leg goes red — the harness must stay
    // silent when it has no basis), and a refusal with nothing ahead states
    // `queue_position: null` instead of quoting a place it did not grant
    // (falsifier: drop the no-ahead guard in `ActorState::wait_hint` and the
    // null leg goes red). The message still carries every number exactly as
    // before. 90 GiB over a 70 GiB capacity, with nothing loaded, refuses.
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[("huge-a", "mock", CategoryId::Work, Some(90.0))],
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"huge-a"}"#).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    assert_eq!(body["code"], "unavailable");
    assert_eq!(body["error"]["code"], "unavailable");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("need 90.0 GiB"),
        "the numbers must still ride the message: {body}"
    );
    assert!(
        body.get("expected_release_s").is_none(),
        "no confirmed release yet: the estimate must be absent, not invented: {body}"
    );
    assert_eq!(
        body.get("queue_position"),
        Some(&Value::Null),
        "nothing is ahead and this refusal got no place: {body}"
    );
}

#[tokio::test]
async fn a_confirmed_release_lands_the_estimate_and_a_place_on_the_next_refusal() {
    // The full path through the real actor: a REAL unload runs to a
    // driver-confirmed release (`mock` engine; the WORK slot reads `empty` only
    // once the confirmation lands — the occupant clears at the confirmed
    // release, never at the acceptance), so the ring holds a measured sample.
    // A load that never becomes ready (`mock-never`, in-flight and non-terminal
    // until its 120 s deadline) then stands ahead in the refused category, and
    // the refusal must quote the estimate and the documented place.
    let (control, _, _, _, _, _) = app(
        aux_slots(3),
        &[
            ("rel", "mock", CategoryId::Work, Some(4.0)),
            ("aux-a", "mock", CategoryId::Aux, Some(30.0)),
            ("aux-slow", "mock-never", CategoryId::Aux, Some(30.0)),
            ("aux-over", "mock", CategoryId::Aux, Some(60.0)),
        ],
    );

    // Leg 1: a real unload, confirmed. `/har/unload` 200 is an acceptance, so
    // wait for the slot to actually empty (the occupant clears on the
    // confirmation) — that confirmation is the measured release the median runs
    // on. Falsifier: never feed the ring in the `Released(Ok)` arm and the
    // estimate leg at the end goes red.
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"rel"}"#).await;
    assert_eq!(status, StatusCode::OK, "load rel: {body}");
    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"rel"}"#).await;
    assert_eq!(status, StatusCode::OK, "unload accepted: {body}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, status_doc) = get_json(&control, "/har/status").await;
        let work = status_doc["slots"]
            .as_array()
            .expect("slots array")
            .iter()
            .find(|slot| slot["category"] == "WORK")
            .expect("WORK slot present");
        if work["state"] == "empty" && work["model"].is_null() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the rel release never reached the confirmed window: {status_doc}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Leg 2: one AUX occupant booked, one AUX load in flight and never ready.
    // Its POST parks on the load's reply (readiness never lands), so it runs on
    // a task while the refusal is judged.
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"aux-a"}"#).await;
    assert_eq!(status, StatusCode::OK, "load aux-a: {body}");
    let slow_post = {
        let control = control.clone();
        tokio::spawn(
            async move { post_json(&control, "/har/load", r#"{"model":"aux-slow"}"#).await },
        )
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, jobs) = get_json(&control, "/har/jobs").await;
        let flying = jobs["jobs"]
            .as_array()
            .expect("jobs array")
            .iter()
            .any(|job| job["model"] == "aux-slow" && job["terminal"] == false);
        if flying {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the never-ready AUX load never became an in-flight row: {jobs}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Leg 3: the refusal. `aux-over` (60 GiB) arms on the third, empty AUX
    // slot; the ledger already holds aux-a plus the in-flight debit for
    // aux-slow, so the card holds 10 free and the 50 GiB deficit is bigger
    // than every in-flight booking (30) - a refusal the 2026-11-10 ruling
    // leaves intact (a booking-only deficit would park instead, see
    // `a_gate_short_load_waits_at_the_api_behind_an_in_flight_booking_and_
    // admits`), and nothing same-category is evictable, so the gate refuses
    // in `finish_admit` - with the hint born beside the numbers. Falsifier
    // (run in place, revert): drop `aux-over` back to 20 GiB and this leg
    // goes red - the row parks on the booking instead of refusing.
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"aux-over"}"#).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    assert_eq!(body["code"], "unavailable");
    assert_eq!(
        body["error"]["code"], "unavailable",
        "the OpenAI envelope keeps today's shape"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("need 60.0 GiB"),
        "the message keeps the numbers: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("blocked by: aux-a"),
        "the named holders still ride the tail: {body}"
    );
    let estimate = body["expected_release_s"]
        .as_f64()
        .expect("the confirmed rel release must land a measurable estimate");
    assert!(
        (0.0..30.0).contains(&estimate),
        "a mock release is seconds, not geological time: {estimate}"
    );
    assert_eq!(
        body["queue_position"], 2,
        "the in-flight aux-slow load is ahead in AUX: this refusal sits at place 2"
    );

    // Teardown honestly: cancelling the never-ready swap releases its engine
    // and resolves the parked POST (409 cancelled) before the test ends.
    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"aux-slow"}"#).await;
    assert_eq!(status, StatusCode::OK, "cancel accepted: {body}");
    let (slow_status, slow_body) = slow_post.await.expect("slow task joined");
    assert_eq!(
        slow_status,
        StatusCode::CONFLICT,
        "the cancelled swap answers conflict, not a receipt: {slow_body}"
    );
}

#[tokio::test]
async fn a_gate_short_load_waits_at_the_api_behind_an_in_flight_booking_and_admits() {
    // Operator ruling 2026-11-10 at the wire: a load whose VRAM deficit is
    // covered only by another load's paper booking must wait, not refuse.
    // `aux-a` holds 30 and `aux-slow` (never-ready mock) books 30, so
    // `aux-over` (20 GiB) is 10 short with 30 of booking ahead: the POST
    // parks the row open instead of answering 503, `/har/jobs` shows the row
    // alive (a refusal would have terminalized it), and cancelling the
    // in-flight row releases the booking so the terminal funnel re-gates the
    // waiting row and the mock load lands a Loaded receipt. Falsifier (run in
    // place, revert): neuter the park arm in `finish_admit` (return it to a
    // plain refusal) and the 200 leg goes red with the old 503 body.
    let (control, _, _, _, _, _) = app(
        aux_slots(3),
        &[
            ("aux-a", "mock", CategoryId::Aux, Some(30.0)),
            ("aux-slow", "mock-never", CategoryId::Aux, Some(30.0)),
            ("aux-over", "mock", CategoryId::Aux, Some(20.0)),
        ],
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"aux-a"}"#).await;
    assert_eq!(status, StatusCode::OK, "load aux-a: {body}");
    let slow_post = {
        let control = control.clone();
        tokio::spawn(
            async move { post_json(&control, "/har/load", r#"{"model":"aux-slow"}"#).await },
        )
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, jobs) = get_json(&control, "/har/jobs").await;
        let flying = jobs["jobs"]
            .as_array()
            .expect("jobs array")
            .iter()
            .any(|job| job["model"] == "aux-slow" && job["terminal"] == false);
        if flying {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the never-ready AUX load never became an in-flight row: {jobs}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // The deficit is paper-only, so this POST parks: the row rides the ledger
    // alive, which a named refusal never leaves behind.
    let over_post = {
        let control = control.clone();
        tokio::spawn(
            async move { post_json(&control, "/har/load", r#"{"model":"aux-over"}"#).await },
        )
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, jobs) = get_json(&control, "/har/jobs").await;
        let waiting = jobs["jobs"]
            .as_array()
            .expect("jobs array")
            .iter()
            .any(|job| job["model"] == "aux-over" && job["terminal"] == false);
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the gate-short load never stayed alive on the ledger: {jobs}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Cancelling the in-flight row terminalizes its booking; the terminal
    // funnel re-gates the waiting row, and the fresh gate sees the card free.
    let (status, body) = post_json(&control, "/har/unload", r#"{"model":"aux-slow"}"#).await;
    assert_eq!(status, StatusCode::OK, "cancel accepted: {body}");
    let (over_status, over_body) =
        tokio::time::timeout(std::time::Duration::from_secs(30), over_post)
            .await
            .expect("the parked load must admit once the booking ahead terminalizes")
            .expect("over task joined");
    assert_eq!(
        over_status,
        StatusCode::OK,
        "the wait ends loaded: {over_body}"
    );
    assert_eq!(
        over_body["status"], "loaded",
        "receipt shape kept: {over_body}"
    );

    // Teardown honestly: cancelling the never-ready swap releases its engine
    // and resolves the parked POST (409 cancelled) before the test ends.
    let (slow_status, slow_body) = slow_post.await.expect("slow task joined");
    assert_eq!(
        slow_status,
        StatusCode::CONFLICT,
        "the cancelled swap answers conflict, not a receipt: {slow_body}"
    );
}

#[tokio::test]
async fn the_wire_reports_an_evicted_victims_row_not_serving_once_the_release_lands() {
    // EVID-01 at the wire (the live proof's exact shape: slot empty while the
    // evicted model's own load row kept `Serving`/`loaded`): after a cross-
    // category eviction commits, `GET /har/jobs` and the `jobs` array of
    // `/har/status` must show the victim row terminal and NOT Serving, with the
    // honest `evicted` word, while the incoming row serves. The victim's kill
    // is held BY TARGET (`hold_kill_of`, the FINDINGS-19 [4] seam) so the
    // pre-commit window — row still `Serving`, slot `releasing`, engine alive —
    // is an observed state, not a race, and the flip is one commit.
    // Falsifier (run in place, revert): drop the `terminalize_victim_load_row`
    // call from `commit_victim_release` in `src/scheduler/occupant.rs` — the
    // slot legs below stay green (the pre-fix half already worked) and the
    // victim-row legs on both surfaces go red with `Serving`/`loaded`.
    let adapters: Vec<(&str, Arc<dyn EngineAdapter>)> =
        vec![("mock-hold-kill", Arc::new(MockAdapter))];
    let (control, _, _, _, runtime, _) = build_app(
        aux_slots(1),
        &[
            ("aux30", "mock-hold-kill", CategoryId::Aux, Some(30.0)),
            ("work50", "mock", CategoryId::Work, Some(50.0)),
        ],
        &adapters,
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"aux30"}"#).await;
    assert_eq!(status, StatusCode::OK, "load aux30: {body}");
    let (_, loaded) = get_json(&control, "/har/status").await;
    let aux_port = u16::try_from(
        loaded["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["model"] == "aux30")
            .and_then(|slot| slot["port"].as_u64())
            .expect("the loaded AUX reports its engine port"),
    )
    .expect("port fits u16");
    runtime.hold_kill_of(aux_port);

    // Card 72, reserve 2, ledger-only: AUX holds 30, WORK wants 50 — the gate
    // must evict the AUX victim (evictable by WORK) before it spawns.
    let evicting = {
        let control = control.clone();
        tokio::spawn(async move { post_json(&control, "/har/load", r#"{"model":"work50"}"#).await })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (_, status_doc) = get_json(&control, "/har/status").await;
        let releasing = status_doc["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .any(|slot| slot["model"] == "aux30" && slot["state"] == "releasing");
        let jobs = get_json(&control, "/har/jobs").await.1;
        let still_serving = jobs["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .any(|job| job["model"] == "aux30" && job["state"] == "Serving");
        if releasing && still_serving {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the parked eviction never made the victim row observable as serving-\
             while-releasing (EVID-01 falsifier inverts this into a permanent red): \
             status {status_doc}, jobs {jobs}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Land the held release: the commit point for BOTH surfaces at once.
    runtime.release_held_kills();
    let (status, body) = evicting.await.expect("evict task joined");
    assert_eq!(
        status,
        StatusCode::OK,
        "the eviction admits after the release: {body}"
    );

    let (_, jobs) = get_json(&control, "/har/jobs").await;
    let victim_row = jobs["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .rev()
        .find(|job| job["model"] == "aux30")
        .expect("the victim keeps its ledger row");
    assert_ne!(
        victim_row["state"], "Serving",
        "the evicted victim's row must not claim a gone engine still serves: {victim_row}"
    );
    assert_eq!(
        victim_row["state"], "Idle",
        "the terminal-unloaded state: {victim_row}"
    );
    assert_eq!(
        victim_row["terminal"],
        serde_json::json!(true),
        "{victim_row}"
    );
    assert_eq!(
        victim_row["outcome"], "evicted",
        "the eviction names itself honestly on the wire: {victim_row}"
    );
    assert!(
        victim_row["error"].is_null() && victim_row["is_fault"] == serde_json::json!(false),
        "an eviction is not a fault: {victim_row}"
    );
    let incoming_row = jobs["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .rev()
        .find(|job| job["model"] == "work50")
        .expect("the incoming row is on the ledger");
    assert_eq!(incoming_row["state"], "Serving", "{incoming_row}");
    assert_eq!(incoming_row["outcome"], "loaded", "{incoming_row}");

    // `/har/status.jobs` is the second surface EVID-01 caught reading stale —
    // same actor snapshot, one truth, no second derivation allowed.
    let (_, status_doc) = get_json(&control, "/har/status").await;
    let status_victim = status_doc["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .rev()
        .find(|job| job["model"] == "aux30")
        .expect("the victim row rides /har/status too");
    assert_ne!(
        status_victim["state"], "Serving",
        "/har/status.jobs must agree with /har/jobs: {status_victim}"
    );
    assert_eq!(status_victim["outcome"], "evicted", "{status_victim}");

    // Slot and ledger agree (pin b at the wire): the AUX slot is empty exactly
    // when no Serving row names the victim, and the engine is really gone.
    let aux_slot = status_doc["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .find(|slot| slot["category"] == "AUX")
        .expect("AUX slot");
    assert!(
        aux_slot["model"].is_null(),
        "the victim's slot emptied: {aux_slot}"
    );
    assert_eq!(aux_slot["state"], "empty", "{aux_slot}");
    let serving_rows: Vec<&str> = jobs["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .filter(|job| job["state"] == "Serving")
        .filter_map(|job| job["model"].as_str())
        .collect();
    assert!(
        !serving_rows.contains(&"aux30") && serving_rows.contains(&"work50"),
        "no Serving row may name the gone model while the incoming one serves: {serving_rows:?}"
    );
    assert!(
        runtime.is_dead(aux_port).await,
        "the evicted engine answers nothing on its port"
    );
}

#[tokio::test]
async fn genuine_upstream_error_is_surfaced_never_retried() {
    // A9: a 500 from the engine is a real answer - no crash-restart, no retry,
    // exactly one job in the ledger.
    let (control, work, _, _, runtime, _) = app(
        AppConfig::default(),
        &[("err-model", "mock-error", CategoryId::Work, Some(4.0))],
    );
    let (status, _) = post_json(&control, "/har/load", r#"{"model":"err-model"}"#).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"err-model","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let (_, jobs) = get_json(&control, "/har/jobs").await;
    assert_eq!(
        jobs["jobs"].as_array().map(Vec::len),
        Some(1),
        "no restart job may exist"
    );
    let _ = runtime;
}

#[tokio::test]
async fn no_model_fallback_with_fresh_instants_answers_200() {
    // FINDINGS-06 [1] shape check on a young process. Honest scope: at HEAD
    // every in-process `/har/load` goes through `swap_for_port`, which touches
    // the usage map, so the API cannot leave an occupant hint-free and this
    // request passes pre-fix too; the pre-fix panic (a fabricated
    // `Instant::now() - 3600s`, reachable when the ACTOR loads an occupant
    // without an API touch, e.g. a crash-reload after boot) is pinned by the
    // lib unit test `no_fabricated_instants_survive_short_uptime`.
    let (control, work, _, _, _, _modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, _) = post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "no-model request must not die: {body}"
    );
    assert_eq!(body["echo"], true);
    assert_eq!(body["body"]["model"], "mock-model");
}

#[tokio::test]
async fn no_model_fallback_prefers_the_most_recently_proxied_occupant() {
    // F-2 MRU contract end to end. Honest scope: `swap_for_port` calls
    // `api.touch` on every in-process load, so the control API cannot leave an
    // occupant usage-hint-free; the untouched fallback branch (the pre-fix
    // panic site, reached in production when the ACTOR reloads an occupant
    // without an API touch) is pinned instead by the lib unit test
    // `no_fabricated_instants_survive_short_uptime`. What is observable here:
    // the genuine proxy touch on `newer` must outrank the later-loaded `older`
    // (whose mock answers 500) — a mis-selection is immediately visible.
    let mut config = AppConfig::default();
    let aux: &mut CategoryConfig = config
        .category_mut(CategoryId::Aux)
        .expect("default config has AUX");
    aux.max_models = 2;
    let (control, _, _, aux_router, runtime, _modelconf) = app(
        config,
        &[
            ("newer", "mock", CategoryId::Aux, Some(4.0)),
            ("older", "mock-error", CategoryId::Aux, Some(4.0)),
        ],
    );
    for name in ["newer", "older"] {
        let body = format!(r#"{{"model":"{name}"}}"#);
        let (status, body) = post_json(&control, "/har/load", &body).await;
        assert_eq!(status, StatusCode::OK, "load {name}: {body}");
    }
    assert_eq!(runtime.server_count(), 2, "both occupants serve");

    let (status, body) = post_json(
        &aux_router,
        "/v1/chat/completions",
        r#"{"model":"newer","messages":[]}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "named warm-up must reach `newer`: {body}"
    );

    let (status, body) = post_json(&aux_router, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the touched `newer` must win the MRU sort over the later-loaded `older` \
         (whose mock answers 500): {body}"
    );
    assert_eq!(body["echo"], true);
    assert_eq!(body["body"]["model"], "mock-model");
    assert_eq!(
        runtime.server_count(),
        2,
        "selection must not reap anything"
    );
}

#[tokio::test]
async fn timeout_class_is_502_and_never_restarts() {
    // FINDINGS-06 [2] classifier half: this leg feeds a READ timeout - the
    // client's response deadline expiring on a socket that was accepted but
    // never answered (`is_timeout()` true, `is_connect()` false), the class
    // that used to mean "engine died" and reaped healthy engines. It must map
    // to a typed 502 naming the class, with no restart in sight. This leg does
    // not claim the both-flags shape; the unit table
    // `timeout_precedes_connect_when_both_flags_are_true` (src/api/mod.rs)
    // pins that case, where the classifier judges timeout before connect. The
    // production proxy client's 600 s read timeout is hard-coded and not
    // injectable without new public surface, so the end-to-end 600 s wait is
    // out of scope; the classifier is the decision site and is exercised with
    // the real error value here.
    let hanging = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("hang server binds");
    let port = hanging.local_addr().expect("hang addr").port();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(100))
        .build()
        .expect("plain-HTTP client with a timeout");
    let error = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&serde_json::json!({"messages": []}))
        .send()
        .await
        .expect_err("the hang server never answers");
    assert!(
        error.is_timeout(),
        "accepted-but-silent must be timeout-class: {error:?}"
    );

    let err = classify_send_failure(&error, port).expect_err("timeouts never restart");
    assert_eq!(err.status(), StatusCode::BAD_GATEWAY, "exact class");
    let message = match err {
        simpleharn::api::ApiError::Upstream { reason } => reason,
        other => panic!("expected Upstream, got {other:?}"),
    };
    assert!(
        message.contains(&format!("engine on :{port} timed out"))
            && message.contains("not restarting a live engine"),
        "reason must name the class and refuse the reap: {message}"
    );
}

#[tokio::test]
async fn dead_engine_connect_failure_restarts_exactly_once() {
    // FINDINGS-06 [2] restart-class proof, verified: the T10.1 mock death aborts
    // the axum serve task so the port REFUSES and `send()` yields
    // `is_connect()` (probe-verified: kind Request, source hyper Connect /
    // ConnectionRefused 10061 - see report). `mock-dies-once` makes only the
    // first engine a corpse and its replacement lives forever, so the crash-retry
    // is fully deterministic. (A 502 "retry exhausted" end-to-end case is
    // unattainable with this runtime: a dies-always engine cannot pass
    // `/har/load` readiness, so the connect arm can never fire twice.)
    let (control, work, _, _, runtime, _modelconf) = app(
        AppConfig::default(),
        &[("doomed", "mock-dies-once", CategoryId::Work, Some(4.0))],
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"doomed"}"#).await;
    assert_eq!(status, StatusCode::OK, "load: {body}");
    assert_eq!(runtime.spawn_count(), 1);

    let (_, loaded) = get_json(&control, "/har/status").await;
    let port = u16::try_from(
        loaded["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["model"] == "doomed")
            .and_then(|slot| slot["port"].as_u64())
            .expect("the serving slot reports its engine port"),
    )
    .expect("port fits u16");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !runtime.is_dead(port).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(runtime.is_dead(port).await, "the engine is really dead");

    let (status, body) = post_json(
        &work,
        "/v1/chat/completions",
        r#"{"model":"doomed","messages":[]}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "connect-class must crash-retry: {body}"
    );
    assert_eq!(body["echo"], true);
    assert_eq!(
        runtime.spawn_count(),
        2,
        "initial spawn + exactly one restart for the connect class"
    );
}

/// R2-05-03 must not eat the proxy contract: an engine that answers a plain-text
/// `404` keeps its content type and its bytes on the wire — the rewrite never
/// touches a proxy path. Closest existing pass-through pins:
/// `proxy_passes_the_engines_content_encoding_through_verbatim` and
/// `proxy_forwards_with_model_rewrite_and_streams_the_response`, which stay
/// green for the same reason.
#[tokio::test]
async fn an_upstream_plain_404_on_a_proxy_path_is_never_rewritten() {
    let (control, work, _, _, _, _) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    let response = work
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"qwen3","mock_plain_404":true}"#))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/plain"),
        "the engine's content type survives the pass-through"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    assert_eq!(
        bytes.as_ref(),
        b"engine says no such route",
        "the engine's body must arrive byte-for-byte, not as a harness envelope"
    );
}
