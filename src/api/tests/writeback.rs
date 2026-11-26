use super::*;

/// Every writeback class has a decided HTTP class, and the
/// mapping re-words nothing. The two config-state classes additionally owe the
/// operator the *name of the file* that refused (the module's `Display`
/// deliberately carries none), so their reason is `"<file>: " + the module's
/// sentence` — a caller-side **prefix** with the sentence intact to the last
/// byte; every other class answers `reason == error.to_string()` exactly. The
/// split is asserted per row (`names_file`), so it cannot silently widen.
/// Falsifiers: send one variant back to `Internal` and its row fails; drop the
/// prefix and the two config rows fail; corrupt the sentence inside the
/// composition and the same two rows fail on the tail equality.
#[test]
fn writeback_errors_map_to_their_http_class_without_rewording() {
    let file = PathBuf::from("ModelConf/qwen3.yaml");
    let cases: Vec<(WritebackError, StatusCode, &'static str, bool)> = vec![
        (
            WritebackError::UnrepresentableValue {
                key: "name".to_string(),
                value: "\"a # b\"".to_string(),
            },
            StatusCode::BAD_REQUEST,
            "bad_request",
            false,
        ),
        (
            WritebackError::KeyNotInFile {
                key: "ghost".to_string(),
            },
            StatusCode::BAD_REQUEST,
            "bad_request",
            false,
        ),
        (
            WritebackError::WouldBreakFile {
                problems: vec!["line 2: not a `key: value` line: `nope`".to_string()],
            },
            StatusCode::CONFLICT,
            "conflict",
            true,
        ),
        (
            WritebackError::EmptyFile,
            StatusCode::CONFLICT,
            "conflict",
            true,
        ),
        (
            WritebackError::Unreadable {
                file: PathBuf::from("ModelConf/qwen3.yaml"),
                io: "os error 2".to_string(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            false,
        ),
        (
            WritebackError::WriteFailed {
                file: PathBuf::from("ModelConf/qwen3.yaml.1.2.sh.tmp"),
                io: "os error 28".to_string(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            false,
        ),
        (
            WritebackError::RenameFailed {
                file: PathBuf::from("ModelConf/qwen3.yaml"),
                io: "os error 5".to_string(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            false,
        ),
        (
            WritebackError::PostWriteValidation {
                detail: "RESTORE FAILED: io; original bytes are in qwen3.yaml.bak".to_string(),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            false,
        ),
    ];
    for (error, status, code, names_file) in cases {
        let text = error.to_string();
        let mapped = ApiError::from_writeback(&file, error);
        assert_eq!(mapped.status(), status, "{text}");
        assert_eq!(mapped.code(), code, "{text}");
        let reason = match &mapped {
            ApiError::BadRequest { reason }
            | ApiError::Conflict { reason }
            | ApiError::Internal { reason } => reason,
            other => panic!("writeback errors map to the three decided classes, got {other:?}"),
        };
        if names_file {
            assert_eq!(
                reason,
                &format!("{}: {text}", file.display()),
                "a config-state refusal must name the file that refused"
            );
            assert!(
                reason.ends_with(&text),
                "the path must be a prefix, never a re-word: {reason}"
            );
        } else {
            assert_eq!(reason, &text, "the mapping must not re-word the module");
        }
    }
}

#[test]
fn config_write_path_follows_boot_and_the_default_file() {
    // Write-back must land where boot reads. Comparing the WHOLE path is what makes
    // this bite: every fallback branch ends in CONFIG_FILE, so a filename-only
    // assertion would pass even if the default directory moved under it.
    let no_source = AppConfig {
        source_path: None,
        ..AppConfig::default()
    };
    let expected = crate::config::default_config_path().expect("test process has an exe path");
    assert_eq!(
        config_write_path(&no_source),
        expected,
        "with no stamped path, write to boot's own default location"
    );
    assert_eq!(
        config_write_path(&no_source).file_name(),
        Some(std::ffi::OsStr::new(crate::config::CONFIG_FILE)),
        "on the fallback, the harness writes only its one config file — never any \
             older spelling (TASK-NAMING-CLEANUP)"
    );

    let loaded = std::path::PathBuf::from(r"C:\Configs\lan.json");
    let stamped = AppConfig {
        source_path: Some(loaded.clone()),
        ..AppConfig::default()
    };
    assert_eq!(
        config_write_path(&stamped),
        loaded,
        "the file boot actually loaded always wins — config file is king, an explicit \
             SIMPLEHARN_CONFIG selection IS the write target (ruling 2026-11-03)"
    );
}

/// Structural net: the two control-handler write paths must
/// run their synchronous filesystem sequences on a blocking thread, never
/// inline on the async request worker. The order test is the whole pin: the
/// write call (and the re-baseline stat that follows it) must appear only
/// AFTER `tokio::task::spawn_blocking(` in the handler body, i.e. inside the
/// blocking leg, so a reactor thread can never park behind the yaml/config
/// read-patch-rename. Response honesty is a behavioural matter and stays
/// pinned where it lives (`config_persistence_is_answered_only_after_the_bytes_read_back`
/// and the `api_surface::config_writeback` `persisted` legs) — the handler
/// still awaits the leg's verdict before answering.
///
/// Falsifier: lift any of the three fs calls back out of its `spawn_blocking`
/// closure and the order assert fires first; delete the offload entirely and
/// the contains assert fires — both red on the pre-fix shape of this file.
/// Every occurrence of each token is checked, so a second inline call added
/// after the leg's `.await` trips the same assert.
#[test]
fn the_control_write_backs_run_on_a_blocking_thread() {
    let source = include_str!("../config_persist.rs");
    for (head, tail, write_calls) in [
        (
            "async fn set_default_model(",
            "default_overrides",
            vec!["persist_default_model("],
        ),
        (
            "async fn har_config_model(",
            "\"status\": \"edited\"",
            vec!["set_flags_on_file(", "crate::model::fingerprint("],
        ),
    ] {
        let start = source
            .find(head)
            .unwrap_or_else(|| panic!("{head} is still here"));
        let close = source[start..]
            .find(if head.contains("set_default_model") {
                "\n    }\n"
            } else {
                "\n}\n"
            })
            .unwrap_or_else(|| panic!("{head} closes where expected"));
        let body = &source[start..start + close];
        assert!(
            body.contains(tail),
            "the scan slice must reach {head}'s tail (early-close fragility)"
        );
        let offload = body
            .find("tokio::task::spawn_blocking(")
            .unwrap_or_else(|| {
                panic!("`{head}` must run its sync fs on a blocking thread (FINDINGS-07 [8])")
            });
        let joined = offload
            + body[offload..]
                .find(".await")
                .unwrap_or_else(|| panic!("`{head}`'s blocking leg is never joined"));
        for call in write_calls {
            assert!(body.contains(call), "{head} must still perform `{call}`");
            for (at, _) in body.match_indices(call) {
                assert!(
                    offload < at && at < joined,
                    "`{head}` reaches `{call}` outside its `spawn_blocking` leg — \
                         the fs call is inline on the request thread again \
                         (FINDINGS-07 [8])"
                );
            }
        }
    }
}

/// `rename()` returning `Ok` is the OS accepting the move, not
/// proof the bytes landed, and this is the one file the harness re-reads at boot
/// under `deny_unknown_fields` — a silent-short config write is a harness that will
/// not start. `persisted: true` is therefore earned by reading the file back and
/// requiring it to parse to the tree that was published.
///
/// The false leg goes through the **real** comparison: the test arm corrupts the
/// on-disk bytes inside the verify window (valid JSON, one extra key), so it is
/// production's `read_back == root` that refuses the answer.
/// Falsifiers, both caught: delete the
/// verification block and the armed leg answers `persisted: true` for bytes nobody
/// checked; or replace the comparison with an always-true `Ok(_) => Ok(())` and the
/// same leg fails while every other pin stays green — which is exactly the
/// rubber-stamp the seam used to be unable to detect. The leg's outcome is
/// `LandedUnverified` with the verdict of the LANDED bytes: the rename
/// succeeded, so the live file holds them and `/har/status` must describe them.
#[test]
fn config_persistence_is_answered_only_after_the_bytes_read_back() {
    let dir = crate::testutil::temp_dir("api-config-readback");
    let path = dir.join("simpleharn.json");
    let config = AppConfig {
        source_path: Some(path.clone()),
        ..AppConfig::default()
    };

    let (persisted, verdict) =
        match persist_default_model(&config, CategoryId::Work, Some("honest.gguf")) {
            PersistOutcome::Landed { verdict } => (true, verdict),
            other => {
                panic!("a write that reads back as published must report Landed, never {other:?}")
            }
        };
    assert!(
        persisted,
        "a write that reads back as published is a persistence the API may claim"
    );
    assert!(
        verdict.is_empty(),
        "clean published bytes earn an empty boot-config verdict: {verdict:?}"
    );
    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the config file exists after a true answer"),
    )
    .expect("temp + rename never expose a partial file");
    assert_eq!(
        on_disk["categories"]
            .as_array()
            .expect("categories array")
            .iter()
            .find(|entry| entry["id"].as_str() == Some(CategoryId::Work.to_string().as_str()))
            .and_then(|entry| entry["default_model"].as_str()),
        Some("honest.gguf"),
        "and the answer means the field is on disk, not merely in memory: {on_disk}"
    );

    // The volume accepted the rename and the bytes are there — they are simply not
    // the tree that was published, and only the real compare can tell.
    arm_config_read_back_fault(&path);
    let outcome = persist_default_model(&config, CategoryId::Work, Some("phantom.gguf"));
    let (persisted, verdict) = match outcome {
        PersistOutcome::LandedUnverified { verdict } => (false, verdict),
        other => panic!(
            "a rename that landed without read-back confirmation must report \
                 LandedUnverified, never {other:?}"
        ),
    };
    assert!(
        !persisted,
        "an unverifiable write may only ever be answered `persisted: false`"
    );
    let named = verdict
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        named.contains(crate::config::CONFIG_FILE) && named.contains("tampered"),
        "the rename landed, so the live file holds these bytes: the outcome must \
             carry what THEY say the next boot would refuse (file root + landed key), \
             not an empty verdict that lets `/har/status` keep asserting the previous \
             one (R2-07-02): {named}"
    );
    release_config_read_back_fault(&path);
    let tampered: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the file is present, merely unverified"),
    )
    .expect("the corruption is different-but-valid, not garbage");
    assert_eq!(
        tampered["tampered"],
        Value::Bool(true),
        "the leg has to be reached by genuinely-diverging bytes, or an always-true \
             compare would sail through it: {tampered}"
    );
    assert_eq!(
        tampered["categories"]
            .as_array()
            .expect("categories array")
            .iter()
            .find(|entry| entry["id"].as_str() == Some(CategoryId::Work.to_string().as_str()))
            .and_then(|entry| entry["default_model"].as_str()),
        Some("phantom.gguf"),
        "the refused write did land — `persisted: false` is a verdict about \
             verification, not about whether the rename worked: {tampered}"
    );
    let backup: Value = serde_json::from_str(
        &std::fs::read_to_string(path.with_extension("json.bak"))
            .expect("the pre-write copy is kept for exactly this case"),
    )
    .expect("the backup is a config file");
    assert_eq!(
        backup["categories"]
            .as_array()
            .expect("categories array")
            .iter()
            .filter(|entry| entry["default_model"].as_str() == Some("honest.gguf"))
            .count(),
        1,
        "the last value the harness can vouch for is still recoverable: {backup}"
    );
}

/// The flush-before-publish tripwire — **a pin by inspection, not a
/// power-loss falsifier** (the same honesty label `model::writeback` puts on
/// its own tripwire: a crash rig is card-window material).
/// What it holds is the shape the durability line is made of:
/// [`persist_default_model`] — the only runtime writer of the file the harness
/// re-reads at boot under `deny_unknown_fields` — publishes its temp and its
/// `.json.bak` copy through the same flushed `write_synced` door as the yaml
/// publisher and the first-run config writer, and reaches no cache-only
/// create/copy primitive in the body. Routing either publish back through the
/// cache-only primitive turns this red; that is exactly the shape in which a
/// rename could expose bytes the volume was never told to commit while the
/// last API answer said `persisted: true`. Comments are stripped before the
/// match, so prose cannot satisfy or fake the asserts.
#[test]
fn the_boot_config_publish_flushes_through_write_synced() {
    let source = include_str!("../config_persist.rs");
    let start = source
        .find("fn persist_default_model(")
        .expect("persist_default_model is still here");
    let rest = &source[start..];
    let end = rest
        .find("\n}\n")
        .expect("persist_default_model closes at column 0");
    let body = rest[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("write_synced("),
        "the boot-config temp and its backup must publish through the flushed \
             write_synced door (R2-07-01)"
    );
    for banned in ["std::fs::write(", "std::fs::copy("] {
        assert!(
            !body.contains(banned),
            "the publish body must not reach the cache-only `{banned}` — a \
                 rename may never expose a file whose content the kernel merely \
                 buffered (R2-07-01)"
        );
    }
}

#[test]
fn concurrent_default_writes_never_lose_a_field_or_leave_a_temp() {
    // The config write-back was an UNGATED read-modify-write onto
    // one FIXED temp name, and two handlers reach it (`/har/config/default` and
    // `/har/config/model`'s dangling-default clear) on separate workers. Interleaved,
    // the second writer renamed its copy of the ORIGINAL, so the first writer's
    // `default_model` vanished from disk while both responses said `persisted: true`;
    // the shared temp added a second failure mode — one writer truncating the other's
    // tmp made its rename fail although the other's content had landed. Six threads
    // over the four categories, a fresh value per round: under the gate plus the
    // unique temp every write answers true and no category can fall back to its seed.
    const ROUNDS: usize = 24;
    let dir = crate::testutil::temp_dir("api-persist-race");
    let path = dir.join("simpleharn.json");
    let mut config = AppConfig::default();
    for category in &mut config.categories {
        category.default_model = Some(format!("seed-{}", category.id));
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&config).expect("seed serializes"),
    )
    .expect("seed config");
    config.source_path = Some(path.clone());

    let plan: [(CategoryId, &str); 6] = [
        (CategoryId::Work, "work"),
        (CategoryId::Work, "work"),
        (CategoryId::Rp, "rp"),
        (CategoryId::Rp, "rp"),
        (CategoryId::Aux, "aux"),
        (CategoryId::Emb, "emb"),
    ];
    let writers = plan
        .into_iter()
        .enumerate()
        .map(|(index, (category, prefix))| {
            let per_writer = config.clone();
            std::thread::spawn(move || {
                (0..ROUNDS)
                    .map(|round| {
                        let name = format!("{prefix}-t{index}-r{round}");
                        persist_default_model(&per_writer, category, Some(&name)).is_landed()
                    })
                    .collect::<Vec<bool>>()
            })
        })
        .collect::<Vec<_>>();
    let outcomes = writers
        .into_iter()
        .map(|writer| writer.join().expect("a writer thread never panics"))
        .collect::<Vec<_>>();
    assert!(
        outcomes.iter().flatten().all(|persisted| *persisted),
        "every writer must be able to answer honestly, not `persisted: true` \
             for bytes that a concurrent rename undid: {outcomes:?}"
    );

    let text = std::fs::read_to_string(&path).expect("the config file is always present");
    let on_disk: Value =
        serde_json::from_str(&text).expect("temp + rename never expose a partial file");
    let categories = on_disk["categories"].as_array().expect("categories array");
    let persisted_default = |category: CategoryId| -> String {
        let id = category.to_string();
        categories
            .iter()
            .find(|entry| entry["id"].as_str() == Some(id.as_str()))
            .and_then(|entry| entry["default_model"].as_str())
            .unwrap_or_else(|| panic!("category {id} lost its default_model: {on_disk}"))
            .to_string()
    };
    for &(category, prefix) in &plan {
        let value = persisted_default(category);
        assert!(
            value.starts_with(prefix),
            "{}'s default is a lost update — no writer ever named {value:?}: {on_disk}",
            category
        );
    }
    // A single-writer category is deterministic under the gate: every later commit
    // re-reads the committed file, so its last round can never be rolled back.
    assert_eq!(
        persisted_default(CategoryId::Aux),
        format!("aux-t4-r{}", ROUNDS - 1),
        "the last AUX write must still be on disk"
    );
    assert_eq!(
        persisted_default(CategoryId::Emb),
        format!("emb-t5-r{}", ROUNDS - 1),
        "the last EMB write must still be on disk"
    );

    let leftovers = std::fs::read_dir(&dir)
        .expect("read temp dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "a failed rename must not leave its temp behind: {leftovers:?}"
    );
}

/// Producer falsifier: the write-back is the one moment the
/// harness becomes the last writer of the boot config, and its read-patch-write
/// carries every OTHER byte as `Value` passthrough — so an operator hand-edit made
/// while the harness ran (reload re-reads the catalog and nothing else, LOCKED)
/// survives into the published file. Pre-fix, `persist_default_model`
/// answered a bare bool and `/har/status` hardcoded `config_errors: []`, so a
/// hand-edited `load_timeout_s: 0` (a boot-fatal field under the boot rule) rode the
/// harness's own `persisted: true` write unseen until the harness refused to
/// start. The verdict is the boot's own validator over the landed tree, so the
/// field can never drift from what boot would say.
#[test]
fn a_landed_write_back_validates_the_published_boot_file_per_field() {
    let dir = crate::testutil::temp_dir("api-config-verdict");
    let path = dir.join("simpleharn.json");
    let config = AppConfig {
        source_path: Some(path.clone()),
        ..AppConfig::default()
    };
    // Mid-run hand-edit: valid JSON, contents the boot must refuse.
    let mut root = serde_json::to_value(&config).expect("seed serializes");
    root["load_timeout_s"] = json!(0);
    root["vram_total_gb"] = json!(-1.0);
    std::fs::write(&path, root.to_string()).expect("seed the hand-edited file");

    let verdict = match persist_default_model(&config, CategoryId::Work, Some("pinned.gguf")) {
        PersistOutcome::Landed { verdict } => verdict,
        other => panic!(
            "the bytes landed and read back; persisted honesty is unchanged by \
                 the verdict, never {other:?}"
        ),
    };
    let named = verdict
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        named.contains("load_timeout_s") && named.contains("vram_total_gb"),
        "every boot-refused field must be named while the harness is alive to say it: {named}"
    );

    // A hand-edited unknown key no longer parses as the schema at all: one error
    // naming the file root, the same shape `AppConfig::parse` gives at boot.
    let mut root = serde_json::to_value(&config).expect("seed serializes");
    root["wrok_port"] = json!(9);
    std::fs::write(&path, root.to_string()).expect("seed the unknown key");
    let verdict = match persist_default_model(&config, CategoryId::Work, Some("pinned2.gguf")) {
        PersistOutcome::Landed { verdict } => verdict,
        other => panic!(
            "the unknown key rode the passthrough and landed as Landed, \
                 never {other:?}"
        ),
    };
    assert_eq!(verdict.len(), 1, "one parse-level error: {verdict:?}");
    let text = verdict[0].to_string();
    assert!(
        text.contains(crate::config::CONFIG_FILE) && text.contains("wrok_port"),
        "the error must name the file and the key, as boot's parse does: {text}"
    );
}

/// `label` is a KNOWN key of a category entry, so the
/// read-patch-write that persists a page `Default` edit carries it through as
/// `Value` passthrough and the landed tree still parses clean — while an
/// unknown key next to it keeps warning exactly as pinned.
/// Falsifier: remove the `label` field from the schema and the clean leg lands
/// a root-naming parse verdict, turning the empty-verdict leg red.
#[test]
fn a_persisted_label_survives_the_write_back_and_an_unknown_key_still_warns() {
    let dir = crate::testutil::temp_dir("api-label-persist");
    let path = dir.join("simpleharn.json");
    let config = AppConfig {
        source_path: Some(path.clone()),
        ..AppConfig::default()
    };
    let mut root = serde_json::to_value(&config).expect("seed serializes");
    root["categories"][0]["label"] = json!("Primary");
    std::fs::write(&path, root.to_string()).expect("seed the labeled file");

    match persist_default_model(&config, CategoryId::Work, Some("pinned.gguf")) {
        PersistOutcome::Landed { verdict } => assert!(
            verdict.is_empty(),
            "the known `label` key must not warn: {verdict:?}"
        ),
        other => panic!("the labeled file must land clean, never {other:?}"),
    }
    let on_disk: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the file the write-back published"),
    )
    .expect("the published file is JSON");
    assert_eq!(
        on_disk["categories"][0]["label"],
        json!("Primary"),
        "the label must survive the write-back"
    );
    assert_eq!(
        on_disk["categories"][0]["default_model"],
        json!("pinned.gguf"),
        "and the write-back still does its own job"
    );

    // An unknown key next to the label still warns as today (the label is not
    // the cause; the verdict names the unknown key).
    let mut root = serde_json::to_value(&config).expect("seed serializes");
    root["categories"][0]["label"] = json!("Primary");
    root["categories"][0]["wrok_port"] = json!(9);
    std::fs::write(&path, root.to_string()).expect("seed the unknown key");
    let verdict = match persist_default_model(&config, CategoryId::Work, Some("pinned2.gguf")) {
        PersistOutcome::Landed { verdict } => verdict,
        other => panic!(
            "the unknown key rode the passthrough and landed as Landed, \
                 never {other:?}"
        ),
    };
    assert_eq!(verdict.len(), 1, "one parse-level error: {verdict:?}");
    let text = verdict[0].to_string();
    assert!(
        text.contains(crate::config::CONFIG_FILE) && text.contains("wrok_port"),
        "the warning must name the file and the unknown key, never the known label: {text}"
    );
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the landed file"))
            .expect("the published file is JSON");
    assert_eq!(
        on_disk["categories"][0]["label"],
        json!("Primary"),
        "the label survives beside the warning"
    );
}

/// Render wiring: `config_errors` is the recorded verdict, in the
/// `kind`/`field`/`reason` vocabulary `catalog_errors` established, and an empty
/// array is earned (clean boot, clean later write) rather than asserted. Falsifier:
/// restoring the hardcoded `[]` leaves both non-empty legs red; flattening the
/// verdict into `last_error` (or into `catalog_errors`) is red on the shape leg.
#[tokio::test]
async fn config_errors_reports_the_recorded_verdict_and_starts_empty() {
    let api = live_state(BootTimings::default());
    let doc = status_document(&api).await.expect("status document");
    assert_eq!(
        doc["config_errors"],
        json!([]),
        "a boot that validated dirty never served this document: a fresh state is clean"
    );
    api.record_config_errors(vec![ConfigError::Invalid {
        field: "vram_total_gb".to_string(),
        reason: "must be a finite number greater than 0".to_string(),
    }]);
    let doc = status_document(&api).await.expect("status document");
    assert_eq!(
        doc["config_errors"],
        json!([{
            "kind": "invalid",
            "field": "vram_total_gb",
            "reason": "must be a finite number greater than 0"
        }]),
        "the wire shape is kind+field+reason, like catalog_errors"
    );
    api.record_config_errors(Vec::new());
    let doc = status_document(&api).await.expect("status document");
    assert_eq!(
        doc["config_errors"],
        json!([]),
        "a later landing write that validates clean replaces the verdict"
    );
}

/// End-to-end tripwire: the page-visible field through the real
/// `/har/config/default` path (`set_default_model` → gated blocking write-back →
/// recorded verdict → status document). The named test is the pin the `ApiState`
/// field's doc cites: a future boot-file mutation that forgets
/// `record_config_errors`, or a render path that regresses to `[]`, fails here.
/// `persisted` stays `true` — the bytes DID land; the field reports that the file
/// on disk will refuse the next boot, which is a different fact.
#[tokio::test]
async fn a_write_back_that_lands_a_boot_invalid_file_names_it_in_config_errors() {
    let dir = crate::testutil::temp_dir("api-config-e2e");
    let path = dir.join("simpleharn.json");
    let mut config = AppConfig::default();
    let mut root = serde_json::to_value(&config).expect("seed serializes");
    root["load_timeout_s"] = json!(0);
    std::fs::write(&path, root.to_string()).expect("seed the mid-run hand-edit");
    config.source_path = Some(path.clone());

    let mut adapters: BTreeMap<String, Arc<dyn crate::engine::EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(crate::engine::mock::MockAdapter) as Arc<dyn crate::engine::EngineAdapter>,
    );
    let scheduler = crate::scheduler::Scheduler::start(
        config.clone(),
        crate::testutil::temp_dir("api-config-e2e-boot"),
        adapters,
        Arc::new(crate::process::ProcessRuntime::new()) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(crate::budget::LedgerOnlyWitness),
    );
    let api = ApiState::new(
        config,
        scheduler,
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    );

    let doc = status_document(&api).await.expect("status document");
    assert_eq!(
        doc["config_errors"],
        json!([]),
        "nothing is recorded before any harness write-back"
    );
    let (effective, persisted) = api
        .set_default_model(CategoryId::Work, Some("pinned.gguf"))
        .await;
    assert_eq!(effective.as_deref(), Some("pinned.gguf"));
    assert!(persisted, "the patched file landed and read back");

    let doc = status_document(&api).await.expect("status document");
    assert!(
        doc["config_errors"]
            .as_array()
            .expect("config_errors stays an array")
            .iter()
            .any(|entry| entry["field"] == json!("load_timeout_s")),
        "the field the next boot would refuse is named now: {doc}"
    );
    assert_eq!(
        doc["categories"]
            .as_array()
            .expect("categories array")
            .iter()
            .find(|c| c["id"] == json!("WORK"))
            .and_then(|c| c["default_model"].as_str()),
        Some("pinned.gguf"),
        "the write-back itself is unchanged: only the verdict was added"
    );
}

