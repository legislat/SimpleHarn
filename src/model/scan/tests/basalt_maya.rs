use super::*;

/// The disk shape a valid basalt row validates against (the same shape
/// `basalt::validate_row` walks in its own module): an absolute `.basalt`
/// pack with bytes, an interpreter, an engine binary; the cwd is `dir`
/// itself. Returns the pack path.
fn basalt_pack(dir: &Path) -> std::path::PathBuf {
    let weights = dir.join("m.basalt");
    std::fs::write(&weights, b"basalt-pack-bytes").expect("write pack");
    std::fs::write(dir.join("python.exe"), b"p").expect("write interpreter");
    std::fs::write(dir.join("basalt-engine.exe"), b"e").expect("write engine");
    weights
}

/// One basalt row document in the valid five-knob shape, every path under
/// `dir`. An override `Some(value)` replaces (or adds) the line; `None`
/// drops it — the shape `validate_row` answers with the missing-key door.
fn basalt_yaml(dir: &Path, weights: &Path, overrides: &[(&str, Option<&str>)]) -> String {
    let mut lines: Vec<(&str, String)> = vec![
        ("engine", "basalt".to_string()),
        ("model", weights.to_string_lossy().into_owned()),
        ("serve_model", "basalt-served".to_string()),
        ("vram_gb", "42".to_string()),
        ("cost", "fixed".to_string()),
        (
            "basalt_python",
            dir.join("python.exe").to_string_lossy().into_owned(),
        ),
        ("basalt_cwd", dir.to_string_lossy().into_owned()),
        (
            "basalt_exe",
            dir.join("basalt-engine.exe").to_string_lossy().into_owned(),
        ),
    ];
    for (key, value) in overrides {
        if let Some(line) = lines.iter_mut().find(|(k, _)| *k == *key) {
            if let Some(value) = value {
                line.1 = value.to_string();
            } else {
                line.0 = "";
            }
        } else if let Some(value) = value {
            lines.push((key, value.to_string()));
        }
    }
    lines
        .iter()
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| format!("{k}: {v}\n"))
        .collect()
}

/// A basalt row that lacks a key the launch requires is refused at rescan
/// — before any load could evict a resident for the doomed spawn — and the
/// report names the field. An absent key and a present-but-blank one are
/// the same door (the adapter's `validate_row` is the single source of the
/// required set, so the scan names whatever that door names). Falsifier:
/// no basalt branch in the scan publishes the row and the report is empty.
#[test]
fn a_basalt_row_missing_a_required_key_is_refused_naming_the_key() {
    let dir = crate::testutil::temp_dir("scan-basalt-missing-key");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let weights = basalt_pack(&dir);
    for (key, value) in [
        ("model", None),
        ("serve_model", None),
        ("basalt_python", None),
        ("basalt_cwd", None),
        ("basalt_exe", None),
        ("vram_gb", None),
        // The blank leg: a present value that carries nothing is as
        // unservable as the absent one.
        ("serve_model", Some("")),
    ] {
        std::fs::write(
            dir.join("row.yaml"),
            basalt_yaml(&dir, &weights, &[(key, value)]),
        )
        .expect("write row");
        let catalog = scan_dir(&dir);
        assert!(
            catalog.models.is_empty(),
            "`{key}` missing/blank must refuse the row: {:?}",
            catalog.errors
        );
        let error = catalog
            .errors
            .iter()
            .find(|e| e.file.contains("row.yaml"))
            .unwrap_or_else(|| {
                panic!(
                    "`{key}` missing/blank must be reported: {:?}",
                    catalog.errors
                )
            });
        assert_eq!(error.kind, ScanKind::Other, "{error:?}");
        assert!(
            error.reason.contains(key),
            "must name `{key}`: {}",
            error.reason
        );
    }
}

/// A greedy basalt row
/// is the catalog citizen the operator's truth needs - Basalt offloads to
/// RAM, so the row may carry no declared `vram_gb` at all (booking takes
/// the room at admit, the launch floor gates). The row must publish at
/// scan, not vanish behind a catalog error (a failure seen live).
/// The `cost: fixed` refusal without `vram_gb` stays pinned by the
/// key-missing loop above. Falsifier: an adapter guard refusing Greedy
/// drops the row again.
#[test]
fn a_greedy_basalt_row_publishes_without_a_declared_vram_gb() {
    let dir = crate::testutil::temp_dir("scan-basalt-greedy");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let weights = basalt_pack(&dir);
    std::fs::write(
        dir.join("row.yaml"),
        basalt_yaml(
            &dir,
            &weights,
            &[("cost", Some("greedy")), ("vram_gb", None)],
        ),
    )
    .expect("write row");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert_eq!(catalog.models[0].name, "row");
    assert_eq!(catalog.models[0].cost, crate::model::CostClass::Greedy);
}

