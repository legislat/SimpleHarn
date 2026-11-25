//! The config endpoints on the wire: effective-context catalog rows, the
//! default-model endpoint, body caps, `/har/config/model` write-back with
//! its typed refusals and staleness rules, the webui fixture document,
//! and the config-file persistence rules.

use super::*;
use simpleharn::api::{BootMsStamp, BootTimings};

/// The app behind the effective-context falsifiers (operator ruling
/// 2026-11-02): an empty catalog plus stand-in adapters under the real engine
/// ids, so rows written afterwards join via an explicit rescan.
fn effective_ctx_app() -> (
    Router,
    Router,
    Router,
    Router,
    MockRuntime,
    std::path::PathBuf,
) {
    build_app(
        AppConfig::default(),
        &[],
        &[
            ("llama", Arc::new(MockAdapter)),
            ("strata", Arc::new(MockAdapter)),
            ("ninfer", Arc::new(MockAdapter)),
        ],
    )
}

/// Drops one raw yaml row into the (already-scanned) modelconf dir and
/// publishes it with `POST /har/rescan`.
async fn add_row(control: &Router, modelconf: &std::path::Path, name: &str, yaml: &str) {
    std::fs::write(modelconf.join(format!("{name}.yaml")), yaml).expect("row yaml");
    let (status, body) = post_json(control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "rescan after {name}: {body}");
}

fn catalog_row<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["data"]
        .as_array()
        .expect("list")
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("{id} published in the listing: {body}"))
}

#[tokio::test]
async fn a_strata_row_shows_the_argv_max_context_as_its_context() {
    // Falsifier: pre-fix `model_json` shipped the row `context` key verbatim,
    // so a strata row serving 262144 through `strata_args` displayed nothing.
    let (control, _, _, _, _, modelconf) = effective_ctx_app();
    add_row(
        &control,
        &modelconf,
        "stratamax",
        "engine: strata\ncategory: WORK\nstrata_args: --pack, --native, --max-context, 262144\n",
    )
    .await;
    let (_, body) = get_json(&control, "/har/models").await;
    assert_eq!(catalog_row(&body, "stratamax")["context"], "262144");
}

#[tokio::test]
async fn a_ninfer_row_shows_the_resolved_profiles_table_max_context() {
    // Falsifier: pre-fix a ninfer row carried only an inert `context` key (or
    // none), never the profile its container really serves. Table values pinned
    // from `BUILT_IN_PROFILES`: the default `b` (262144) and explicit `d`
    // (16384) — the same resolution the spawn encodes.
    let (control, _, _, _, _, modelconf) = effective_ctx_app();
    add_row(
        &control,
        &modelconf,
        "nindefault",
        "engine: ninfer\ncategory: WORK\n",
    )
    .await;
    add_row(
        &control,
        &modelconf,
        "nind",
        "engine: ninfer\ncategory: WORK\nninfer_profile: d\n",
    )
    .await;
    let (_, body) = get_json(&control, "/har/models").await;
    assert_eq!(catalog_row(&body, "nindefault")["context"], "262144");
    assert_eq!(catalog_row(&body, "nind")["context"], "16384");
}

#[tokio::test]
async fn a_stale_context_key_never_beats_the_effective_source() {
    // Falsifier: pre-fix both stale rows displayed their inert `context: 4096`
    // while the engine serves 262144, and the mock row displayed its dead key.
    // The truth precedence: the effective source wins; where the engine speaks
    // none (mock, and a strata argv without `--max-context`) the answer is
    // null, never the dead key.
    let (control, _, _, _, _, modelconf) = effective_ctx_app();
    add_row(
        &control,
        &modelconf,
        "stratastale",
        "engine: strata\ncategory: WORK\ncontext: 4096\nstrata_args: --max-context, 262144\n",
    )
    .await;
    add_row(
        &control,
        &modelconf,
        "stranamax",
        "engine: strata\ncategory: WORK\ncontext: 4096\nstrata_args: --pack, --native\n",
    )
    .await;
    add_row(
        &control,
        &modelconf,
        "ninstale",
        "engine: ninfer\ncategory: WORK\ncontext: 4096\n",
    )
    .await;
    add_row(
        &control,
        &modelconf,
        "mockstale",
        "engine: mock\ncategory: WORK\ncontext: 4096\n",
    )
    .await;
    let (_, body) = get_json(&control, "/har/models").await;
    assert_eq!(catalog_row(&body, "stratastale")["context"], "262144");
    assert!(catalog_row(&body, "stranamax")["context"].is_null());
    assert_eq!(catalog_row(&body, "ninstale")["context"], "262144");
    assert!(catalog_row(&body, "mockstale")["context"].is_null());
}

