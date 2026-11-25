use super::*;

/// A row that still declares `strata_config` is the SUPERSEDED mode:
/// refused outright with the migration
/// sentence, whatever the profile file itself holds — unless the row also
/// carries `strata_exe`, which means the import already happened and the
/// accurate sentence is the mutual-exclusion one.
#[test]
fn a_strata_config_row_is_refused_naming_the_import_command() {
    let dir = crate::testutil::temp_dir("scan-strata-superseded");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("broken.json"), "{ not json").expect("write profile");
    for (file, target) in [
        ("nofile.yaml", "Z:/nope/missing.json"),
        (
            "manual.yaml",
            &dir.join("broken.json").display().to_string(),
        ),
    ] {
        std::fs::write(
            dir.join(file),
            format!("engine: strata\nstrata_config: {target}\nvram_gb: 8\n"),
        )
        .expect("write");
    }
    std::fs::write(
        dir.join("both.yaml"),
        format!(
            "engine: strata\nstrata_exe: C:\\Strata\\engine\\strata.exe\n\
                 strata_config: {}\nvram_gb: 8\n",
            dir.join("broken.json").display()
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "no row may publish");
    for file in ["nofile.yaml", "manual.yaml"] {
        assert!(
            catalog
                .errors
                .iter()
                .any(|e| e.kind == ScanKind::PolicyRefused
                    && e.file.contains(file)
                    && e.reason.contains("superseded")
                    && e.reason.contains("simpleharn import strata")),
            "{file} must be refused with the migration sentence: {:?}",
            catalog.errors
        );
    }
    // A row that already carries the imported surface only
    // needs its `strata_config` line deleted, so the scan names the
    // mutual-exclusion door, not a re-import the row does not need.
    assert!(
        catalog
            .errors
            .iter()
            .any(|e| e.kind == ScanKind::PolicyRefused
                && e.file.contains("both.yaml")
                && e.reason.contains("mutually exclusive")
                && !e.reason.contains("simpleharn import strata")),
        "both-sources row must meet the mutual-exclusion sentence: {:?}",
        catalog.errors
    );
}

/// Template mode moves the path walk onto the RENDERA: a row whose engine
/// args name a path the disk lacks drops the entry at rescan, naming the
/// key and the path exactly as the old file walk did.
#[test]
fn a_template_row_naming_a_missing_path_drops_the_entry_naming_key_and_path() {
    let dir = crate::testutil::temp_dir("scan-strata-missing-path");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("strata.exe"), b"x").expect("write exe");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("write tokenizer");
    // The walk only judges this host's absolute shapes, so the doomed
    // path must be spelled in the host grammar to reach the stat.
    #[cfg(windows)]
    let doomed = r"Z:\nope\doomed.gguf";
    #[cfg(not(windows))]
    let doomed = "/nope/scan-doomed.gguf";
    let yaml = format!(
        "engine: strata\nvram_gb: 8\nserve_model: m\nstrata_exe: {}\n\
             strata_tokenizer: {}\nstrata_cwd: {}\n\
             strata_args: --native, {doomed}\n",
        dir.join("strata.exe").display(),
        dir.join("tokenizer.bin").display(),
        dir.display(),
    );
    std::fs::write(dir.join("ref.yaml"), yaml).expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty());
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("a referenced path that is gone must drop the entry");
    assert!(
        error
            .reason
            .starts_with("strata profile references missing path: ")
            && error.reason.contains("doomed.gguf"),
        "{}",
        error.reason
    );
    // The array element reports under its owning key so the operator can
    // find the line in the row.
    assert!(error.reason.contains("args"), "{}", error.reason);
}

/// The `--control-vector-scaled <path>:1.0` shape: the scale suffix is
/// stripped before the stat, so a real file named `x.gguf` referenced as
/// `x.gguf:1.0` publishes instead of being falsely dropped.
#[test]
fn a_template_row_control_vector_scale_suffix_is_stripped_before_the_stat() {
    let dir = crate::testutil::temp_dir("scan-strata-scaled");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("strata.exe"), b"x").expect("write exe");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("write tokenizer");
    std::fs::write(dir.join("vec.gguf"), b"v").expect("write vector");
    let yaml = format!(
        "engine: strata\nvram_gb: 8\nserve_model: m\nstrata_exe: {}\n\
             strata_tokenizer: {}\nstrata_cwd: {}\n\
             strata_args: --control-vector-scaled, {}:1.0\n",
        dir.join("strata.exe").display(),
        dir.join("tokenizer.bin").display(),
        dir.display(),
        dir.join("vec.gguf").display(),
    );
    std::fs::write(dir.join("scaled.yaml"), yaml).expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
}

