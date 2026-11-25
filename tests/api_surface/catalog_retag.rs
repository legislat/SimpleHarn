//! The catalog and the retag path on the wire: embedding-model retags that
//! must survive the next scan, the ambiguous-alias family, retags that
//! clear dangling defaults and report their follow-ups, and the scan-error
//! postures the page shows.

use super::*;

/// `AppConfig::default()` with a `llama_server_exe` that exists. The default is
/// `None` — and `None` reports no `engine_binary_missing` at
/// all (the unset posture belongs to `config_warnings`, not to a scan error) —
/// but a test pinning the BINARY-IS-VERDICTED path still needs a configured,
/// present stand-in. `tag` keeps each test's stand-in directory attributable
/// and isolated.
fn config_with_existing_llama(tag: &str) -> AppConfig {
    let stand_in = simpleharn::testutil::temp_dir(tag).join("llama-server.exe");
    std::fs::write(&stand_in, b"test stand-in").expect("llama stand-in file");
    AppConfig {
        llama_server_exe: Some(stand_in),
        ..AppConfig::default()
    }
}

#[tokio::test]
async fn retagging_an_embedding_model_out_of_emb_keeps_it_in_the_catalog() {
    // The page's category dropdown once wrote only `category:`, so
    // retagging an `is_embedding_model: true` + `category: EMB` file to WORK left
    // the alias; the reload at the end of the same handler hit
    // EmbeddingCategoryConflict and DROPPED the file — the model 404s everywhere,
    // its row vanishes, and the endpoint answered success. The removal must ride
    // the same atomic rewrite as the category write.
    let (control, _, _, _, _, modelconf) = app(
        config_with_existing_llama("retag-emb"),
        &[("plain", "mock", CategoryId::Work, Some(4.0))],
    );
    let emb_file = modelconf.join("gemma-emb.yaml");
    std::fs::write(
        &emb_file,
        "# operator header\nname: gemma-emb\nengine: mock\nis_embedding_model: true\ncategory: EMB\nvram_gb: 2\n",
    )
    .expect("seed embedding fixture");
    let keep_file = modelconf.join("gemma-keep.yaml");
    std::fs::write(
        &keep_file,
        "name: gemma-keep\nengine: mock\nis_embedding_model: true\ncategory: EMB\n",
    )
    .expect("seed alias-keeper fixture");
    let plain_file = modelconf.join("plain.yaml");
    let plain_before = std::fs::read_to_string(&plain_file).expect("seed plain");

    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["models"], 3, "all fixtures catalogued: {body}");

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gemma-emb","category":"WORK"}"#,
    )
    .await;
    assert!(
        status.is_success(),
        "retag must answer 2xx: {status} {body}"
    );

    let edited = std::fs::read_to_string(&emb_file).expect("read back");
    assert!(edited.contains("category: WORK"), "{edited}");
    assert_eq!(
        edited.matches("category:").count(),
        1,
        "exactly one category line: {edited}"
    );
    assert!(
        !edited.contains("is_embedding_model"),
        "the stale alias survives the retag: {edited}"
    );
    assert!(
        edited.starts_with("# operator header"),
        "operator bytes lost: {edited}"
    );

    // The survival claim, straight from the finding's falsifier: a fresh scan keeps
    // the model in WORK with no catalog error.
    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["models"], 3,
        "the retagged model must survive the scan: {body}"
    );
    assert!(
        body["catalog_errors"].as_array().is_some_and(Vec::is_empty),
        "no conflict after the retag: {body}"
    );
    let (_, models) = get_json(&control, "/har/models").await;
    let entry = models["data"]
        .as_array()
        .expect("list")
        .iter()
        .find(|m| m["id"] == "gemma-emb")
        .expect("model gone from the catalog after one dropdown click");
    assert_eq!(entry["category"], "WORK");

    // Retag back to EMB: survives, no conflict.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gemma-emb","category":"EMB"}"#,
    )
    .await;
    assert!(
        status.is_success(),
        "retag back must answer 2xx: {status} {body}"
    );
    let (_, models) = get_json(&control, "/har/models").await;
    let entry = models["data"]
        .as_array()
        .expect("list")
        .iter()
        .find(|m| m["id"] == "gemma-emb")
        .expect("model must survive the retag back to EMB");
    assert_eq!(entry["category"], "EMB");

    // A retag to EMB must not needlessly strip a present alias: `category: EMB`
    // beside `is_embedding_model: true` is the legal shape.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gemma-keep","category":"EMB"}"#,
    )
    .await;
    assert!(status.is_success(), "status: {status} {body}");
    let kept = std::fs::read_to_string(&keep_file).expect("read back");
    assert!(
        kept.contains("is_embedding_model: true"),
        "an EMB retag removed the operator's alias: {kept}"
    );

    // A non-embedding model retagged: its file changes only in the category line.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"plain","category":"AUX"}"#,
    )
    .await;
    assert!(status.is_success(), "status: {status} {body}");
    assert_eq!(
        std::fs::read_to_string(&plain_file).expect("read back"),
        plain_before.replace("category: WORK", "category: AUX"),
        "a non-embedding model's file must change only where it was asked"
    );
}