#[tokio::test]
async fn a_llama_row_still_shows_its_own_context_key() {
    // Falsifier for over-reach: the llama row key is the served `-c`, so it
    // keeps shipping verbatim (this test pins the unchanged behavior).
    let (control, _, _, _, _, modelconf) = effective_ctx_app();
    add_row(
        &control,
        &modelconf,
        "llamactx",
        "engine: llama\ncategory: WORK\ncontext: 32768\n",
    )
    .await;
    let (_, body) = get_json(&control, "/har/models").await;
    assert_eq!(catalog_row(&body, "llamactx")["context"], "32768");
}

#[tokio::test]
async fn default_model_endpoint_validates_and_takes_effect() {
    let (control, work, _, _, _, _) = app(
        AppConfig::default(),
        &[
            ("qwen3", "mock", CategoryId::Work, Some(4.0)),
            ("emb-model", "mock", CategoryId::Emb, Some(2.0)),
        ],
    );

    // Wrong category: 400 with the reason.
    let (status, body) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"RP","model":"qwen3"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");

    // Valid set: ok, and the status reflects it.
    let (status, _) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"qwen3"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, status_body) = get_json(&control, "/har/status").await;
    let work_cat = status_body["categories"]
        .as_array()
        .expect("categories")
        .iter()
        .find(|cat| cat["id"] == "WORK")
        .expect("WORK");
    assert_eq!(work_cat["default_model"], "qwen3");
    // The no-model fallback now loads it without touching the boot config file.
    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["body"]["model"], "mock-model");
}

#[tokio::test]
async fn default_model_persists_the_canonical_name_not_the_alias() {
    // `resolve` accepts case-insensitive names and GGUF aliases,
    // but the scheduler and the boot check compare `m.name == model` exactly. The
    // endpoint must persist the entry's canonical name, or the default it just
    // accepted 404s on the first no-model request and after every restart.
    let (control, work, _, _, _, modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    std::fs::write(
        modelconf.join("qwen3.yaml"),
        "name: qwen3\nengine: mock\ncategory: WORK\nvram_gb: 4\nmodel: models/Qwen3-0.6B-Q8_0.gguf\n",
    )
    .expect("seed alias row");
    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let (status, body) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"Qwen3-0.6B-Q8_0"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["default_model"], "qwen3", "canonical name: {body}");

    // The no-model fallback must load it: the scheduler resolves by exact name.
    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["body"]["model"], "mock-model");

    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(modelconf.join(TEST_CONFIG_NAME)).expect("config file"),
    )
    .expect("valid json");
    let work_cat = on_disk["categories"]
        .as_array()
        .expect("categories")
        .iter()
        .find(|cat| cat["id"] == "WORK")
        .expect("WORK");
    assert_eq!(
        work_cat["default_model"], "qwen3",
        "the persisted value is the canonical name, not the alias: {on_disk}"
    );
}

#[tokio::test]
async fn default_model_requires_the_model_key_and_rejects_wrong_types() {
    // Absent, null and wrong-typed `model` all once cleared the
    // persisted default. Only `null` may clear; absent and wrong-typed are caller
    // mistakes and must 400 without touching the file.
    let (control, _, _, _, _, modelconf) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, _) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"qwen3"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let before = std::fs::read_to_string(modelconf.join(TEST_CONFIG_NAME)).expect("config");

    for body in [
        r#"{"category":"WORK"}"#,
        r#"{"category":"WORK","model":123}"#,
        r#"{"category":"WORK","model":true}"#,
        r#"{"category":"WORK","model":["qwen3"]}"#,
        r#"{"category":"WORK","model":{"name":"qwen3"}}"#,
    ] {
        let (status, reply) = post_json(&control, "/har/config/default", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "`{body}`: {reply}");
        assert_eq!(
            std::fs::read_to_string(modelconf.join(TEST_CONFIG_NAME)).expect("config"),
            before,
            "a refused body may not rewrite the config: `{body}`"
        );
    }

    // `null` is the documented clear, and it still works.
    let (status, body) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":null}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body["default_model"].is_null(), "{body}");
}