/// The `.basalt` pack is the weights file the launch spawns against: a
/// missing one is the same `ModelPathMissing` door as llama's `model:`
/// (same class — refuse before admission can evict a victim for
/// the doomed spawn), not a generic typed reject. Falsifier: mapping every
/// `BasaltRowError::Bad` to `Other` reports the right file under the wrong
/// kind.
#[test]
fn a_basalt_row_naming_a_missing_pack_is_model_path_missing() {
    let dir = crate::testutil::temp_dir("scan-basalt-missing-pack");
    std::fs::create_dir_all(&dir).expect("temp dir");
    basalt_pack(&dir);
    std::fs::write(
        dir.join("row.yaml"),
        basalt_yaml(&dir, &dir.join("ghost.basalt"), &[]),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a pack that is not on disk must not publish: {:?}",
        catalog.errors
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing pack travels the llama `model:` door");
    assert!(error.file.contains("row.yaml"), "{error:?}");
    assert!(error.reason.contains("ghost.basalt"), "{}", error.reason);
}

/// A server-owned flag smuggled through `basalt_engine_args` meets the
/// policy surface (`PolicyRefused`), naming the flag and its owner, exactly
/// like the `extra_args`/`ninfer_serve_args` family. The row's
/// `extra_words` contract in `from_flat` is the first door; the scan's
/// direct `validate_row` leg keeps the mapping consistent.
#[test]
fn a_basalt_row_with_a_server_owned_engine_arg_is_policy_refused() {
    let dir = crate::testutil::temp_dir("scan-basalt-hostile-arg");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let weights = basalt_pack(&dir);
    std::fs::write(
        dir.join("row.yaml"),
        basalt_yaml(
            &dir,
            &weights,
            &[("basalt_engine_args", Some("--host=0.0.0.0"))],
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a harness-owned flag must refuse the row: {:?}",
        catalog.errors
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::PolicyRefused)
        .expect("the hostile word is the policy surface, never a typed reject");
    assert!(error.file.contains("row.yaml"), "{error:?}");
    assert!(
        error.reason.contains("`--host`") && error.reason.contains("harness-owned"),
        "the refusal names the flag and its owner: {}",
        error.reason
    );
}

/// The happy path pinned whole: a valid basalt row publishes with its five
/// knobs live on their own row — zero reports, neither `UnknownKey` (the
/// knobs are consumed) nor `InertKey` (the basalt row reads them) — and the
/// `.basalt` pack's bytes never book into `gguf_size_gb` (the booking
/// source is the declared `vram_gb` with `cost: fixed`). Falsifier: without
/// the scan-branch and consumed-key work the knobs warn and the pack size
/// books.
#[test]
fn a_valid_basalt_row_publishes_with_its_five_knobs_live() {
    let dir = crate::testutil::temp_dir("scan-basalt-valid");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let weights = basalt_pack(&dir);
    let yaml = basalt_yaml(
        &dir,
        &weights,
        &[
            ("basalt_parallel", Some("2")),
            ("basalt_engine_args", Some("--temp=0.7")),
        ],
    );
    std::fs::write(dir.join("row.yaml"), yaml).expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog.errors.is_empty(),
        "a knob on its own row stays silent: {:?}",
        catalog.errors
    );
    assert_eq!(catalog.models[0].engine, "basalt");
    assert!(
        catalog.models[0].gguf_size_gb.is_none(),
        "a .basalt pack size never feeds the GGUF estimate"
    );
}

/// The disk shape a valid maya row validates against (the same shape
/// `maya::validate_row` walks in its own module): an interpreter, an engine
/// binary, a tokenizer dir carrying `vocab.json`, and a minimal config JSON
/// shaped like the operator's installs. Returns the config path.
fn maya_install(dir: &Path) -> std::path::PathBuf {
    let tokenizer = dir.join("tokenizer");
    std::fs::create_dir_all(&tokenizer).expect("write tokenizer dir");
    std::fs::write(tokenizer.join("vocab.json"), b"{}").expect("write vocab");
    std::fs::write(dir.join("python.exe"), b"p").expect("write interpreter");
    std::fs::write(dir.join("maya.exe"), b"e").expect("write engine");
    let config = dir.join("maya.json");
    std::fs::write(
        &config,
        serde_json::json!({
            "exe": dir.join("maya.exe"),
            "args": ["--glm-pack", "pack", "--max-context", "131072"],
            "cwd": dir,
            "tokenizer": tokenizer,
            "model_name": "maya-served",
        })
        .to_string(),
    )
    .expect("write config");
    config
}

/// One maya row document in the valid shape, every path under `dir`. An
/// override `Some(value)` replaces (or adds) the line; `None` drops it —
/// the shape `validate_row` answers with the missing-key door.
fn maya_yaml(dir: &Path, config: &Path, overrides: &[(&str, Option<&str>)]) -> String {
    let mut lines: Vec<(&str, String)> = vec![
        ("engine", "maya".to_string()),
        ("serve_model", "maya-served".to_string()),
        ("cost", "greedy".to_string()),
        ("launch_only_with_at_least_vram_free", "20".to_string()),
        ("load_timeout", "900".to_string()),
        (
            "maya_python",
            dir.join("python.exe").to_string_lossy().into_owned(),
        ),
        ("maya_cwd", dir.to_string_lossy().into_owned()),
        ("maya_config", config.to_string_lossy().into_owned()),
    ];
    for (key, value) in overrides {
        if let Some(line) = lines.iter_mut().find(|(k, _)| *k == *key) {
            if let Some(value) = value {
                line.1 = value.to_string();
            } else {
                line.0 = "";
            }
        } else if let Some(value) = value {
            lines.push((key, value.to_string()));
        }
    }
    lines
        .iter()
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| format!("{k}: {v}\n"))
        .collect()
}