/// A shape a knob cannot carry (a relative `strata_exe`) is reported as a
/// typed reject naming the key — and the row does not publish.
#[test]
fn a_template_row_shape_refusal_reports_naming_the_yaml_key() {
    let dir = crate::testutil::temp_dir("scan-strata-shape");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("write tokenizer");
    let yaml = format!(
        "engine: strata\nvram_gb: 8\nserve_model: m\nstrata_exe: relative\\strata.exe\n\
             strata_tokenizer: {}\n",
        dir.join("tokenizer.bin").display(),
    );
    std::fs::write(dir.join("shape.yaml"), yaml).expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty());
    let error = catalog
        .errors
        .iter()
        .find(|e| e.file.contains("shape.yaml"))
        .expect("the row must be reported");
    assert!(
        error.reason.contains("strata_exe") && error.reason.contains("absolute path"),
        "{}",
        error.reason
    );
}

#[test]
fn a_strata_entry_without_a_profile_source_defers_to_the_adapter() {
    // The preflight pin (`required field` refuses before the
    // victim dies) needs this entry published, so an absent source key is
    // the adapter's refusal to name (row 19: `strata_exe`), not the scan's.
    let dir = crate::testutil::temp_dir("scan-strata-nokey");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("nocfg.yaml"), "engine: strata\nvram_gb: 8\n").expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
}