#[tokio::test]
async fn retagging_a_bom_prefixed_embedding_model_keeps_it_in_the_catalog() {
    // The falsifier for the BOM-blind key matcher: writeback's key matcher
    // did not strip a UTF-8 BOM while the scan parser does, so on a file whose
    // BOM'd FIRST line is `is_embedding_model: true` the batch retag treated the
    // removal as a no-op, published `category: WORK` beside the surviving alias,
    // answered `{"status":"edited"}` — and the next scan dropped the model with
    // EmbeddingCategoryConflict. Pre-fix this fails: the alias line survives the
    // retag and the rescan reports models=2 with the conflict naming the file.
    let (control, _, _, _, _, modelconf) = app(
        config_with_existing_llama("retag-bom"),
        &[("plain", "mock", CategoryId::Work, Some(4.0))],
    );
    let bom_file = modelconf.join("gemma-bom.yaml");
    std::fs::write(
        &bom_file,
        "\u{feff}is_embedding_model: true\nname: gemma-bom\nengine: mock\ncategory: EMB\nvram_gb: 2\n",
    )
    .expect("seed BOM'd embedding fixture");

    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["models"], 2, "all fixtures catalogued: {body}");

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"gemma-bom","category":"WORK"}"#,
    )
    .await;
    assert!(
        status.is_success(),
        "retag must answer 2xx: {status} {body}"
    );

    let edited = std::fs::read_to_string(&bom_file).expect("read back");
    assert!(
        !edited.contains("is_embedding_model"),
        "the alias on the BOM'd first line survives the retag: {edited}"
    );
    assert!(edited.contains("category: WORK"), "{edited}");
    assert_eq!(
        edited.matches("category:").count(),
        1,
        "exactly one category line: {edited}"
    );

    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["models"], 2,
        "the retagged model must survive the scan: {body}"
    );
    assert!(
        body["catalog_errors"].as_array().is_some_and(Vec::is_empty),
        "no conflict after the retag: {body}"
    );
    let (_, models) = get_json(&control, "/har/models").await;
    let entry = models["data"]
        .as_array()
        .expect("list")
        .iter()
        .find(|m| m["id"] == "gemma-bom")
        .expect("model dropped by the scan after a retag answered success");
    assert_eq!(entry["category"], "WORK");
}

/// Two yamls whose `model:` paths share one GGUF basename in different dirs —
/// the `mid/X.gguf` vs `big/X.gguf` shape. Same-name
/// twins became unconstructable with file-stem identity, so the
/// surviving ambiguity class is the alias tier of `catalog::resolve`: neither file
/// may win the shared GGUF name. The files carry no `name:` key at all.
fn seed_alias_twins(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let work = dir.join("twin-work.yaml");
    let aux = dir.join("twin-aux.yaml");
    std::fs::write(
        &work,
        "engine: mock\ncategory: WORK\nvram_gb: 4\nmodel: models/mid/twin.gguf\n",
    )
    .expect("seed twin-work");
    std::fs::write(
        &aux,
        "engine: mock\ncategory: AUX\nvram_gb: 4\nmodel: models/big/twin.gguf\n",
    )
    .expect("seed twin-aux");
    (work, aux)
}

async fn ambiguous_app() -> (
    Router,
    Router,
    Router,
    Router,
    MockRuntime,
    std::path::PathBuf,
) {
    let routers = app(
        AppConfig::default(),
        &[("filler", "mock", CategoryId::Work, Some(4.0))],
    );
    seed_alias_twins(&routers.5);
    // The twins fixture gets a REAL config file on disk (the `app()`
    // sandbox path), so the "…and not_persisted" test can assert the claim its name
    // makes — the refused 409 left the operator's config bytes untouched — instead of
    // only reading the in-memory `/har/status` categories.
    std::fs::write(
        routers.5.join(TEST_CONFIG_NAME),
        r#"{"seeded": "pre-existing operator bytes"}"#,
    )
    .expect("seed config file");
    let (status, _) = post_json(&routers.0, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "both twins must be in the catalog");
    routers
}