/// End-to-end: the test arm lands a rename whose read-back refuses the
/// tree, so the live boot file holds bytes nobody validated. `/har/status`
/// `config_errors` must then describe THOSE bytes — the same authority a
/// successful write-back's verdict has — because the previous (empty) verdict
/// would claim the next boot reads a file the harness itself just replaced.
/// Falsifier: the pre-fix `if persisted { record }` shape answers
/// `persisted: false` here while `config_errors` stays `[]` — this leg goes red
/// and only this leg; every other pin stays green on the stale-verdict shape.
#[tokio::test]
async fn a_landed_but_unverified_write_restates_the_recorded_config_verdict() {
    let dir = crate::testutil::temp_dir("api-config-unverified-e2e");
    let path = dir.join("simpleharn.json");
    let config = AppConfig {
        source_path: Some(path.clone()),
        ..AppConfig::default()
    };
    let mut adapters: BTreeMap<String, Arc<dyn crate::engine::EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(crate::engine::mock::MockAdapter) as Arc<dyn crate::engine::EngineAdapter>,
    );
    let scheduler = crate::scheduler::Scheduler::start(
        config.clone(),
        crate::testutil::temp_dir("api-config-unverified-boot"),
        adapters,
        Arc::new(crate::process::ProcessRuntime::new()) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(crate::budget::LedgerOnlyWitness),
    );
    let api = ApiState::new(
        config,
        scheduler,
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    );
    let doc = status_document(&api).await.expect("status document");
    assert_eq!(
        doc["config_errors"],
        json!([]),
        "nothing is recorded before any harness write-back"
    );

    arm_config_read_back_fault(&path);
    let (effective, persisted) = api
        .set_default_model(CategoryId::Work, Some("phantom.gguf"))
        .await;
    release_config_read_back_fault(&path);

    assert_eq!(
        effective.as_deref(),
        Some("phantom.gguf"),
        "the in-memory override still takes the edit"
    );
    assert!(
        !persisted,
        "an unverifiable write may only ever be answered `persisted: false`"
    );
    let doc = status_document(&api).await.expect("status document");
    let errors = doc["config_errors"]
        .as_array()
        .expect("config_errors stays an array");
    assert!(
        errors.iter().any(|entry| {
            entry["field"] == json!(crate::config::CONFIG_FILE)
                && entry["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("tampered"))
        }),
        "the field must describe the bytes the harness just landed (file root + \
             the unknown key they carry), not keep the previous clean verdict: {doc}"
    );
}