/// The happy path pinned whole (same posture as basalt's): a valid maya
/// row publishes with its three knobs live — zero reports, neither
/// `UnknownKey` (the knobs are consumed) nor `InertKey` (the maya row
/// reads them) — and the greedy booking posture (no declared `vram_gb`,
/// the launch floor is the gate) lands intact. Falsifier: dropping the
/// knob lines from the consumed const warns the live row on its own keys.
#[test]
fn a_valid_maya_row_publishes_with_its_three_knobs_live() {
    let dir = crate::testutil::temp_dir("scan-maya-valid");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    std::fs::write(dir.join("row.yaml"), maya_yaml(&dir, &config, &[])).expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog.errors.is_empty(),
        "a knob on its own row stays silent: {:?}",
        catalog.errors
    );
    let entry = &catalog.models[0];
    assert_eq!(entry.engine, "maya");
    assert_eq!(entry.serve_model.as_deref(), Some("maya-served"));
    assert_eq!(entry.cost, crate::model::CostClass::Greedy);
    assert_eq!(entry.load_timeout_s, Some(900));
    assert_eq!(entry.vram_gate.floor(), Some(20.0));
}

/// The typed `maya_vision` overlay over real temp
/// assets publishes through the scan side of the single door with zero
/// reports: the key is consumed, and every overlay path exists, so the
/// row earns neither `UnknownKey` nor a typed reject. Falsifier: dropping
/// the overlay from the consumed const warns the live row on its own key;
/// removing one asset then makes the same row a `ModelPathMissing` naming
/// `maya_vision.model`, so a door that never checks the paths fails here.
#[test]
fn a_maya_row_with_a_vision_overlay_publishes_with_no_reports() {
    let dir = crate::testutil::temp_dir("scan-maya-vision");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    for name in ["vision.exe", "mmproj.gguf", "vision.gguf"] {
        std::fs::write(dir.join(name), b"v").expect("overlay asset");
    }
    let line = format!(
        "exe={} mmproj={} model={}",
        dir.join("vision.exe").to_string_lossy(),
        dir.join("mmproj.gguf").to_string_lossy(),
        dir.join("vision.gguf").to_string_lossy(),
    );
    std::fs::write(
        dir.join("row.yaml"),
        maya_yaml(&dir, &config, &[("maya_vision", Some(&line))]),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog.errors.is_empty(),
        "a live overlay on its own row earns no report: {:?}",
        catalog.errors
    );
    std::fs::remove_file(dir.join("vision.gguf")).expect("remove overlay asset");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a missing overlay asset must not publish: {:?}",
        catalog.models
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing overlay asset travels the model-path door");
    assert!(
        error.reason.contains("maya_vision.model"),
        "the reason names the overlay member: {}",
        error.reason
    );
}