#[tokio::test]
async fn load_of_an_ambiguous_gguf_alias_is_409_naming_both_yamls() {
    // Ambiguity survives stem identity on the alias tier: before the fix
    // this answered 200 having spawned whichever quantization sorted first, under
    // the identity the client asked for.
    let (control, _, _, _, runtime, _) = ambiguous_app().await;
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"twin"}"#).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(body["code"], "conflict", "flat code: {body}");
    assert_eq!(body["error"]["code"], "conflict", "envelope code: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    // A bare `contains('2')` was satisfiable by a digit in the fixture
    // temp path the same message embeds, so a count-free regression still passed.
    // Pin the operator-visible phrase the wire actually renders.
    assert!(
        message.contains("ambiguous")
            && message.contains("`twin`")
            && message.contains("2 catalog entries"),
        "names the query and the entry count: {message}"
    );
    assert!(
        message.contains("twin-work.yaml") && message.contains("twin-aux.yaml"),
        "the operator must see BOTH colliding files, not the one that would have won: {message}"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "an ambiguous alias spawns nothing"
    );
}

#[tokio::test]
async fn named_proxy_on_an_ambiguous_gguf_alias_is_the_same_409_not_a_misdirected_404() {
    // The twins straddle WORK and AUX: the old first-match-wins answer depended on
    // which yaml sorted first, and a wrong-category hit 404'd naming the OTHER
    // entry's public port. The client cannot fix that, so it is a 409.
    let (control, work, _, _, runtime, _) = ambiguous_app().await;
    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"model":"twin"}"#).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(body["code"], "conflict");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("twin-work.yaml") && message.contains("twin-aux.yaml"),
        "both files named: {message}"
    );
    assert!(
        !message.contains("not found"),
        "never a misdirection: {message}"
    );
    assert_eq!(
        runtime.server_count(),
        0,
        "an ambiguous alias ensures nothing"
    );

    // A no-`model` request never resolves a name, so the twins cannot affect it.
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"filler"}"#).await;
    assert_eq!(status, StatusCode::OK, "the unique model loads: {body}");
    let (status, body) = post_json(&work, "/v1/chat/completions", r#"{"messages":[]}"#).await;
    assert_eq!(status, StatusCode::OK, "no-model is unaffected: {body}");
}

#[tokio::test]
async fn a_name_contradicting_the_stem_is_dropped_at_scan_and_the_first_file_loads_by_stem() {
    // The shared-name-handoff class, relocated by file-stem identity:
    // a second file carrying the FIRST file's name can no
    // longer reach the catalog at all — the scan drops it with the InvalidName
    // mismatch, and the twin shape never arrives at the API to be picked.
    let (control, _, _, _, runtime, dir) = app(
        AppConfig::default(),
        &[("filler", "mock", CategoryId::Work, Some(4.0))],
    );
    std::fs::write(
        dir.join("twin-work.yaml"),
        "engine: mock\ncategory: WORK\nvram_gb: 4\nmodel: models/mid/twin-q4.gguf\n",
    )
    .expect("seed twin-work");
    std::fs::write(
        dir.join("twin-aux.yaml"),
        "name: twin-work\nengine: mock\ncategory: AUX\nvram_gb: 4\nmodel: models/big/twin-q8.gguf\n",
    )
    .expect("seed name-twin");
    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["models"], 2,
        "filler + the stem-named file only: {body}"
    );
    assert!(
        body["catalog_errors"]
            .as_array()
            .expect("errors")
            .iter()
            .any(|e| e["kind"] == "invalid_name"
                && e["file"]
                    .as_str()
                    .is_some_and(|f| f.contains("twin-aux.yaml"))
                && e["reason"]
                    == "name: \"twin-work\" does not match file twin-aux.yaml \u{2014} identity \
                        is the file stem; rename the file or delete the key"),
        "the second file must be dropped with the exact mismatch reason: {body}"
    );

    // The first file stays individually addressable by its stem, and the wrong
    // twin is gone rather than waiting behind a 409.
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"twin-work"}"#).await;
    assert_eq!(status, StatusCode::OK, "the stem loads: {body}");
    assert_eq!(runtime.server_count(), 1, "exactly one engine");
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"twin-aux"}"#).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the dropped twin is unknown: {body}"
    );
}