/// A complete xolotl row: required keys present,
/// the timeout at its floor, and a relative `-m` that resolves against
/// `xolotl_cwd` — it scans, publishes, and earns no report.
#[test]
fn an_xolotl_row_scans_and_publishes() {
    let dir = crate::testutil::temp_dir("scan-xolotl-clean");
    std::fs::write(dir.join("python.exe"), b"p").expect("python fixture");
    std::fs::write(dir.join("serve_ds41.py"), b"").expect("launcher fixture");
    std::fs::write(dir.join("ds41_generate.exe"), b"e").expect("engine fixture");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("tokenizer fixture");
    std::fs::create_dir_all(dir.join("model")).expect("model dir");
    std::fs::write(dir.join("model").join("V41.gguf"), b"g").expect("weights fixture");
    std::fs::write(
        dir.join("ds41.yaml"),
        format!(
            "engine: xolotl\n\
                 cost: greedy\n\
                 launch_only_with_at_least_vram_free: whole\n\
                 load_timeout: 600\n\
                 serve_model: ds41-flash\n\
                 xolotl_python: {}\n\
                 xolotl_launcher: {}\n\
                 xolotl_cwd: {}\n\
                 xolotl_exe: {}\n\
                 xolotl_tokenizer: {}\n\
                 xolotl_args: --backend cuda, --ctx 262144, --kv-q8, -m model/V41.gguf\n",
            dir.join("python.exe").display(),
            dir.join("serve_ds41.py").display(),
            dir.display(),
            dir.join("ds41_generate.exe").display(),
            dir.join("tokenizer.bin").display(),
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    assert_eq!(catalog.models[0].cost, crate::model::CostClass::Greedy);
}

/// The whole-posture keys and the row's shape: an unknown `xolotl_*` key
/// warns and the row still publishes (a dead key is not a refusal); a
/// foreign engine-scoped key (`context`) earns its inert-key hint naming
/// its owner.
#[test]
fn unknown_xolotl_keys_warn_and_foreign_knobs_hint_their_owner() {
    let dir = crate::testutil::temp_dir("scan-xolotl-unknown");
    std::fs::write(dir.join("python.exe"), b"p").expect("python fixture");
    std::fs::write(dir.join("serve_ds41.py"), b"").expect("launcher fixture");
    std::fs::write(dir.join("ds41_generate.exe"), b"e").expect("engine fixture");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("tokenizer fixture");
    std::fs::write(
        dir.join("ds41.yaml"),
        format!(
            "engine: xolotl\nload_timeout: 600\nserve_model: ds41-flash\n\
                 xolotl_python: {}\nxolotl_launcher: {}\nxolotl_cwd: {}\n\
                 xolotl_exe: {}\nxolotl_tokenizer: {}\n\
                 xolotl_mode: turbo\ncontext: 4096\n",
            dir.join("python.exe").display(),
            dir.join("serve_ds41.py").display(),
            dir.display(),
            dir.join("ds41_generate.exe").display(),
            dir.join("tokenizer.bin").display(),
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog
            .errors
            .iter()
            .any(|e| { e.kind == ScanKind::UnknownKey && e.reason.contains("xolotl_mode") }),
        "an unknown xolotl_* key must warn: {:?}",
        catalog.errors
    );
    assert!(
        catalog.errors.iter().any(|e| {
            e.kind == ScanKind::InertKey
                && e.reason.contains("`context`")
                && e.reason.contains("llama")
        }),
        "the llama-owned key must hint its owner on a xolotl row: {:?}",
        catalog.errors
    );
}

/// A relative `-m` that does not resolve against `xolotl_cwd` is the
/// `ModelPathMissing` door (refuse before admission can evict a resident
/// for the doomed spawn), and the reason names the flag and the resolved
/// path. Falsifier: mapping `MissingPath` to `Other` reports the right
/// file under the wrong kind.
#[test]
fn a_xolotl_relative_model_that_misses_is_model_path_missing() {
    let dir = crate::testutil::temp_dir("scan-xolotl-missing-model");
    std::fs::write(dir.join("python.exe"), b"p").expect("python fixture");
    std::fs::write(dir.join("serve_ds41.py"), b"").expect("launcher fixture");
    std::fs::write(dir.join("ds41_generate.exe"), b"e").expect("engine fixture");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("tokenizer fixture");
    std::fs::write(
        dir.join("ds41.yaml"),
        format!(
            "engine: xolotl\nload_timeout: 600\nserve_model: ds41-flash\n\
                 xolotl_python: {}\nxolotl_launcher: {}\nxolotl_cwd: {}\n\
                 xolotl_exe: {}\nxolotl_tokenizer: {}\n\
                 xolotl_args: --backend cuda, -m model/gone.gguf\n",
            dir.join("python.exe").display(),
            dir.join("serve_ds41.py").display(),
            dir.display(),
            dir.join("ds41_generate.exe").display(),
            dir.join("tokenizer.bin").display(),
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "a doomed spawn must not publish");
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing weights travel the model-path door");
    assert!(
        error.reason.contains("-m") && error.reason.contains("gone.gguf"),
        "the refusal names the flag and the path: {}",
        error.reason
    );
}

/// Ruling 6 at the scan door: a DSpark sidecar the row names and the
/// disk lacks refuses at rescan by path, not at load after eviction.
#[test]
fn a_xolotl_row_naming_a_missing_dspark_is_model_path_missing() {
    let dir = crate::testutil::temp_dir("scan-xolotl-dspark");
    std::fs::write(dir.join("python.exe"), b"p").expect("python fixture");
    std::fs::write(dir.join("serve_ds41.py"), b"").expect("launcher fixture");
    std::fs::write(dir.join("ds41_generate.exe"), b"e").expect("engine fixture");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("tokenizer fixture");
    std::fs::write(
        dir.join("ds41.yaml"),
        format!(
            "engine: xolotl\nload_timeout: 600\nserve_model: ds41-flash\n\
                 xolotl_python: {}\nxolotl_launcher: {}\nxolotl_cwd: {}\n\
                 xolotl_exe: {}\nxolotl_tokenizer: {}\n\
                 xolotl_env: STRATA_SERVE_DSPARK={}\n",
            dir.join("python.exe").display(),
            dir.join("serve_ds41.py").display(),
            dir.display(),
            dir.join("ds41_generate.exe").display(),
            dir.join("tokenizer.bin").display(),
            dir.join("gone.Q2_K.gguf").display(),
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "a doomed spawn must not publish");
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the named sidecar travels the model-path door");
    assert!(
        error.reason.contains("STRATA_SERVE_DSPARK") && error.reason.contains("gone.Q2_K.gguf"),
        "the refusal names the variable and the path: {}",
        error.reason
    );
}
