use super::*;

#[test]
fn a_one_edit_typo_gets_a_unique_suggestion() {
    assert_eq!(did_you_mean("contex"), " (did you mean 'context'?)");
    // `por` is distance 2 from every consumed key (and `port` is policy-hinted,
    // not consumed), so no suggestion is owed — a guess would be false advice.
    assert_eq!(did_you_mean("por"), "");
    // A key one edit from a single consumed key still gets the advice.
    assert_eq!(did_you_mean("ngl_"), " (did you mean 'ngl'?)");
    assert!(!within_one_edit("context", "context"));
}

/// The `whole` keyword of the VRAM-gate key must ride the real scan with
/// zero reports (the key is consumed, so never `UnknownKey`), keep its
/// raw line verbatim for write-back, and land typed as the whole-machine
/// gate. The golden fixture stays numeric on
/// purpose: this pin owns the keyword shape.
/// Falsifier: a parse that rejects the keyword at scan time surfaces an
/// error or drops the entry, and this pin goes red.
#[test]
fn a_whole_machine_row_scans_and_keeps_its_line() {
    let dir = crate::testutil::temp_dir("scan-whole");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("wholerow.yaml"),
        "engine: mock\ncost: fixed\nvram_gb: 12\nlaunch_only_with_at_least_vram_free: whole\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog
            .errors
            .iter()
            .all(|e| e.kind != ScanKind::UnknownKey),
        "the keyword must not make its key unknown: {:?}",
        catalog.errors
    );
    let m = &catalog.models[0];
    assert_eq!(m.vram_gate, crate::model::VramGate::Whole);
    assert_eq!(
        m.raw
            .get("launch_only_with_at_least_vram_free")
            .map(String::as_str),
        Some("whole"),
        "write-back must see the operator's line unchanged"
    );
}