#[tokio::test]
async fn colliding_gguf_file_names_are_409_on_the_alias_tier() {
    // Reviewer's example: distinct stems, one shared gguf file name across two
    // directories, so the alias tier is claimed twice.
    let (control, _, _, _, _, dir) = app(
        AppConfig::default(),
        &[("filler", "mock", CategoryId::Work, Some(4.0))],
    );
    let mid = dir.join("qwen3-mid.yaml");
    let big = dir.join("qwen3-big.yaml");
    std::fs::write(
        &mid,
        "engine: mock\ncategory: WORK\nvram_gb: 4\nmodel: models/mid/Qwen3-0.6B-Q8_0.gguf\n",
    )
    .expect("seed mid");
    std::fs::write(
        &big,
        "engine: mock\ncategory: WORK\nvram_gb: 4\nmodel: models/big/Qwen3-0.6B-Q8_0.gguf\n",
    )
    .expect("seed big");
    post_json(&control, "/har/rescan", "{}").await;

    for query in ["Qwen3-0.6B-Q8_0.gguf", "Qwen3-0.6B-Q8_0"] {
        let body = format!(r#"{{"model":"{query}"}}"#);
        let (status, reply) = post_json(&control, "/har/load", &body).await;
        assert_eq!(status, StatusCode::CONFLICT, "`{query}` body: {reply}");
        let message = reply["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("qwen3-mid.yaml") && message.contains("qwen3-big.yaml"),
            "`{query}` names both quantizations: {message}"
        );
    }

    // Each entry stays individually addressable by its own harness name.
    let (status, reply) = post_json(&control, "/har/load", r#"{"model":"qwen3-big"}"#).await;
    assert_eq!(status, StatusCode::OK, "a unique name still loads: {reply}");
}

#[tokio::test]
async fn setting_the_default_model_on_an_ambiguous_alias_is_refused_and_not_persisted() {
    // The write-back half of the alias finding: `/har/config/default` persists the name it
    // validated, so an ambiguity must stop BEFORE `persist_default_model`.
    // The name promised a file-level claim the body never checked —
    // it read only the in-memory `/har/status` categories, and a regression that
    // answered 409 *and still rewrote the config file* passed. The fixture now owns a
    // seeded config file in the test's temp dir; the refusal must leave its bytes
    // untouched.
    let (control, _, _, _, _, dir) = ambiguous_app().await;
    let config_path = dir.join(TEST_CONFIG_NAME);
    let before = std::fs::read_to_string(&config_path).expect("seeded config exists");

    let (status, body) = post_json(
        &control,
        "/har/config/default",
        r#"{"category":"WORK","model":"twin"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("twin-work.yaml") && message.contains("twin-aux.yaml"),
        "both files named: {message}"
    );
    let (_, status_body) = get_json(&control, "/har/status").await;
    let work_cat = status_body["categories"]
        .as_array()
        .expect("categories")
        .iter()
        .find(|cat| cat["id"] == "WORK")
        .expect("WORK");
    assert!(
        work_cat["default_model"].is_null(),
        "an ambiguous alias never becomes a category default: {status_body}"
    );
    // The file half of the claim (the pre-fix hole: an in-memory-only refusal test
    // passed while the endpoint rewrote the operator's config).
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("config file must still exist"),
        before,
        "a refused (409) default-set must leave the config file byte-identical"
    );
}

#[tokio::test]
async fn editing_an_ambiguous_alias_refuses_the_writeback_rather_than_picking_a_victim_file() {
    // `/har/config/model` resolves a name to choose WHICH yaml to edit.
    // Under ambiguity it must refuse; editing the first match silently rewrites one
    // operator file for a model the client did not necessarily name.
    let (control, _, _, _, _, dir) = ambiguous_app().await;
    let (work_file, aux_file) = (
        std::fs::read_to_string(dir.join("twin-work.yaml")).expect("read twin-work"),
        std::fs::read_to_string(dir.join("twin-aux.yaml")).expect("read twin-aux"),
    );

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"twin","cpu":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(body["code"], "conflict");
    assert_eq!(
        std::fs::read_to_string(dir.join("twin-work.yaml")).expect("read twin-work"),
        work_file,
        "the refused write must leave twin-work.yaml byte-identical"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("twin-aux.yaml")).expect("read twin-aux"),
        aux_file,
        "the refused write must leave twin-aux.yaml byte-identical"
    );
}