#[tokio::test]
async fn control_bodies_are_capped_below_the_proxy_ceiling() {
    // The 1 GiB proxy ceiling once was shared by
    // `/har/*`, so a control POST could force a 1 GiB buffer. The control surface
    // now has its own 64 KiB limit; a body over it is refused before any handler.
    let (control, _, _, _, _, _) = app(AppConfig::default(), &[]);
    let oversized = "x".repeat(64 * 1024 + 1);
    let body = format!(r#"{{"model":"{oversized}"}}"#);
    let response = control
        .clone()
        .oneshot(
            Request::post("/har/load")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the control limit must refuse the body before the handler: {text}"
    );
    assert!(
        text.contains("length limit exceeded"),
        "the refusal must name the limit, not a parse error: {text}"
    );
    // Operator ruling: an over-limit body stays `400 bad_request`;
    // `413` is not used — request-shape refusals stay one class.
    let body: Value = serde_json::from_slice(&bytes).expect("the refusal is the JSON envelope");
    assert_eq!(body["code"], "bad_request", "flat envelope: {body}");
    assert_eq!(body["error"]["code"], "bad_request", "envelope: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("length limit exceeded"),
        "the envelope message still names the limit: {body}"
    );
}

#[tokio::test]
async fn proxy_bodies_are_capped_at_64_mib() {
    // The proxy ceiling was 1 GiB, and the body is buffered into a
    // `Json<Value>` before any handler runs. The 64 MiB cap must be enforced by the
    // extractor, not discovered after a multi-GiB parse. Falsifier: restore 1 GiB and
    // this body parses far enough to answer a JSON error instead of the limit.
    let (_, work, _, _, _, _) = app(AppConfig::default(), &[]);
    let oversized = vec![b'x'; 64 * 1024 * 1024 + 1];
    let response = work
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::from(oversized))
                .expect("static request"),
        )
        .await
        .expect("in-process request");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("bounded body");
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the proxy limit must refuse the body before the handler: {text}"
    );
    assert!(
        text.contains("length limit exceeded"),
        "the refusal must name the limit, not a JSON parse error: {text}"
    );
    // Operator ruling: an over-limit body stays `400 bad_request`;
    // `413` is not used — request-shape refusals stay one class.
    let body: Value = serde_json::from_slice(&bytes).expect("the refusal is the JSON envelope");
    assert_eq!(body["code"], "bad_request", "flat envelope: {body}");
    assert_eq!(body["error"]["code"], "bad_request", "envelope: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("length limit exceeded"),
        "the envelope message still names the limit: {body}"
    );
}

#[tokio::test]
async fn config_model_writes_back_to_the_yaml_file() {
    // The CPU leg rides a `llama` row since the engine-knob gate:
    // a `cpu` edit is refused for engines without the `cpu_offload` knob
    // (the
    // refusal legs are pinned by
    // `cpu_edit_is_refused_on_models_whose_engine_never_reads_the_key`); the
    // category leg keeps the mock row, proving category writes stay engine-blind.
    let (control, _, _, _, _, modelconf) = build_app(
        AppConfig::default(),
        &[
            ("qwen3", "mock", CategoryId::Work, Some(4.0)),
            ("gguf3", "llama", CategoryId::Work, Some(4.0)),
        ],
        &[("llama", Arc::new(MockAdapter))],
    );
    let file = modelconf.join("qwen3.yaml");
    let original = std::fs::read_to_string(&file).expect("model file");

    // Category edit: written into the file, catalog refreshed.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let edited = std::fs::read_to_string(&file).expect("read back");
    assert!(edited.contains("category: AUX"), "was: {edited}");
    assert!(edited.contains("name: qwen3"), "rest of the file survives");

    // The catalog now shows the new category on the right listener.
    let (_, rp_models) = get_json(&control, "/har/models").await;
    let entry = rp_models["data"]
        .as_array()
        .expect("list")
        .iter()
        .find(|m| m["id"] == "qwen3")
        .expect("entry");
    assert_eq!(entry["category"], "AUX");

    // CPU edit on the offload engine: boolean -> yaml key, and back.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gguf3","cpu":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(std::fs::read_to_string(modelconf.join("gguf3.yaml"))
        .expect("read")
        .contains("cpu_offload: true"));
    let _ = original;
}