/// The golden fixture pins the const from the front: every consumed key appears
/// in a real file, and the whole file scans with ZERO unknown-key errors.
/// Every engine knob rides the `engine: mock` file deliberately: mock reads
/// none of them, so the fixture's complete set of reports is the
/// `InertKey` hints — one per foreign knob, naming its owning row — and the
/// const-coverage claim stays honest without pretending any knob is live on
/// mock. Harness-blind keys (`name`, `model`, the `measured_*`...) stay
/// silent even here.
#[test]
fn the_golden_yaml_covers_every_consumed_key_and_scans_unknown_key_free() {
    let dir = crate::testutil::temp_dir("scan-golden");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
            dir.join("golden.yaml"),
            "engine: mock\n\
             name: golden\n\
             model: mock://golden\n\
             category: WORK\n\
             is_embedding_model: false\n\
             vram_gb: 12\n\
             ram_gb: 9\n\
             cost: greedy\n\
             launch_only_with_at_least_vram_free: 6\n\
             load_timeout: 600\n\
             min_turn: 300\n\
             serve_model: golden-id\n\
             cpu_offload: false\n\
             jinja: true\n\
             extra_args: --warmup\n\
             mmproj: F:/m/mmproj.gguf\n\
             context: 4096\n\
             ngl: 99\n\
             batch: 512\n\
             ubatch: 256\n\
             flash_attn: 1\n\
             cache_type_k: q8_0\n\
             cache_type_v: q8_0\n\
             parallel: 2\n\
             fit: true\n\
             fit_target: 0.9\n\
             fit_ctx: 2048\n\
             llama_server_exe: D:/llama/llama-server.exe\n\
             measured_gb: 20.0\n\
             measured_gb_min: 10.0\n\
             measured_ram_gb: 14.0\n\
             measured_ram_gb_min: 10.0\n\
             measured_offload: false\n\
             measured_attribution: total_diff\n\
             measured_profile: b\n\
             strata_config: C:/Strata/profile.json\n\
             strata_python: C:/Strata/.venv/Scripts/python.exe\n\
             strata_cwd: C:/Strata\n\
             strata_exe: C:/Strata/engine/strata.exe\n\
             strata_args: --pack, D:/models/pack\n\
             strata_tokenizer: C:/Strata/tokenizer.bin\n\
             strata_lib_dirs: C:/Strata/lib\n\
             strata_env: STRATA_KVA=0\n\
             strata_gpu: 0\n\
             strata_parallel: 1\n\
             strata_vision: exe=C:/Strata/vision.exe mmproj=D:/m/mmproj.gguf model=D:/m/m.gguf\n\
             strata_sampling: temperature=1.0 top_p=0.95 top_k=20\n\
             strata_aliases: golden-a, golden-b\n\
             strata_reasoning_budget: 1024\n\
             strata_profile_extra: {\"answer_room\":true}\n\
             strata_uncensored: false\n\
             basalt_python: C:/Basalt/.venv/Scripts/python.exe\n\
             basalt_cwd: C:/Basalt\n\
             basalt_exe: C:/Basalt/bin/basalt.exe\n\
             basalt_parallel: 1\n\
             basalt_vision: dynamic\n\
             basalt_engine_args: --temp=0.7\n\
             basalt_sampling: {\"temperature\":0.7}\n\
              maya_python: D:/Maya/.venv/Scripts/python.exe\n\
              maya_cwd: D:/Maya\n\
              maya_config: D:/Maya/maya-maya-m.json\n\
              maya_vision: exe=C:/Maya/vision/mmproj-exe.exe mmproj=D:/m/mmproj.gguf model=D:/m/m.gguf gpu=true\n\
             xolotl_python: C:/Xolotl/.venv/Scripts/python.exe\n\
             xolotl_launcher: C:/Xolotl/tools/v41/serve_ds41.py\n\
             xolotl_cwd: D:/Models/Xolotl\n\
             xolotl_exe: C:/Xolotl/build/ds41_generate.exe\n\
             xolotl_tokenizer: C:/Xolotl/tok/tok.bin\n\
             xolotl_args: --backend cuda, --ctx 262144\n\
             xolotl_env: STRATA_PRESTAGE_GIB=8 GGML_NO_BACKTRACE=1\n\
             xolotl_lib_dirs: C:/Xolotl/lib\n\
             xolotl_gpu: 0\n\
             xolotl_sampling: temperature=1.0 top_p=0.95\n\
             xolotl_aliases: ds41, ds41-flash\n\
             xolotl_reasoning_budget: 32768\n\
             xolotl_vision: exe=C:/Xolotl/vision.exe mmproj=D:/m/v.gguf model=D:/m/m.gguf reference_dir=C:/Xolotl/ref gpu=false\n\
             xolotl_parallel: 2\n\
             xolotl_profile_extra: {\"conversation_cache\":true}\n\
             ninfer_autostart: /opt/a.sh\n\
             ninfer_container: golden-box\n\
             ninfer_profile: b\n\
             ninfer_key: true\n\
             ninfer_image: ninfer:local\n\
             ninfer_model_name: Golden-Artifact-Dir\n\
             ninfer_artifact: golden.ninfer\n\
             ninfer_internal_port: 8004\n\
             ninfer_model_roots: /mnt/d/Models,/home/ops/ai/models\n\
             ninfer_serve_args: --spec=dflash2 --draft-tokens=7\n\
             ninfer_gpu_device: \"0\"\n\
             ninfer_model_id: golden-id\n\
             ninfer_key_file: $HOME/.simpleharn/keys/golden-box.key\n\
             ninfer_serve_cmd: ninfer-serve\n\
             ninfer_env: CUDA_MODULE_LOADING=LAZY\n\
             external: false\n",
        )
        .expect("write golden");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog
            .errors
            .iter()
            .all(|e| e.kind != ScanKind::UnknownKey),
        "every const member must be a real, read key: {:?}",
        catalog.errors
    );
    let inert: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::InertKey)
        .collect();
    assert!(
        catalog.errors.iter().all(|e| e.kind == ScanKind::InertKey),
        "mock reads no knob: its only reports are the inert-key hints: {:?}",
        catalog.errors
    );
    let mut foreign: Vec<(&str, &str)> = Vec::new();
    for row in crate::engine::policy::all() {
        if row.id != "mock" {
            foreign.extend(row.knobs.iter().map(|key| (*key, row.id)));
        }
    }
    assert_eq!(
        inert.len(),
        foreign.len(),
        "exactly one hint per foreign engine knob: {:?}",
        catalog.errors
    );
    for (key, owner) in &foreign {
        let named = format!("`{key}` is read only by the {owner} engine(s)");
        assert!(
            inert.iter().any(|e| e.reason.contains(&named)),
            "knob `{key}` must report `{owner}` as its owner: {:?}",
            catalog.errors
        );
    }
    assert!(
        inert
            .iter()
            .any(|e| e.reason.contains("cpu_offload") && e.reason.contains("llama")),
        "the key the scoping ruling started with stays named where it was: {:?}",
        catalog.errors
    );
    for key in CONSUMED_KEYS {
        if *key == "external" {
            continue; // retired: consumed (never flagged) but stripped from raw
        }
        assert!(
            catalog.models[0].raw.contains_key(*key),
            "the golden fixture must actually exercise `{key}`"
        );
    }
}