/// A row that spells `maya_vision` while the engine-side config still
/// carries its own `vision` object is refused at rescan — before any load
/// could evict a resident for the conflicted spawn — and the sentence
/// names `maya_vision`. Falsifier: a silent precedence publishes the row
/// and the staged bytes answer to a different spelling than the row.
#[test]
fn a_maya_row_with_overlay_and_config_vision_is_refused_naming_the_overlay() {
    let dir = crate::testutil::temp_dir("scan-maya-vision-conflict");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    for name in ["vision.exe", "mmproj.gguf", "vision.gguf"] {
        std::fs::write(dir.join(name), b"v").expect("overlay asset");
    }
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
    value["vision"] = serde_json::json!({
        "exe": dir.join("vision.exe"),
        "mmproj": dir.join("mmproj.gguf"),
        "model": dir.join("vision.gguf"),
        "gpu": true,
    });
    std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
    let line = format!(
        "exe={} mmproj={} model={}",
        dir.join("absent-overlay.exe").to_string_lossy(),
        dir.join("mmproj.gguf").to_string_lossy(),
        dir.join("vision.gguf").to_string_lossy(),
    );
    std::fs::write(
        dir.join("row.yaml"),
        maya_yaml(&dir, &config, &[("maya_vision", Some(&line))]),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "the conflicted row must not publish: {:?}",
        catalog.models
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.file == "row.yaml")
        .expect("the conflict is reported on the row");
    assert_eq!(error.kind, ScanKind::Other, "{error:?}");
    assert!(
        error.reason.contains("maya_vision") && error.reason.contains("conflicts"),
        "the refusal names the overlay key and the conflict, before any path check: {}",
        error.reason
    );
}

/// The scan captures the config's `--max-context` figure once and
/// carries it on the entry under the scan-derived key; the `/har/models`
/// display home answers it end-to-end; a rescan re-derives the same figure;
/// the carry follows the config's bytes (the `=` spelling, and no figure
/// when the config spells none); and a hand-written line of the carry name
/// never outranks the config — it is replaced or removed as the dead key
/// the scan names. The carry itself is not a yaml key, so the valid row
/// still publishes with zero reports. Falsifier: dropping the capture
/// leaves the display null; reading the carry from yaml instead of the
/// derived figure fails the hand-written override legs.
#[test]
fn a_scanned_maya_row_carries_the_config_max_context_and_rescans_with_it() {
    let dir = crate::testutil::temp_dir("scan-maya-context");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    std::fs::write(dir.join("row.yaml"), maya_yaml(&dir, &config, &[])).expect("write");
    fn carry(catalog: &crate::model::scan::Catalog) -> Option<&str> {
        catalog.models[0]
            .raw
            .get(crate::engine::maya::MAYA_MAX_CONTEXT_KEY)
            .map(String::as_str)
    }
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog.errors.is_empty(),
        "the carry earns no report of its own: {:?}",
        catalog.errors
    );
    assert_eq!(carry(&catalog), Some("131072"));
    assert_eq!(
        crate::engine::effective_context(&catalog.models[0]).as_deref(),
        Some("131072"),
        "the display home answers the carried figure"
    );
    let again = scan_dir(&dir);
    assert_eq!(
        carry(&again),
        Some("131072"),
        "a rescan re-derives the same figure: {:?}",
        again.errors
    );
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
    value["args"] = serde_json::json!(["--max-context=262144"]);
    std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
    let moved = scan_dir(&dir);
    assert_eq!(
        crate::engine::effective_context(&moved.models[0]).as_deref(),
        Some("262144"),
        "the carried figure follows the staged bytes, not the last display"
    );
    value["args"] = serde_json::json!(["--glm-pack", "pack"]);
    std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
    let bare = scan_dir(&dir);
    assert_eq!(crate::engine::effective_context(&bare.models[0]), None);
    // A hand-written carry line never outranks the config: with no config
    // figure it is removed, not displayed — and named as the unknown key
    // it is.
    std::fs::write(
        dir.join("row.yaml"),
        maya_yaml(&dir, &config, &[("maya_max_context", Some("999"))]),
    )
    .expect("write");
    let hand = scan_dir(&dir);
    assert_eq!(
        carry(&hand),
        None,
        "the absent config figure removes the hand-written carry"
    );
    assert!(
        hand.errors
            .iter()
            .any(|e| e.kind == ScanKind::UnknownKey && e.reason.contains("maya_max_context")),
        "a hand-written carry line is the dead key the scan names: {:?}",
        hand.errors
    );
    // With a config figure, the same hand-written line is replaced by the
    // derived one: the page shows the config, never the hand-written value.
    value["args"] = serde_json::json!(["--max-context", "131072"]);
    std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
    let overridden = scan_dir(&dir);
    assert_eq!(carry(&overridden), Some("131072"));
}