#[tokio::test]
async fn a_wrong_typed_cpu_is_refused_rather_than_silently_dropped() {
    // `body.get("cpu").and_then(Value::as_bool)` once treated `"true"`
    // and `1` as "absent", so a client that asked for two changes got one and a
    // `{"status":"edited"}` saying both landed. The in-file sibling
    // `/har/config/logging` 400s a non-bool `enabled`, and the wrong-typed-`model`
    // fix made that the parity
    // rule for this file — the same mistake may not live two routes apart.
    let (control, _, _, _, _, modelconf) = build_app(
        AppConfig::default(),
        &[("gguf3", "llama", CategoryId::Work, Some(4.0))],
        &[("llama", Arc::new(MockAdapter))],
    );
    let file = modelconf.join("gguf3.yaml");
    let before = std::fs::read_to_string(&file).expect("model file");

    for presented in [r#""true""#, "1", "null", r#"[true]"#, r#"{"on":true}"#] {
        let (status, body) = post_json(
            &control,
            "/har/config/model",
            &format!(r#"{{"model":"gguf3","cpu":{presented}}}"#),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "`cpu:{presented}` is a caller mistake, not an absent field: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("cpu") && message.contains("boolean"),
            "the refusal must name the key and what it wanted: {message}"
        );
    }

    // The leg with teeth: the type error precedes the rewrite, so the sibling edit in
    // the same request is refused too — no half-applied write behind a 400.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gguf3","category":"AUX","cpu":"yes"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let untouched = std::fs::read_to_string(&file).expect("read back");
    assert_eq!(
        untouched, before,
        "a refused request may not edit the file at all"
    );
    assert!(!untouched.contains("category: AUX"), "was: {untouched}");

    // And a real boolean is still accepted, in both polarities.
    for offered in ["true", "false"] {
        let (status, body) = post_json(
            &control,
            "/har/config/model",
            &format!(r#"{{"model":"gguf3","cpu":{offered}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(
            std::fs::read_to_string(&file)
                .expect("read")
                .contains(&format!("cpu_offload: {offered}")),
            "`cpu:{offered}` must land"
        );
    }
}

#[tokio::test]
async fn a_wrong_typed_category_is_refused_rather_than_silently_dropped() {
    // The exact parity sibling of the `cpu` leg above.
    // `body.get("category").and_then(Value::as_str)` read `5`, `true` and `null` as
    // "absent", so `{"model":x,"category":5,"cpu":true}` applied only the cpu edit
    // and answered `{"status":"edited"}` — one of the two requested changes,
    // reported as success. `/har/config/default` already refuses the same shape;
    // one field's parity bug may not stay on its sibling.
    let (control, _, _, _, _, modelconf) = build_app(
        AppConfig::default(),
        &[("gguf3", "llama", CategoryId::Work, Some(4.0))],
        &[("llama", Arc::new(MockAdapter))],
    );
    let file = modelconf.join("gguf3.yaml");
    let before = std::fs::read_to_string(&file).expect("model file");

    for presented in ["5", "true", "null", r#"["AUX"]"#, r#"{"c":"AUX"}"#] {
        let (status, body) = post_json(
            &control,
            "/har/config/model",
            &format!(r#"{{"model":"gguf3","category":{presented}}}"#),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "`category:{presented}` is a caller mistake, not an absent field: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("category") && message.contains("string"),
            "the refusal must name the key and what it wanted: {message}"
        );
    }

    // The leg with teeth: the type error precedes the rewrite, so the sibling `cpu`
    // edit riding the same body is refused with it — no half-applied write behind
    // a 400. Pre-fix this leg answered 200 `"edited"` with only the cpu key landed.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gguf3","category":5,"cpu":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    let untouched = std::fs::read_to_string(&file).expect("read back");
    assert_eq!(
        untouched, before,
        "a refused request may not edit the file at all"
    );
    assert!(!untouched.contains("cpu_offload"), "was: {untouched}");

    // And a real string is still accepted, exactly as before.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gguf3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        std::fs::read_to_string(&file)
            .expect("read")
            .contains("category: AUX"),
        "a valid category edit must still land"
    );
}

#[tokio::test]
async fn the_webui_fixture_is_a_document_the_endpoint_can_actually_emit() {
    // The DOM suite drives the page from a hand-maintained
    // `tests/webui/fixtures/status.json`, and nothing compared that file with what
    // `/har/status` really sends. It had drifted into a document the server cannot
    // produce — the `version` field behind the crate, and a `boot.scan_ms` no
    // `BootTimings` field has ever had (the initial scan is deliberately unmeasured)
    // while two measures that do exist were absent. A page tested against a
    // fictional server passes while the real one breaks it.
    let (control, _, _, _, _, _) = build_app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
        &[],
    );
    post_json(&control, "/har/load", r#"{"model":"qwen3"}"#).await;
    let (_, live) = get_json(&control, "/har/status").await;
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/webui/fixtures/status.json")
            .expect("the DOM suite's fixture is committed"),
    )
    .expect("the fixture is JSON");

    fn keys(value: &Value, what: &str) -> Vec<String> {
        let mut keys = value
            .as_object()
            .unwrap_or_else(|| panic!("{what} must be an object"))
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    assert_eq!(
        keys(&fixture, "fixture"),
        keys(&live, "/har/status"),
        "the fixture carries a different top-level shape than the endpoint emits"
    );
    assert_eq!(
        fixture["version"],
        env!("CARGO_PKG_VERSION"),
        "the fixture quotes a version the harness has never run"
    );

    // The release identity rides beside the version: the fixture quotes one in
    // the tag grammar, and the live document answers with null (untagged or
    // dirty build tree) or exactly such a tag (a tagged checkout embeds it at
    // build time). The shape is pinned here, never one verdict: asserting a
    // fixed null would fail the tagged CI checkout that legitimately carries
    // its tag, and asserting the fixture's own tag would pin this tree's git
    // state into a test.
    fn release_tag_shape_ok(tag: &str) -> bool {
        let mut parts = tag.split('.');
        let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        match (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) {
            (Some(a), Some(b), Some(c), None, None) => numeric(a) && numeric(b) && numeric(c),
            (Some(a), Some(b), Some(c), Some(d), None) => {
                numeric(a) && numeric(b) && numeric(c) && numeric(d)
            }
            _ => false,
        }
    }
    assert!(
        fixture["release"]
            .as_str()
            .is_some_and(release_tag_shape_ok),
        "the fixture's release must be a `x.y.z[.h]` tag: {:?}",
        fixture["release"]
    );
    match &live["release"] {
        Value::Null => {}
        Value::String(tag) => assert!(
            release_tag_shape_ok(tag),
            "the embedded release identity must be a `x.y.z[.h]` tag: {tag:?}"
        ),
        other => panic!("`release` must be null or a tag string, got {other}"),
    }

    // `boot` is the one section whose live shape depends on which measures have
    // completed, so it is compared against the fully-stamped variant.
    let first_bind_at = BootMsStamp::default();
    assert!(first_bind_at.stamp(41), "the first writer wins");
    let reap_ms = BootMsStamp::default();
    assert!(reap_ms.stamp(812), "the first writer wins");
    let stamped = BootTimings {
        config_ms: 0,
        scheduler_spawn_ms: 7,
        bind_ms: Some(53),
        first_bind_at,
        reap_ms,
    };
    assert_eq!(
        keys(&fixture["boot"], "fixture.boot"),
        keys(
            &serde_json::to_value(stamped).expect("boot timings serialize"),
            "BootTimings"
        ),
        "`boot` must name the measures the harness has, no more and no fewer"
    );

    for section in ["categories", "mem", "slots", "listeners"] {
        let row = |doc: &Value| match doc[section].as_array() {
            Some(rows) => rows.first().cloned().unwrap_or(Value::Null),
            None => doc[section].clone(),
        };
        assert_eq!(
            keys(&row(&fixture), &format!("fixture.{section}")),
            keys(&row(&live), &format!("/har/status.{section}")),
            "the {section} rows in the fixture are not the rows the endpoint sends"
        );
    }

    let live_job = live["jobs"]
        .as_array()
        .and_then(|jobs| jobs.first())
        .cloned()
        .expect("the mock load leaves a job row to compare against");
    let fixture_job = fixture["jobs"]
        .as_array()
        .and_then(|jobs| jobs.first())
        .cloned()
        .expect("the fixture carries job rows");
    // B6: `path` is present only on a row that passed the admission gate —
    // a refusal carries the refusal, an unload never stands before the
    // gate, and the fixture's first row is a refusal while the mock load's
    // is an admit. The key-set compare exempts it the same targeted way
    // the models leg below exempts `launched_at`/`stale_config`; the rung
    // VALUES are pinned by `scheduler::tests` and
    // `the_job_ledger_names_the_admission_rung`. The fixture carries the
    // rung on every admitted row, re-derived from each row's own
    // free/gate/victims figures — it stays a document the endpoint can
    // emit. Falsifier: add any OTHER field to one side of this pair and
    // the exempt compare goes red.
    let fixture_job_keys = keys(&fixture_job, "fixture.jobs[0]");
    let live_job_keys = keys(&live_job, "/har/status.jobs[0]");
    let rung_exempt = |job_keys: &[String]| -> Vec<String> {
        job_keys
            .iter()
            .filter(|key| **key != "path")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rung_exempt(&fixture_job_keys),
        rung_exempt(&live_job_keys),
        "the fixture's job ledger is not the ledger shape `JobStatus` serializes"
    );
    let rung_only: Vec<&String> = live_job_keys
        .iter()
        .filter(|key| !fixture_job_keys.contains(key))
        .collect();
    assert!(
        rung_only.iter().all(|key| *key == "path"),
        "the live row carries fields the fixture never has: {rung_only:?}"
    );

    // Same rule for the models table (review 2026-10-31): the DOM suite renders
    // `fixtures/models.json` through the Cost cell's provenance ladder, and the
    // fixture had drifted eight fields behind `model_json` — the page passed against
    // rows `/har/models` never sends. `launched_at`/`stale_config` exist on a row
    // ONLY while its model is installed, so they alone are exempt and must be the
    // entire difference; anything else in that difference is a shape violation.
    let (_, live_models) = get_json(&control, "/har/models").await;
    let fixture_models: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/webui/fixtures/models.json")
            .expect("the DOM suite's models fixture is committed"),
    )
    .expect("the models fixture is JSON");
    let fixture_keys = keys(
        &row_of(&fixture_models, "fixture /har/models"),
        "fixture /har/models",
    );
    let live_keys = keys(&row_of(&live_models, "/har/models"), "live /har/models");
    let installed_only = |keys: &[String]| {
        keys.iter()
            .filter(|key| *key != "launched_at" && *key != "stale_config")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        installed_only(&fixture_keys),
        installed_only(&live_keys),
        "the models fixture rows are not the rows the endpoint sends"
    );
    let installed: Vec<&String> = live_keys
        .iter()
        .filter(|key| !fixture_keys.contains(key))
        .collect();
    assert!(
        installed
            .iter()
            .all(|key| *key == "launched_at" || *key == "stale_config"),
        "the live row carries fields the fixture never has: {installed:?}"
    );

    fn row_of(doc: &Value, what: &str) -> Value {
        doc["data"]
            .as_array()
            .and_then(|rows| rows.first())
            .cloned()
            .unwrap_or_else(|| panic!("{what} carries no rows"))
    }
}

#[tokio::test]
async fn config_model_reports_writeback_refusals_by_http_class() {
    // The write-back's typed failures decide the status, so a
    // refusal that is the *file's* state is no longer dressed up as a harness
    // fault. Both legs reach the endpoint through the window the editor-race
    // doctrine allows (DECISIONS: the operator edits yamls while the harness is
    // idle) — the catalog still publishes the entry while the file on disk has
    // moved under it. Falsifier: send `WouldBreakFile` back to `Internal` in
    // `ApiError::from_writeback` and the 409 leg answers 500.
    let (control, _, _, _, _, modelconf) = build_app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
        &[],
    );
    let file = modelconf.join("qwen3.yaml");
    // The actor's cached catalog must hold the row before the file is moved under
    // it — that cache is exactly what makes the two legs below reachable (and a
    // request here also waits out the boot scan, so a late scan cannot turn this
    // pin into a silent 404).
    let (_, published) = get_json(&control, "/har/models").await;
    assert!(
        published["data"]
            .as_array()
            .expect("list")
            .iter()
            .any(|entry| entry["id"] == "qwen3"),
        "the row must be published before the file moves: {published}"
    );

    // 409 conflict: the edited document is one the scan gate would never publish,
    // so the blanket refusal is a live-state clash — and the scanner's own problem
    // lines reach the operator, who is the only one who can fix the file.
    std::fs::write(&file, "name: qwen3\nnot a key line\n").expect("mangle");
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(body["code"], "conflict");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("edit would break the file"),
        "the module's own sentence must survive the mapping: {message}"
    );
    assert!(
        message.contains("not a key line"),
        "the problem lines are the operator's diagnosis: {message}"
    );
    // The config-state refusal names the file, as a
    // caller-side PREFIX. The two `contains` legs above already prove the module's
    // sentence survived; these prove it is intact at both seams — the reason opens
    // with the path and the module's sentence is its exact tail. Falsifier: drop
    // the prefix in `ApiError::from_writeback` and the first leg fails; corrupt the
    // sentence inside the composition and the `contains` legs and this tail leg
    // fail while the module's own pins stay green.
    assert!(
        message.starts_with(&format!("conflict: {}: ", file.display())),
        "a config-state refusal must name its file first: {message}"
    );
    assert!(
        message.ends_with(
            "edit would break the file: line 2: not a `key: value` line: `not a key line`"
        ),
        "the module's sentence must be the untouched tail of the prefix: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("read back"),
        "name: qwen3\nnot a key line\n",
        "a refused edit writes nothing at all"
    );

    // 500 internal: the file itself vanished under a still-published entry — that
    // one genuinely is the harness's/environment's problem, and stays 500 with the
    // `internal` code. The error-body doctrine moved the OS sentence out of the client
    // body and into the ring: the body keeps the class, names the harness log as
    // the place the detail lives, and must not carry the path; the full reason's
    // presence in the ring is pinned by
    // `os_failure_text_reaches_the_ring_but_never_the_client_body`.
    std::fs::write(&file, "name: qwen3\nengine: mock\n").expect("restore");
    std::fs::remove_file(&file).expect("delete");
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {body}");
    assert_eq!(body["code"], "internal");
    assert_eq!(body["error"]["code"], "internal");
    assert_eq!(body["error"]["message"], body["message"]);
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("internal error:") && message.contains("harness log"),
        "the stable sentence keeps the class and points at the detail: {message}"
    );
    for leaked in [
        "cannot read",
        "qwen3.yaml",
        modelconf.to_str().unwrap_or("?"),
    ] {
        assert!(
            !body.to_string().contains(leaked),
            "the 500 body may not carry the OS detail or its path ({leaked}): {body}"
        );
    }

    // And the healthy path is untouched by the typing: same endpoint, same 200.
    std::fs::write(&file, "name: qwen3\nengine: mock\ncategory: WORK\n").expect("reseed");
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(std::fs::read_to_string(&file)
        .expect("read")
        .contains("category: AUX"));
}