/// The CPU-resident row shapes: a truthy
/// `cpu_offload` on an engine that reads the knob gates on RAM, so the
/// `whole` machine gate is a permanent contradiction — the scan refuses
/// the row with the named problem rather than publishing a row that can
/// never load. Falsifier: drop the refusal leg and the row publishes to
/// wait forever at load time.
#[test]
fn a_cpu_row_claiming_the_whole_gate_is_refused() {
    let dir = crate::testutil::temp_dir("scan-cpu-whole");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("cpu.yaml"),
        "engine: llama\ncpu_offload: true\nlaunch_only_with_at_least_vram_free: whole\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.iter().all(|m| m.name != "cpu"),
        "the contradictory row must not publish: {:?}",
        catalog.models
    );
    let problem = catalog
        .errors
        .iter()
        .find(|e| e.file == "cpu.yaml")
        .expect("the row is refused with a named problem");
    assert_eq!(problem.kind, ScanKind::PolicyRefused, "{problem:?}");
    assert!(
        problem.reason.contains("cpu_offload")
            && problem.reason.contains("whole")
            && problem.reason.contains("RAM"),
        "the line names both keys and the resource: {}",
        problem.reason
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same row shape without the contradiction: a numeric VRAM launch
/// floor on a CPU row is dead config, not an error — the inert-key warn
/// (entry publishes), same posture as every knob scoped out by the
/// knob table. Falsifier: dropping the warn makes the key look live to
/// the operator.
#[test]
fn a_numeric_vram_floor_on_a_cpu_row_warns_as_inert() {
    let dir = crate::testutil::temp_dir("scan-cpu-floor");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("cpu.yaml"),
        "engine: llama\ncpu_offload: true\nlaunch_only_with_at_least_vram_free: 8\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    let entry = catalog
        .models
        .iter()
        .find(|m| m.name == "cpu")
        .expect("the row still publishes");
    assert_eq!(entry.vram_gate.floor(), Some(8.0));
    assert!(
        catalog.errors.iter().any(|e| {
            e.kind == ScanKind::InertKey
                && e.file == "cpu.yaml"
                && e.reason.contains("launch_only_with_at_least_vram_free")
                && e.reason.contains("cpu_offload")
        }),
        "the dead VRAM floor must be warned, not silent: {:?}",
        catalog.errors
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The fold's persist→rescan round trip. The writer
/// (`MeasuredData::edits` in the scheduler) stamps every serving model with the
/// measured quartet; the scanner must read those keys back silently — a
/// harness-written key reported as "not read by the harness" is a false
/// statement to the operator and trips the live-ModelConf-clean gate. The
/// const-coverage pin missed this class because it checks the const against a
/// hand-listed literal set, not against what the WRITER emits.
#[test]
fn a_fold_written_yaml_rescans_without_unknown_key_reports() {
    let written = "engine: ninfer\n\
                       name: served\n\
                       ninfer_image: ninfer:local\n\
                       ninfer_container: ninfer-served\n\
                       vram_gb: 20\n\
                       measured_gb: 31.2\n\
                       measured_gb_min: 31.2\n\
                       measured_attribution: engine_report\n\
                       measured_profile: b\n";
    let (map, problems) = parse_flat_yaml(written);
    assert!(
        problems.is_empty(),
        "the fold's own write-back must rescan clean: {problems:?}"
    );
    assert_eq!(
        map.get("measured_attribution").map(String::as_str),
        Some("engine_report")
    );
}

/// The const pinned from the other side: the code's key literals are members.
/// `FLAG_TABLE` is the llama flag map read straight into argv; the manual list
/// mirrors the remaining `.get("<key>")` reads across the adapters and the
/// typed entry (adding a read without listing it here fails this test; adding
/// it to the const without reading it fails the golden test).
#[test]
fn consumed_keys_cover_every_key_the_code_reads() {
    for (key, _flag) in crate::engine::llama::FLAG_TABLE {
        assert!(
            CONSUMED_KEYS.contains(key),
            "FLAG_TABLE key `{key}` must be a consumed key"
        );
    }
    for key in [
        "name",
        "engine",
        "model",
        "category",
        "is_embedding_model",
        "vram_gb",
        "ram_gb",
        "cost",
        "launch_only_with_at_least_vram_free",
        "load_timeout",
        "min_turn",
        "serve_model",
        "cpu_offload",
        "jinja",
        "extra_args",
        "llama_server_exe",
        "measured_gb",
        "measured_gb_min",
        "measured_ram_gb",
        "measured_ram_gb_min",
        "measured_offload",
        "strata_config",
        "strata_python",
        "strata_cwd",
        "strata_exe",
        "strata_args",
        "strata_tokenizer",
        "strata_lib_dirs",
        "strata_env",
        "strata_gpu",
        "strata_parallel",
        "strata_vision",
        "strata_sampling",
        "strata_uncensored",
        "basalt_python",
        "basalt_cwd",
        "basalt_exe",
        "basalt_parallel",
        "basalt_engine_args",
        "maya_python",
        "maya_cwd",
        "maya_config",
        "maya_vision",
        "xolotl_python",
        "xolotl_launcher",
        "xolotl_cwd",
        "xolotl_exe",
        "xolotl_tokenizer",
        "xolotl_args",
        "xolotl_env",
        "xolotl_lib_dirs",
        "xolotl_gpu",
        "xolotl_sampling",
        "xolotl_aliases",
        "xolotl_reasoning_budget",
        "ninfer_autostart",
        "ninfer_container",
        "ninfer_profile",
        "ninfer_key",
        "ninfer_image",
        "ninfer_model_name",
        "ninfer_artifact",
        "ninfer_internal_port",
        "ninfer_model_roots",
        "ninfer_serve_args",
        "ninfer_gpu_device",
        "ninfer_model_id",
        "ninfer_key_file",
        "ninfer_serve_cmd",
        "external",
    ] {
        assert!(
            CONSUMED_KEYS.contains(&key),
            "`{key}` must be a consumed key"
        );
    }
    // The one key family the const cannot enumerate: the operator-chosen
    // custom-profile override is consumed by prefix rule (the adapter refuses a
    // selected custom profile whose line is absent, so the read is real).
    let pdir = crate::testutil::temp_dir("scan-profile-prefix");
    let _ = std::fs::remove_dir_all(&pdir);
    std::fs::create_dir_all(&pdir).expect("temp dir");
    std::fs::write(
            pdir.join("custom.yaml"),
            "engine: ninfer\nninfer_container: box\nninfer_profile: lt\nninfer_profile_lt: ctx=16384 kv=32768 conc=2\n",
        )
        .expect("write");
    std::fs::write(
        pdir.join("bad.yaml"),
        "engine: ninfer\nninfer_container: bx2\nninfer_profile_BAD-NAME: ctx=1 kv=1 conc=1\n",
    )
    .expect("write");
    let catalog = scan_dir(&pdir);
    assert!(
            catalog
                .errors
                .iter()
                .all(|e| e.kind != ScanKind::UnknownKey
                    || e.reason.contains("ninfer_profile_BAD-NAME")),
            "only the unsuffixable key may raise unknown-key: {:?}",
            catalog.errors
        );
    assert!(
            catalog
                .errors
                .iter()
                .any(|e| e.kind == ScanKind::UnknownKey
                    && e.reason.contains("ninfer_profile_BAD-NAME")),
            "a profile-override suffix the adapter could never name is dead config: {:?}",
            catalog.errors
        );
    let custom = catalog
        .models
        .iter()
        .find(|m| m.name == "custom")
        .expect("the valid-override entry publishes");
    assert!(
        custom.raw.contains_key("ninfer_profile_lt"),
        "the override line must survive into raw for the adapter to read"
    );
    let _ = std::fs::remove_dir_all(&pdir);
}

/// The fixture check the sweep left behind: the repo-root `ModelConf` copy
/// must carry no key the harness does not read. The live install's
/// `ModelConf` is covered by the operator rescan, not this test. Skipped
/// when the dir is absent (the repo copy is gitignored and machine-dependent).
#[test]
fn the_repo_modelconf_fixture_has_no_unknown_keys() {
    let dir = Path::new("ModelConf");
    if !dir.is_dir() {
        eprintln!("no ModelConf directory beside the test cwd; skipping");
        return;
    }
    let catalog = scan_dir(dir);
    let offenders: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::UnknownKey)
        .collect();
    assert!(
        offenders.is_empty(),
        "live ModelConf must scan with zero unknown-key errors: {offenders:?}"
    );
}

/// Fork C gate: the strict acceptance must not blind-side the live catalog.
/// "Fully clean" is stated as a closed debt ledger: the
/// live dir publishes every file whose bytes carry no harness-owned claim,
/// and the report ledger is exactly the ruled one — empty since the three
/// strata rows adopted their rendered template blocks (row 19;
/// the `strata_config` refusal is gone, `strata_exe` etc. carry the profile
/// the harness renders at load). The `context` inert hints that
/// once sat on the ninfer rows are cleaned up: the key is
/// live llama config and the ninfer context rides its `ninfer_profile`
/// line, so the rows no longer carry it. Every entry is pinned by file and
/// kind: a live file growing any other report, or an unplanned new file,
/// fails here, and the ledger shrinks as each cleanup gesture lands.
/// Windows-only: the fixture is this machine's Windows ModelConf copy — its
/// `F:\` paths cannot validate as absolute on another host. Skipped when
/// the dir is absent.
#[cfg(windows)]
#[test]
fn the_repo_modelconf_fixture_scans_fully_clean() {
    let dir = Path::new("ModelConf");
    if !dir.is_dir() {
        eprintln!(
            "no ModelConf directory beside the test cwd (looked for {}); skipping",
            dir.display()
        );
        return;
    }
    let accepted = std::fs::read_dir(dir)
        .expect("read live ModelConf dir")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .ok()
                .is_some_and(|file_type| is_accepted_config(&file_type, &entry.path()))
        })
        .count();
    assert!(accepted > 0, "an accepted-file count of zero pins nothing");
    let catalog = scan_dir(dir);
    let mut got: BTreeMap<String, &'static str> = BTreeMap::new();
    for error in &catalog.errors {
        let label = match error.kind {
            ScanKind::InertKey => "inert",
            ScanKind::PolicyRefused => "refused",
            ScanKind::InvalidYaml => {
                panic!("the repo ModelConf fixture grew an invalid-yaml file: {error:?}")
            }
            other => {
                panic!("the repo ModelConf fixture grew an unplanned {other:?} report: {error:?}")
            }
        };
        let first = got.insert(error.file.clone(), label);
        assert!(
            first.is_none(),
            "one file carried two ledger reports: {error:?}"
        );
    }
    let want: BTreeMap<&str, &str> = BTreeMap::new();
    let got: BTreeMap<&str, &str> = got
        .iter()
        .map(|(file, label)| (file.as_str(), *label))
        .collect();
    assert_eq!(
        got, want,
        "the repo ModelConf fixture ledger is closed: every report is one of the ruled 10c entries"
    );
    let refused = want.values().filter(|kind| **kind == "refused").count();
    assert_eq!(
        catalog.models.len(),
        accepted - refused,
        "every accepted live file except the ruled refusals publishes: {:?}",
        catalog.errors
    );
}