/// A maya row that lacks a key the launch requires, carries one below
/// the Maya load floor, or claims a cost class the engine cannot book,
/// is refused at rescan — before any load could evict a resident for the
/// doomed spawn — and the report names the key (the adapter's
/// `validate_row` is the single source of the required set, so the scan
/// names whatever that door names). Falsifier: no maya branch in the
/// scan publishes the row and the report is empty.
#[test]
fn a_maya_row_missing_or_below_a_required_key_is_refused_naming_the_key() {
    let dir = crate::testutil::temp_dir("scan-maya-missing-key");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    for (key, value) in [
        ("maya_config", None),
        ("maya_python", None),
        ("load_timeout", None),
        // A present value below the floor is the same door with its
        // number: the global 120 s default would kill a healthy load.
        ("load_timeout", Some("120")),
        // The engine reports no computed requirement: `exact` books a
        // figure that does not exist (same posture as basalt).
        ("cost", Some("exact")),
    ] {
        std::fs::write(
            dir.join("row.yaml"),
            maya_yaml(&dir, &config, &[(key, value)]),
        )
        .expect("write row");
        let catalog = scan_dir(&dir);
        assert!(
            catalog.models.is_empty(),
            "`{key}` missing/below-floor must refuse the row: {:?}",
            catalog.errors
        );
        let error = catalog
            .errors
            .iter()
            .find(|e| e.file.contains("row.yaml"))
            .unwrap_or_else(|| panic!("`{key}` must be reported: {:?}", catalog.errors));
        assert_eq!(error.kind, ScanKind::Other, "{error:?}");
        assert!(
            error.reason.contains(key),
            "must name `{key}`: {}",
            error.reason
        );
    }
}

/// A `maya_config` the disk lacks is the `ModelPathMissing` door (the
/// same class as llama's `model:` and basalt's pack — refuse before
/// admission can evict a victim for the doomed spawn), not a generic
/// typed reject, and the reason names both the key and the path.
/// Falsifier: mapping `MayaRowError::MissingPath` to `Other` reports the
/// right file under the wrong kind.
#[test]
fn a_maya_row_naming_a_missing_config_is_model_path_missing() {
    let dir = crate::testutil::temp_dir("scan-maya-missing-config");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    // Maya's door judges this host's absolute shape, so the absent config
    // must be spelled in the host grammar to reach the existence check.
    #[cfg(windows)]
    let absent = "Z:/absent-maya.json";
    #[cfg(not(windows))]
    let absent = "/nope/absent-maya.json";
    std::fs::write(
        dir.join("row.yaml"),
        maya_yaml(&dir, &config, &[("maya_config", Some(absent))]),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a config that is not on disk must not publish: {:?}",
        catalog.errors
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing config travels the model-path door");
    assert!(error.file.contains("row.yaml"), "{error:?}");
    assert!(
        error.reason.contains("maya_config") && error.reason.contains("absent-maya.json"),
        "the reason names the key and the absent path: {}",
        error.reason
    );
}

/// The served id is verified, not trusted: a config whose `model_name`
/// is not the row's `serve_model` would pass readiness against a
/// different model, so the scan refuses the row and the sentence names
/// both keys. Falsifier: dropping the `model_name` check in
/// `validate_row` publishes the mismatched row.
#[test]
fn a_maya_row_whose_config_model_name_mismatches_is_refused_naming_the_keys() {
    let dir = crate::testutil::temp_dir("scan-maya-name-mismatch");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = maya_install(&dir);
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
    value["model_name"] = serde_json::json!("ghost-model");
    std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
    std::fs::write(dir.join("row.yaml"), maya_yaml(&dir, &config, &[])).expect("write row");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a mismatched served id must not publish: {:?}",
        catalog.errors
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.file.contains("row.yaml"))
        .expect("the mismatch is reported");
    assert_eq!(error.kind, ScanKind::Other, "{error:?}");
    assert!(
        error.reason.contains("serve_model")
            && error.reason.contains("model_name")
            && error.reason.contains("ghost-model"),
        "the sentence names both keys and the foreign id: {}",
        error.reason
    );
}