#[tokio::test]
async fn a_flag_edit_never_ambers_its_own_loaded_model_and_status_never_stats() {
    // Axis-2 interlock 5 (api side) plus the zero-stat counter-probe, together on
    // the surface the page reads:
    //  * the loaded row carries `launched_at` (server-formatted) + `stale_config`
    //    false, and an unloaded row carries NEITHER key (absent, not null);
    //  * the harness's own `cpu` edit rewrites the yaml through the handler's
    //    reload and must not amber its own model (re-baseline rides the write);
    //  * with the file deleted from under the readers, `/har/models` and
    //    `/har/status` keep answering from cache — a status path that statted the
    //    model file would flip the verdict here (behavioural leg; the structural
    //    leg is the api-source tripwire in the lib tests);
    //  * and the verdict IS alive: the next rescan accuses the deleted file.
    // Falsifier: drop the `note_model_file_written` send in `har_config_model` and
    // the post-edit assert fails — the handler's own reload ambering the row it
    // just edited is the exact UX lie the interlock forbids.
    let (control, _work, _rp, _aux, _runtime, modelconf) = build_app(
        AppConfig::default(),
        &[
            ("llamaline", "llama", CategoryId::Work, Some(4.0)),
            ("plainrow", "mock", CategoryId::Work, Some(4.0)),
        ],
        &[("llama", Arc::new(MockAdapter))],
    );
    let (_, body) = get_json(&control, "/har/models").await;
    let row_of = |body: &Value, id: &str| -> Value {
        body["data"]
            .as_array()
            .expect("list")
            .iter()
            .find(|m| m["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} in the listing: {body}"))
    };
    let unloaded = row_of(&body, "plainrow");
    assert!(
        unloaded.get("launched_at").is_none() && unloaded.get("stale_config").is_none(),
        "absent when not loaded, never null: {unloaded}"
    );

    let (status, body) = post_json(&control, "/har/load", r#"{"model":"llamaline"}"#).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let (_, body) = get_json(&control, "/har/models").await;
    let loaded = row_of(&body, "llamaline");
    let stamp = loaded["launched_at"]
        .as_str()
        .expect("server-formatted stamp")
        .to_string();
    assert_eq!(
        stamp.len(),
        20,
        "ISO-8601 seconds, printed verbatim by the page: {stamp}"
    );
    assert_eq!(loaded["stale_config"], false, "{loaded}");
    assert!(
        row_of(&body, "plainrow").get("launched_at").is_none(),
        "the edit rode only the loaded row"
    );

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"llamaline","cpu":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let (_, body) = get_json(&control, "/har/models").await;
    let after_edit = row_of(&body, "llamaline");
    assert_eq!(after_edit["cpu_offload"], true, "{after_edit}");
    assert_eq!(
        after_edit["stale_config"], false,
        "the harness's own flag edit never ambers its own model"
    );
    assert_eq!(
        after_edit["launched_at"].as_str(),
        Some(stamp.as_str()),
        "and never moves the launch stamp"
    );

    // Counter-probe: the file leaves the disk entirely; the read paths answer from
    // cache, verdict unmoved (a statting status path would flip it right here).
    std::fs::remove_file(modelconf.join("llamaline.yaml")).expect("delete the yaml");
    let (status, models) = get_json(&control, "/har/models").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the model list still serves from cache"
    );
    let hidden = row_of(&models, "llamaline");
    assert_eq!(
        hidden["stale_config"], false,
        "the verdict is a cache, not a stat: {hidden}"
    );
    assert_eq!(hidden["launched_at"].as_str(), Some(stamp.as_str()));
    let (status, doc) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::OK);
    let slot = doc["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .find(|s| s["model"] == "llamaline")
        .expect("the resident still holds its slot");
    assert_eq!(slot["stale_config"], false, "status answers from cache too");

    // The verdict is alive: the rescan accuses the deleted file (the marker's
    // deleted-file leg, end to end).
    let (status, _) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK);
    let (_, doc) = get_json(&control, "/har/status").await;
    let slot = doc["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .find(|s| s["model"] == "llamaline")
        .expect("reload never unloads the resident");
    assert_eq!(
        slot["stale_config"], true,
        "a vanished file is evidence of change: the amber lands on the rescan"
    );
}