/// Two files, one identity: `from_flat` trims the stem it is handed, so
/// `qwen3.yaml` beside `qwen3 .yaml` publishes twice under one name — the shape
/// named when it asked for the `seen_names` twin of
/// `seen_containers`. Both rows publish (the scan reports, it never silences:
/// the old bytes stay visible) and **both** files are
/// reported, each naming every claimant, so neither wins by scan order. Falsifier:
/// delete the post-pass and the error list is empty with two rows under one name;
/// report only the later claimant (the `seen_containers` shape) and the first file
/// vanishes from the report, leaving it the silent favourite.
#[test]
fn two_files_claiming_one_name_are_both_reported_and_both_publish() {
    let dir = crate::testutil::temp_dir("dupname");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("qwen3.yaml"), "engine: llama\nvram_gb: 2\n").expect("write");
    // A stem whose trailing space the identity trim swallows: two distinct files,
    // one harness name. A `name:` key cannot do this — it must equal the stem.
    std::fs::write(dir.join("qwen3 .yaml"), "engine: llama\nvram_gb: 3\n").expect("write");

    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        2,
        "a collision is reported, never silenced: {:?}",
        catalog.errors
    );
    assert!(
        catalog.models.iter().all(|m| m.name == "qwen3"),
        "both rows really share one identity: {:?}",
        catalog
            .models
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>()
    );
    let offenders: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::DuplicateName)
        .collect();
    assert_eq!(
        offenders.len(),
        2,
        "every claimant is reported: {offenders:?}"
    );
    for (error, own) in offenders.iter().zip(["qwen3 .yaml", "qwen3.yaml"]) {
        assert_eq!(error.file, own, "one line per claiming file");
        assert!(
            error.reason.contains("qwen3.yaml") && error.reason.contains("qwen3 .yaml"),
            "the line names every claimant, not only the other one: {}",
            error.reason
        );
        assert!(
            error.reason.contains("name `qwen3`"),
            "the identity the two files collided on is named: {}",
            error.reason
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `kind` is part of the wire contract (`/har/status` and `/har/rescan` both
/// serialize `catalog_errors`): the JSON field names are pinned here.
#[test]
fn scan_error_kinds_serialize_as_snake_case() {
    let error = ScanError {
        kind: ScanKind::ModelPathMissing,
        file: "m.yaml".to_string(),
        reason: "model file not found: F:/x.gguf".to_string(),
    };
    let json = serde_json::to_value(&error).expect("serializable");
    assert_eq!(json["kind"], "model_path_missing");
    assert_eq!(json["file"], "m.yaml");
    assert_eq!(json["reason"], "model file not found: F:/x.gguf");
    for (kind, name) in [
        (ScanKind::InvalidName, "invalid_name"),
        (ScanKind::EngineBinaryMissing, "engine_binary_missing"),
        (ScanKind::EngineUnavailable, "engine_unavailable"),
        (ScanKind::UnknownKey, "unknown_key"),
        (ScanKind::InertKey, "inert_key"),
        (ScanKind::InvalidYaml, "invalid_yaml"),
        (ScanKind::DuplicateName, "duplicate_name"),
        (ScanKind::PolicyRefused, "policy_refused"),
        (ScanKind::Other, "other"),
    ] {
        let value = serde_json::to_value(ScanError {
            kind,
            file: "config".to_string(),
            reason: "x".to_string(),
        })
        .expect("serializable");
        assert_eq!(value["kind"], name, "{kind:?}");
    }
}

/// Seeded property test, reader half: generated nasty
/// documents. Invariants, every generated document: (a) `parse_flat_yaml`
/// never panics (catch_unwind); (b) never silent and never a phantom — the
/// independently computed line-by-line oracle (documented rules only: BOM +
/// trim normalization, comment/blank skip, first-colon split, key charset
/// `[A-Za-z_][A-Za-z0-9_-]*`, first-value-kept duplicates) must match the
/// problems vector exactly, line number and kind, and the published key set
/// exactly; (c) every published key stays inside the documented charset; (d)
/// parsing the same input twice answers identically (the reproducibility a
/// fuzzer owes). Deterministic by construction: fixed seeds, pure integer
/// PRNG. The value half of the split is not re-oracled here — its boundary
/// rule belongs to `split_value_comment` and is pinned by the torture
/// corpus; this test pins line classification and the never-silent
/// contract.
#[test]
fn seeded_fuzz_flat_yaml_never_panics_never_goes_silent_and_stays_deterministic() {
    use std::collections::BTreeSet;
    const SEEDS: [u64; 6] = [
        0x0005_DEEC_E66D,
        0xDEAD_BEEF_CAFE_BABE,
        0x0123_4567_89AB_CDEF,
        0xA5A5_A5A5_A5A5_A5A5,
        0,
        u64::MAX,
    ];
    const DOCS_PER_SEED: usize = 40;
    let mut total = 0usize;
    for seed in SEEDS {
        let mut rng = FuzzRng::new(seed);
        for doc_index in 0..DOCS_PER_SEED {
            let doc = fuzz_document(&mut rng);
            total += 1;
            let where_ = format!("seed {seed} doc {doc_index}");
            let parsed = std::panic::catch_unwind(|| parse_flat_yaml(&doc))
                .unwrap_or_else(|_| panic!("{where_}: parse_flat_yaml panicked on {doc:?}"));
            let (map, problems) = &parsed;

            let mut expected: Vec<(usize, &'static str)> = Vec::new();
            let mut expected_keys: BTreeSet<String> = BTreeSet::new();
            for (index, raw_line) in doc.lines().enumerate() {
                let line = normalized_line(raw_line);
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some(colon) = line.find(':') else {
                    expected.push((index + 1, "junk"));
                    continue;
                };
                let key = line[..colon].trim();
                let valid = !key.is_empty()
                    && key
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !valid {
                    expected.push((index + 1, "badkey"));
                    continue;
                }
                if expected_keys.contains(key) {
                    expected.push((index + 1, "dup"));
                    continue;
                }
                expected_keys.insert(key.to_string());
            }
            let got: Vec<(usize, &'static str)> = problems
                .iter()
                .map(|problem| {
                    let rest = problem
                        .strip_prefix("line ")
                        .unwrap_or_else(|| panic!("{where_}: a problem names no line: {problem}"));
                    let (number, text) = rest.split_once(':').unwrap_or_else(|| {
                        panic!("{where_}: unparsable problem header: {problem}")
                    });
                    let number: usize = number
                        .parse()
                        .unwrap_or_else(|_| panic!("{where_}: unparsable line number: {problem}"));
                    let text = text.trim_start();
                    let kind = if text.starts_with("not a `key: value` line") {
                        "junk"
                    } else if text.starts_with("invalid key") {
                        "badkey"
                    } else if text.starts_with("duplicate key") {
                        "dup"
                    } else {
                        panic!("{where_}: an unknown problem shape: {problem}");
                    };
                    (number, kind)
                })
                .collect();
            assert_eq!(
                    got, expected,
                    "{where_}: the problems vector disagrees with the line-by-line oracle; document: {doc:?}"
                );
            assert_eq!(
                map.keys().cloned().collect::<Vec<_>>(),
                expected_keys.into_iter().collect::<Vec<_>>(),
                "{where_}: the published key set disagrees with the oracle; document: {doc:?}"
            );
            for key in map.keys() {
                assert!(
                    !key.is_empty()
                        && key
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                    "{where_}: published key outside the documented charset: `{key}`"
                );
            }
            assert_eq!(
                parsed,
                parse_flat_yaml(&doc),
                "{where_}: parsing the same input twice disagreed"
            );
        }
    }
    assert_eq!(total, SEEDS.len() * DOCS_PER_SEED, "every seed ran");
}