#[tokio::test]
async fn retagging_one_loaded_aux_model_never_touches_the_other_aux_occupant() {
    // `/har/config/model` is a single-model edit, but its retag branch
    // called `scheduler.unload(old_CATEGORY)` — "unloads every occupant of `category`".
    // With AUX at max_models 2..6, retagging one AUX model killed the others: in-flight
    // generations lost, VRAM dumped, cold reload for the operator's next request.
    let (control, _, _, aux_router, runtime, _) = app(
        aux_slots(2),
        &[
            ("aux1", "mock", CategoryId::Aux, Some(3.0)),
            ("aux2", "mock", CategoryId::Aux, Some(3.0)),
        ],
    );
    for name in ["aux1", "aux2"] {
        let body = format!(r#"{{"model":"{name}"}}"#);
        let (status, body) = post_json(&control, "/har/load", &body).await;
        assert_eq!(status, StatusCode::OK, "load {name}: {body}");
    }
    assert_eq!(await_server_count(&runtime, 2).await, 2, "both slots serve");

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"aux1","category":"WORK"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a retag never 500s: {body}");
    assert_eq!(body["status"], "edited", "response shape kept: {body}");
    assert_eq!(body["model"], "aux1", "response shape kept: {body}");
    assert_eq!(body["category"], "WORK", "response shape kept: {body}");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let (_, loaded) = get_json(&control, "/har/status").await;
        let slots = loaded["slots"].as_array().expect("slots");
        let survivor = slots
            .iter()
            .any(|slot| slot["model"] == "aux2" && slot["category"] == "AUX");
        let moved_away = !slots.iter().any(|slot| slot["model"] == "aux1");
        if survivor && moved_away {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("aux2 must keep its AUX slot while aux1's leaves it: {loaded}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(
        await_server_count(&runtime, 1).await,
        1,
        "only the retagged model's engine may die"
    );
    assert_eq!(runtime.spawn_count(), 2, "the survivor was never restarted");

    // The surviving engine is really alive, not merely listed: its listener answers.
    let (status, body) = post_json(
        &aux_router,
        "/v1/chat/completions",
        r#"{"model":"aux2","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the untouched AUX serves: {body}");
    assert_eq!(body["echo"], true);
}

#[tokio::test]
async fn retagging_an_unloaded_aux_model_leaves_the_loaded_aux_occupant_alive() {
    // The second half of the collateral-unload defect: the collateral unload fired on the CATEGORY CHANGE,
    // not on the model being loaded, so retagging a third AUX model that was never
    // loaded wiped the AUX occupants that were serving "for nothing".
    // The old blind 150 ms sleep is gone. aux1 is a `mock-hold-kill`
    // engine whose kill is held BY TARGET (`hold_kill_of`), so a collateral
    // `unload(AUX)` can neither succeed (that engine cannot die) nor slip past an
    // unmeasured window unnoticed: its kill parks on the runtime gate forever
    // (this test never releases it) and stands in `held_kills()` as permanent,
    // observable evidence of the wrong kill. The settle is positive: `slot_freed`
    // answers only once the actor has processed the retag's follow-up command, and
    // the `/har/rescan` round-trip drains the single-writer actor once more.
    let adapters: Vec<(&str, Arc<dyn EngineAdapter>)> =
        vec![("mock-hold-kill", Arc::new(MockAdapter))];
    let (control, _, _, aux_router, runtime, _) = build_app(
        aux_slots(2),
        &[
            ("aux1", "mock-hold-kill", CategoryId::Aux, Some(3.0)),
            ("aux2", "mock", CategoryId::Aux, Some(3.0)),
            ("aux3", "mock", CategoryId::Aux, Some(3.0)),
        ],
        &adapters,
    );
    let (status, body) = post_json(&control, "/har/load", r#"{"model":"aux1"}"#).await;
    assert_eq!(status, StatusCode::OK, "load aux1: {body}");
    assert_eq!(await_server_count(&runtime, 1).await, 1, "one occupant");

    let (_, loaded) = get_json(&control, "/har/status").await;
    let aux1_port = u16::try_from(
        loaded["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["model"] == "aux1")
            .and_then(|slot| slot["port"].as_u64())
            .expect("the loaded AUX reports its engine port"),
    )
    .expect("port fits u16");
    // Hold the ONE thing a wrong retag could kill: by-target holding means the
    // fake's parking cannot be re-armed or dodged by a change in how many kills
    // the product issues (the by-target-hold seam).
    runtime.hold_kill_of(aux1_port);

    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"aux3","category":"WORK"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a retag never 500s: {body}");
    assert_eq!(body["status"], "edited");
    assert_eq!(body["category"], "WORK");
    // The positive settle signal: `slot_freed` is the actor's reply to
    // the retag's follow-up command (`unload_model` - the idempotent Done the
    // actor guarantees for a never-loaded model), so the retag path is proven
    // PROCESSED, not merely posted. The broken collateral-unload branch answered its
    // `unload(old_category)` at this same point.
    assert_eq!(
        body["slot_freed"],
        serde_json::json!(true),
        "the retag's follow-up ran through the actor, so the retag path is proven \
         processed before the negative below: {body}"
    );

    // One more full round-trip through the single-writer actor: its reply cannot
    // arrive before the actor has dequeued every command the retag queued ahead of
    // it, and the awaits around it give a collateral unload's spawned kill task
    // its polls. No timed sleep stands in for that ordering.
    let (status, body) = post_json(&control, "/har/rescan", r#"{}"#).await;
    assert_eq!(status, StatusCode::OK, "rescan round-trip: {body}");

    // The negative is now structural, not timed: had the retag collateral-unloaded
    // AUX, aux1's kill would have parked on the held gate and stayed there - so
    // this proves an arrival would be detected, whenever it comes.
    assert!(
        runtime.held_kills().is_empty(),
        "R2-18-03 falsifier: a collateral `unload(AUX)` issued a kill of the loaded \
         aux1 and it is parked on the held gate: {:?}",
        runtime.held_kills()
    );
    let (_, loaded) = get_json(&control, "/har/status").await;
    let slots = loaded["slots"].as_array().expect("slots");
    assert!(
        slots
            .iter()
            .any(|slot| slot["model"] == "aux1" && slot["category"] == "AUX"),
        "an edit to aux3 must not disturb the loaded aux1: {loaded}"
    );
    assert_eq!(
        runtime.server_count(),
        1,
        "the never-loaded retag kills nothing"
    );
    assert_eq!(runtime.spawn_count(), 1, "and restarts nothing");

    let (status, body) = post_json(
        &aux_router,
        "/v1/chat/completions",
        r#"{"model":"aux1","messages":[]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "aux1 is still answering: {body}");
    // The permanent evidence one last time: the legs above gave the wrong kill -
    // had the product issued one - every remaining poll; none arrived and parked.
    assert!(
        runtime.held_kills().is_empty(),
        "no kill of aux1 ever arrived to park on the held gate: {:?}",
        runtime.held_kills()
    );
}

/// The boot config with `WORK`'s default pointed at `model`, the shape the retag
/// dangling-default clear acts on (operator finding on default-model coherence).
fn config_defaulting_work(model: &str) -> AppConfig {
    let mut config = AppConfig::default();
    config
        .categories
        .iter_mut()
        .find(|c| c.id == CategoryId::Work)
        .expect("WORK category")
        .default_model = Some(model.to_string());
    config
}

#[tokio::test]
async fn the_retag_reply_surfaces_the_slot_and_default_follow_ups() {
    // The retag branch once swallowed the slot-free (`let _ =`) and the
    // dangling-default clear (the dropped `(effective, persisted)` tuple), so a
    // default-clear whose write failed still answered a bare `"status":"edited"`
    // with no field, log contrast, or warning for either failure. The 200 stays an
    // acceptance of the yaml edit; the reply now names each follow-up's outcome,
    // with `null` marking one that was not attempted.
    let specs = [("qwen3", "mock", CategoryId::Work, Some(4.0))];

    // Leg 1 — both follow-ups land: the clear reaches the file (build_app stamps
    // source_path inside the test's own modelconf dir).
    let (control, _, _, _, _, modelconf) = build_app(config_defaulting_work("qwen3"), &specs, &[]);
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a retag never 500s: {body}");
    assert_eq!(body["status"], "edited", "acceptance contract kept: {body}");
    assert_eq!(
        body["slot_freed"],
        serde_json::json!(true),
        "the own-slot free ran (idempotent Done for a never-loaded model): {body}"
    );
    assert_eq!(
        body["default_cleared"],
        serde_json::json!(true),
        "the dangling WORK default was cleared and persisted: {body}"
    );
    assert!(
        body["warnings"].as_array().is_some_and(Vec::is_empty),
        "a clean retag carries no warnings: {body}"
    );
    let written: Value = serde_json::from_str(
        &std::fs::read_to_string(modelconf.join(TEST_CONFIG_NAME))
            .expect("the default-clear wrote the config file"),
    )
    .expect("the written config file is JSON");
    let work = written["categories"]
        .as_array()
        .expect("categories array")
        .iter()
        .find(|c| c["id"] == "WORK")
        .expect("WORK entry");
    assert_eq!(
        work["default_model"],
        Value::Null,
        "the file no longer points WORK at the moved model: {written}"
    );

    // A non-retag edit attempts neither follow-up: both fields stay null.
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "same-category edit: {body}");
    assert_eq!(body["slot_freed"], Value::Null, "not attempted: {body}");
    assert_eq!(
        body["default_cleared"],
        Value::Null,
        "not attempted: {body}"
    );

    // Leg 2 — the clear cannot persist: a hand-stamped source_path in a missing
    // directory makes the gated write fail (disk-full/locked-file stand-in). The
    // 200 and `"edited"` stay — the yaml edit is honestly done — but the reply says
    // the file still names the moved model, and memory never moves either: a
    // clear whose bytes never landed leaves the effective default at the value
    // the file still holds (config file is king). The earlier pin asserted memory
    // took the clear anyway; the operator ruling retires that memory-beats-file
    // shape, so this pin now demands the opposite.
    let mut broken = config_defaulting_work("qwen3");
    let unwriteable = simpleharn::testutil::temp_dir("api-retag-nopersist")
        .join("gone")
        .join(TEST_CONFIG_NAME);
    broken.source_path = Some(unwriteable.clone());
    let (control, _, _, _, _, _) = build_app(broken, &specs, &[]);
    let (status, body) = post_json(
        &control,
        "/har/config/model",
        r#"{"model":"qwen3","category":"AUX"}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "acceptance is unchanged by a failed follow-up: {body}"
    );
    assert_eq!(body["status"], "edited", "acceptance contract kept: {body}");
    assert_eq!(
        body["default_cleared"],
        serde_json::json!(false),
        "the swallowed failure is now an answered false: {body}"
    );
    assert!(
        body["warnings"]
            .as_array()
            .expect("warnings stays an array")
            .iter()
            .any(|w| w.as_str().is_some_and(|w| w.contains("default"))),
        "the swallowed failure must surface as a warning naming the default: {body}"
    );
    assert!(
        !unwriteable.exists(),
        "a failed persist may not leave a file behind"
    );
    let (_, doc) = get_json(&control, "/har/status").await;
    let work = doc["categories"]
        .as_array()
        .expect("categories array")
        .iter()
        .find(|c| c["id"] == "WORK")
        .expect("WORK entry");
    assert_eq!(
        work["default_model"],
        serde_json::json!("qwen3"),
        "memory keeps answering the default the file still holds — a clear that \
         never landed never changes memory (R2-07-03): {doc}"
    );
}

#[tokio::test]
async fn an_unknown_key_warns_on_the_page_and_the_model_still_loads() {
    // The warning is real (visible in `catalog_errors`, `kind`
    // round-tripping through the JSON) and harmless (README doctrine: preserved
    // for reference — the entry publishes and loads).
    let (control, _, _, _, runtime, dir) = app(
        AppConfig::default(),
        &[("filler", "mock", CategoryId::Work, Some(4.0))],
    );
    std::fs::write(
        dir.join("typo.yaml"),
        "engine: mock\ncategory: WORK\nvram_gb: 4\ncontex: 4096\nhost: 0.0.0.0\nport: 8123\n",
    )
    .expect("seed typo fixture");
    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let errors = body["catalog_errors"].as_array().expect("errors");
    let typo = errors
        .iter()
        .find(|e| {
            e["kind"] == "unknown_key" && e["reason"].as_str().is_some_and(|r| r.contains("contex"))
        })
        .unwrap_or_else(|| panic!("the typo must be reported: {body}"));
    assert!(
        typo["reason"]
            .as_str()
            .is_some_and(|r| r.contains("(did you mean 'context'?)")),
        "one unique edit away: {typo}"
    );
    assert!(
        errors.iter().any(|e| e["kind"] == "unknown_key"
            && e["reason"].as_str()
                == Some("engine host is harness-forced (loopback, \u{a7}5) \u{2014} this key does nothing")),
        "the host policy hint must be exact: {body}"
    );
    assert!(
        errors.iter().any(|e| e["kind"] == "unknown_key"
            && e["reason"].as_str()
                == Some("engine port is harness-assigned from category config \u{2014} this key does nothing")),
        "the port policy hint must be exact: {body}"
    );

    let (status, body) = post_json(&control, "/har/load", r#"{"model":"typo"}"#).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a warning never unmakes a model: {body}"
    );
    assert_eq!(runtime.server_count(), 1);
}

#[tokio::test]
async fn a_bogus_llama_server_exe_is_a_page_visible_engine_binary_missing_error() {
    // Cross-check twin of the boot banner: the missing binary
    // is not only said once at boot, it is IN `catalog_errors` with its kind on
    // the wire (file `config`, kind `engine_binary_missing`).
    let config = AppConfig {
        llama_server_exe: Some(std::path::PathBuf::from(
            "Z:/definitely-not-here/llama-server.exe",
        )),
        ..AppConfig::default()
    };
    let (control, _, _, _, _, _) = app(config, &[("qwen3", "mock", CategoryId::Work, Some(4.0))]);
    let (status, body) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let error = body["catalog_errors"]
        .as_array()
        .expect("errors")
        .iter()
        .find(|e| e["kind"] == "engine_binary_missing")
        .unwrap_or_else(|| panic!("the missing binary must be page-visible: {body}"));
    assert_eq!(error["file"], "config");
    let reason = error["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("llama-server.exe not found at Z:/definitely-not-here/llama-server.exe")
            && reason.contains("without a row `llama_server_exe` will fail"),
        "{reason}"
    );
}

/// The unset-binary posture through the whole surface: an UNSET `llama_server_exe` is a
/// posture, not a fault — no `engine_binary_missing` scan error (the scan has
/// no verdict to hand), and `/har/status` names `llama_server_exe_unset` in
/// `config_warnings` while a loopback bind owes no LAN warning. Falsifier:
/// re-materialize a hidden default and the unset leg goes red; report a
/// missing-binary error for `None` and the error leg goes red.
#[tokio::test]
async fn an_unset_llama_binary_warns_on_the_page_and_never_fakes_a_scan_error() {
    let (control, _, _, _, _, _) = app(
        AppConfig::default(),
        &[("qwen3", "mock", CategoryId::Work, Some(4.0))],
    );
    let (status, body) = get_json(&control, "/har/status").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body["catalog_errors"]
            .as_array()
            .expect("errors")
            .iter()
            .all(|e| e["kind"] != "engine_binary_missing"),
        "no configured binary is no verdict: {body}"
    );
    let codes: Vec<&str> = body["config_warnings"]
        .as_array()
        .expect("config_warnings is always an array")
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert!(
        codes.contains(&"llama_server_exe_unset"),
        "the unset posture is page-visible: {body}"
    );
    assert!(
        !codes.contains(&"lan_open_without_token"),
        "a loopback bind owes no LAN warning: {body}"
    );
}

#[tokio::test]
async fn a_missing_gguf_is_dropped_by_the_scan_and_named_on_the_page() {
    // The missing-file posture through the API surface: a llama entry pointed at a GGUF
    // that is not on disk never reaches the catalog (so it cannot evict anything),
    // and `/har/rescan` names the file and the path with its kind.
    let (control, _, _, _, _, dir) = app(
        AppConfig::default(),
        &[("filler", "mock", CategoryId::Work, Some(4.0))],
    );
    std::fs::write(
        dir.join("doomed.yaml"),
        "engine: llama\nmodel: Z:/definitely-not-here/doomed.gguf\nvram_gb: 4\n",
    )
    .expect("seed doomed");
    let (status, body) = post_json(&control, "/har/rescan", "{}").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["models"], 1, "only the mock filler survived: {body}");
    let error = body["catalog_errors"]
        .as_array()
        .expect("errors")
        .iter()
        .find(|e| e["kind"] == "model_path_missing")
        .unwrap_or_else(|| panic!("the missing file must be named: {body}"));
    assert!(error["file"]
        .as_str()
        .is_some_and(|f| f.contains("doomed.yaml")));
    assert!(
        error["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("model file not found: ") && r.contains("doomed.gguf")),
        "{}",
        error["reason"]
    );

    let (status, body) = post_json(&control, "/har/load", r#"{"model":"doomed"}"#).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a dropped entry is unknown: {body}"
    );
}