#[tokio::test]
async fn cpu_edit_is_refused_on_models_whose_engine_never_reads_the_key() {
    // With the inert-key posture shipped, a scripted `cpu` write to a
    // strata (or mock) row would make the harness itself the #1 producer of the
    // InertKey noise it just surfaced. The page offers no control there and
    // the API now agrees: 400 naming the key, the row engine and the owning set —
    // while the engine-blind category write and the llama cpu write keep working.
    // Falsifier: removing the offload-knob gate in `har_config_model` returns
    // 200 here and lands a dead `cpu_offload` line in the yaml.
    let (control, _, _, _, _, modelconf) = build_app(
        AppConfig::default(),
        &[
            ("strataline", "strata", CategoryId::Work, Some(8.0)),
            ("mockline", "mock", CategoryId::Work, Some(4.0)),
            ("llamaline", "llama", CategoryId::Work, Some(4.0)),
        ],
        &[
            ("strata", Arc::new(MockAdapter)),
            ("llama", Arc::new(MockAdapter)),
        ],
    );
    for (model, engine) in [("strataline", "strata"), ("mockline", "mock")] {
        let (status, body) = post_json(
            &control,
            "/har/config/model",
            &format!(r#"{{"model":"{model}","cpu":true}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
        assert_eq!(body["code"], "bad_request");
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("cpu_offload")
                && message.contains(engine)
                && message.contains("llama"),
            "the refusal must name key, row engine and the owning set: {message}"
        );
        let text = std::fs::read_to_string(modelconf.join(format!("{model}.yaml"))).expect("read");
        assert!(
            !text.contains("cpu_offload"),
            "a refused edit must leave the yaml free of the dead key: {text}"
        );
    }
    // The refusal is surgical: the category write still works on the same strata row.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"strataline","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(std::fs::read_to_string(modelconf.join("strataline.yaml"))
        .expect("read")
        .contains("category: AUX"));
    // And the offload engine still accepts the cpu key.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"llamaline","cpu":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(std::fs::read_to_string(modelconf.join("llamaline.yaml"))
        .expect("read")
        .contains("cpu_offload: true"));
}

#[tokio::test]
async fn moving_the_default_model_clears_its_old_category_default() {
    // Operator finding on default-model coherence: moving the default model out of a category
    // must not leave the old listener defaulting to a model it no longer serves.
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"qwen3"}"#,
    )
    .await;

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"RP"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let (_, status_body) = get_json(&control, "/har/status").await;
    let work_cat = status_body["categories"]
        .as_array()
        .expect("categories")
        .iter()
        .find(|cat| cat["id"] == "WORK")
        .expect("WORK");
    assert!(
        work_cat["default_model"].is_null(),
        "the old category must not keep a default that moved away: {status_body}"
    );
}

#[tokio::test]
async fn defaults_persist_to_the_config_boot_loaded_even_under_a_custom_path() {
    // Operator finding on write-back targeting: write-back must hit the file boot read
    // (`SIMPLEHARN_CONFIG` aware), and a partial config with no `categories` key
    // must still accept the patch instead of silently staying in memory.
    let path = simpleharn::testutil::temp_dir("api-config").join("simpleharn.json");
    std::fs::write(&path, r#"{"modelconf_dir": "unused"}"#).expect("seed config");
    let mut config =
        AppConfig::parse(&std::fs::read_to_string(&path).expect("read")).expect("parses");
    config.source_path = Some(path.clone());

    let (control, _, _, _, _, _) = app(config, &[("qwen3", "mock", CategoryId::Work, Some(4.0))]);
    let (status, body) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"qwen3"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body["persisted"].as_bool().unwrap_or(false),
        "custom-path config must accept the write: {body}"
    );

    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("persisted file must exist at the boot path"),
    )
    .expect("valid json");
    assert!(
        on_disk.get("source_path").is_none(),
        "the internal path marker must never be serialized"
    );
    // The written file must survive a full boot reload — the class of bug that
    // dropped `control: true` on the first rebuild-through-to_value.
    let reloaded =
        AppConfig::parse(&std::fs::read_to_string(&path).expect("read back persisted config"))
            .expect("persisted config parses");
    let reload_errors = reloaded.validate();
    assert!(
        reload_errors.is_empty(),
        "persisted config must validate: {reload_errors:?}"
    );
    let work = reloaded
        .categories
        .iter()
        .find(|cat| cat.id == CategoryId::Work)
        .expect("WORK");
    assert_eq!(work.default_model.as_deref(), Some("qwen3"));
    assert!(
        reloaded.categories.iter().any(|cat| cat.control),
        "the control flag must survive the persist reload"
    );
    let work_json = on_disk["categories"]
        .as_array()
        .expect("categories rebuilt from boot config")
        .iter()
        .find(|cat| cat["id"] == "WORK")
        .expect("WORK");
    assert_eq!(work_json["default_model"], "qwen3");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_default_that_never_landed_never_changes_memory() {
    // Config file is king. A write that never landed may not leave
    // memory holding a value the file does not. The boot path is a directory
    // here: every read fails and the publish rename can never replace it, so
    // `persist_default_model` can only answer `NotLanded`. Falsifier: the
    // pre-fix shape inserted the override before the persist leg, so this
    // call answered `default_model: "new"` and `/har/status` served a default
    // the next boot will never see.
    let dir = simpleharn::testutil::temp_dir("api-config-never-landed");
    let config_path = dir.join(TEST_CONFIG_NAME);
    std::fs::create_dir(&config_path).expect("the boot path is an unwritable directory");
    let mut config = AppConfig::default();
    config
        .categories
        .iter_mut()
        .find(|cat| cat.id == CategoryId::Work)
        .expect("WORK")
        .default_model = Some("old".to_string());
    config.source_path = Some(config_path.clone());
    let (control, _, _, _, _, _) = app(
        config,
        &[
            ("old", "mock", CategoryId::Work, Some(4.0)),
            ("new", "mock", CategoryId::Work, Some(4.0)),
        ],
    );

    let (status, body) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"new"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["persisted"],
        serde_json::json!(false),
        "a write that could never rename cannot claim persistence: {body}"
    );
    assert_eq!(
        body["default_model"],
        serde_json::json!("old"),
        "the answer carries the still-effective value, not the refused edit: {body}"
    );

    let (_, status_body) = get_json(&control, "/har/status").await;
    let work_cat = status_body["categories"]
        .as_array()
        .expect("categories")
        .iter()
        .find(|cat| cat["id"] == "WORK")
        .expect("WORK");
    assert_eq!(
        work_cat["default_model"], "old",
        "memory still answers the previous default the file holds: {status_body}"
    );
    assert!(
        config_path.is_dir(),
        "the failed write left no file behind where the boot path stands"
    );
    let _ = std::fs::remove_dir(&config_path);
}
