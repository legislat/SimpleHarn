use super::*;

#[test]
fn scan_dir_collects_models_and_names_bad_files() {
    let dir = crate::testutil::temp_dir("scan-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("a.yaml"), "engine: mock\nvram_gb: 4\n").expect("write");
    std::fs::write(dir.join("bad.yaml"), "oops\n").expect("write");
    std::fs::write(dir.join("ignored.txt"), "name: nope\n").expect("write");

    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "only the yaml counts");
    assert_eq!(catalog.models[0].name, "a");
    assert_eq!(
        catalog.models[0]
            .file
            .as_ref()
            .map(|p| p.file_name().unwrap().to_str().unwrap()),
        Some("a.yaml")
    );
    assert_eq!(catalog.errors.len(), 1);
    assert_eq!(catalog.errors[0].kind, ScanKind::InvalidYaml);
    assert!(catalog.errors[0].file.contains("bad.yaml"));
    assert!(
        catalog.errors[0]
            .reason
            .starts_with("yaml not loaded \u{2014} problem lines: "),
        "{}",
        catalog.errors[0].reason
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The zero-key hole, closed in the direction the doctrine
/// points: a `.yaml` or `.yml` the acceptance gate treats as a config but which
/// yields no keys at all publishes nothing **and is reported**, exactly as
/// `commit_verified` refuses to write it (`EmptyFile`). Before the gate these
/// files entered the catalog as `engine: llama` rows named by their stems with
/// `errors` empty — spawnable-shaped phantoms. The report names the file it
/// refused, never a hard-coded extension: a comment-only `.yml` must not be
/// called a `.yaml`. Falsifier: delete the `map.is_empty()` gate and the rows
/// come back while `errors` empties; keep the pre-fix message and the
/// `reason.contains(name)` assertion on `notes.yml` goes red.
#[test]
fn an_empty_or_comment_only_yaml_is_reported_and_never_publishes() {
    let dir = crate::testutil::temp_dir("scan-zero-key");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("blank.yaml"), "").expect("seed blank");
    std::fs::write(dir.join("notes.yaml"), "# only a comment\n\n").expect("seed notes");
    std::fs::write(dir.join("notes.yml"), "# only a comment\n\n").expect("seed notes yml");
    std::fs::write(dir.join("real.yaml"), "engine: mock\nvram_gb: 4\n").expect("seed real");

    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog
            .models
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["real"],
        "a file with no keys must not publish a row, phantom or otherwise"
    );
    assert_eq!(catalog.errors.len(), 3, "{:?}", catalog.errors);
    for name in ["blank.yaml", "notes.yaml", "notes.yml"] {
        let Some(report) = catalog.errors.iter().find(|e| e.file.contains(name)) else {
            panic!(
                "{name} was neither published nor reported: {:?}",
                catalog.errors
            );
        };
        assert_eq!(report.kind, ScanKind::InvalidYaml, "{report:?}");
        assert!(
            report.reason.contains("no `key: value` lines"),
            "the reason must say which zero-key shape this is: {:?}",
            report.reason
        );
        assert!(
            report.reason.contains(name),
            "the reason must name the refused file, not a fixed extension: {:?}",
            report.reason
        );
        assert!(
            !report.reason.contains("problem lines"),
            "the problem-line sentence belongs to the other gate: {:?}",
            report.reason
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fork C doctrine, pinned to its exact shape: a file with problem lines is
/// NEVER published, and one report names the file and every offending line.
#[test]
fn a_file_with_problem_lines_is_never_published_and_every_line_is_named() {
    let dir = crate::testutil::temp_dir("scan-invalid-yaml");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("broken.yaml"),
        "engine: mock\nvram_gb: 2\njust text\nbad key: 1\ncontext: 1\ncontext: 2\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "nothing is published blindly: a problem line drops the whole file"
    );
    let errors: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::InvalidYaml)
        .collect();
    assert_eq!(errors.len(), 1, "exactly ONE report: {:?}", catalog.errors);
    assert!(errors[0].file.contains("broken.yaml"), "{:?}", errors[0]);
    assert_eq!(
        errors[0].reason,
        "yaml not loaded \u{2014} problem lines: line 3: not a `key: value` line: `just \
             text`; line 4: invalid key `bad key`; line 6: duplicate key `context` (first \
             value kept)"
    );
}

/// The UnknownKey/InvalidYaml boundary (Fork C's first-class test): dead keys
/// are not broken files. Valid syntax with a dead key publishes with a warn;
/// one junk line never publishes; both never publishes; a lone duplicate never
/// publishes.
#[test]
fn a_dead_key_publishes_but_a_problem_line_never_does() {
    let dir = crate::testutil::temp_dir("scan-dead-key-boundary");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("deadkey.yaml"),
        "engine: mock\nvram_gb: 2\nfrobnicate: yes\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("junk.yaml"),
        "engine: mock\nvram_gb: 2\njust text\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("both.yaml"),
        "engine: mock\nvram_gb: 2\nfrobnicate: yes\njust text\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("dup.yaml"),
        "engine: mock\nvram_gb: 2\nvram_gb: 3\n",
    )
    .expect("write");

    let catalog = scan_dir(&dir);
    let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["deadkey"],
        "only the dead-key file publishes: {names:?}"
    );

    let dead = catalog
        .errors
        .iter()
        .filter(|e| e.file.contains("deadkey.yaml"))
        .collect::<Vec<_>>();
    assert!(
        dead.iter().all(|e| e.kind == ScanKind::UnknownKey)
            && dead.iter().any(|e| e.reason.contains("frobnicate")),
        "the dead key is warn-only: {dead:?}"
    );
    for (file, needle) in [
        ("junk.yaml", "not a `key: value` line"),
        ("both.yaml", "not a `key: value` line"),
        ("dup.yaml", "duplicate key"),
    ] {
        let report = catalog
            .errors
            .iter()
            .find(|e| e.kind == ScanKind::InvalidYaml && e.file.contains(file))
            .unwrap_or_else(|| {
                panic!(
                    "{file} must be refused as invalid yaml: {:?}",
                    catalog.errors
                )
            });
        assert!(report.reason.contains(needle), "{}", report.reason);
    }
    assert!(
        !catalog
            .errors
            .iter()
            .any(|e| e.kind == ScanKind::Other && e.file.contains("junk.yaml")),
        "problem lines travel as InvalidYaml, never scattered Other reports"
    );
}

#[test]
fn duplicate_ninfer_container_names_are_refused() {
    // Unload reaps by name: one container may back exactly one model
    // (review, 2026-10-26).
    let dir = crate::testutil::temp_dir("scan-dup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("a.yaml"),
        "engine: ninfer\nninfer_image: ninfer:local\nninfer_container: shared\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("b.yaml"),
        "engine: ninfer\nninfer_image: ninfer:local\nninfer_container: shared\n",
    )
    .expect("write");
    std::fs::write(
        dir.join("c.yaml"),
        "engine: ninfer\nninfer_image: ninfer:local\nninfer_container: other\n",
    )
    .expect("write");

    let catalog = scan_dir(&dir);
    let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"a") && names.contains(&"c"), "{names:?}");
    assert!(
        !names.contains(&"b"),
        "the duplicate must be refused: {names:?}"
    );
    assert!(
        catalog.errors.iter().any(|e| e.file.contains("b.yaml")
            && e.kind == ScanKind::Other
            && e.reason.contains("duplicate ninfer_container")),
        "{:?}",
        catalog.errors
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_zero_vram_gb_file_is_named_not_silently_dropped() {
    // `vram_gb: 0` books the model free at admission, so the
    // scan must refuse it — and name the file, as it does for every sibling
    // reject, so /har/status says which yaml the operator has to fix.
    let dir = crate::testutil::temp_dir("scan-vram0");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("free.yaml"), "engine: mock\nvram_gb: 0\n").expect("write");

    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a free booking must never enter the catalog"
    );
    assert!(
        catalog
            .errors
            .iter()
            .any(|e| e.file.contains("free.yaml") && e.reason.contains("vram_gb")),
        "the file must be named in catalog_errors: {:?}",
        catalog.errors
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_dir_is_one_error_not_a_panic() {
    let catalog = scan_dir(Path::new("Z:/definitely-not-here"));
    assert!(catalog.models.is_empty());
    assert!(catalog.errors.len() == 1);
    assert!(catalog.errors[0].reason.contains("cannot read"));
}

/// Fork B acceptance matrix (`.yml` joined the gate by Doctrine 10's
/// override): `*.yaml` and `*.yml` regular files (any case) are the only
/// config shape; `README.md`, a `.txt`, a name with no extension, `.yaml.bak`,
/// `.YAML.bak` and even a directory named `dir.yaml` are INERT with ZERO
/// catalog errors (the anti-noise pin — a stray junk file must not start
/// talking). Falsifier for the override: with only `.yaml` accepted the
/// `*.yml` rows vanish and the expected-name list can never match.
#[test]
fn only_regular_dot_yaml_and_dot_yml_files_are_configs_and_the_rest_is_silent() {
    let dir = crate::testutil::temp_dir("scan-gate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("good.yaml"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("CAPS.YAML"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("mixed.YmL"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("fine.yml"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("TALL.YML"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("README.md"), "# ModelConf notes\n").expect("write");
    // Junk inside a non-candidate: if the filter ever parsed it, an
    // InvalidYaml report would appear and the errors-empty assert would bite.
    std::fs::write(dir.join("x.yaml.bak"), "oops no colon\n").expect("write");
    std::fs::write(dir.join("x.yml.bak"), "oops no colon\n").expect("write");
    std::fs::write(dir.join("UPPER.YAML.bak"), "oops no colon\n").expect("write");
    std::fs::write(dir.join("notes.txt"), "oops no colon\n").expect("write");
    std::fs::write(dir.join("noext"), "oops no colon\n").expect("write");
    std::fs::create_dir_all(dir.join("dir.yaml")).expect("dir named yaml");
    std::fs::create_dir_all(dir.join("dir.yml")).expect("dir named yml");

    let catalog = scan_dir(&dir);
    let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["CAPS", "TALL", "fine", "good", "mixed"],
        "exactly the *.yaml and *.yml regular files: {names:?}"
    );
    assert!(
        catalog.errors.is_empty(),
        "non-candidates are silent-inert, never noise: {:?}",
        catalog.errors
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The predicate pinned as a unit, both directions, using real `FileType`s
/// (a regular file's and a directory's) so the `is_file` half is not
/// decorative either. `.yml` joins the accepted set case-insensitively by
/// Doctrine 10's override; every other shape stays rejected.
#[test]
fn the_extension_gate_accepts_dot_yaml_and_dot_yml_case_insensitively() {
    let dir = crate::testutil::temp_dir("scan-gate-predicate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("probe.txt"), "x").expect("write probe");
    std::fs::create_dir_all(dir.join("probe.d")).expect("make dir");
    let (mut file_type, mut dir_type) = (None, None);
    for entry in std::fs::read_dir(&dir).expect("read dir") {
        let entry = entry.expect("entry");
        if entry.file_name() == "probe.txt" {
            file_type = Some(entry.file_type().expect("file type"));
        } else if entry.file_name() == "probe.d" {
            dir_type = Some(entry.file_type().expect("dir type"));
        }
    }
    let file_type = file_type.expect("probe file");
    let dir_type = dir_type.expect("probe dir");
    for name in [
        "m.yaml",
        "m.YAML",
        "m.YaMl",
        "model.yaml",
        "m.yml",
        "m.YML",
        "m.YmL",
        "model.yml",
    ] {
        assert!(is_accepted_config(&file_type, Path::new(name)), "{name}");
    }
    for name in [
        "x.yaml.bak",
        "x.yml.bak",
        "UPPER.YAML.bak",
        "README.md",
        "notes.txt",
        "noext",
        ".yaml",
        ".yml",
        "m.yamlx",
        "m.ymlx",
        // The write-back temp shape: a killed
        // write can never leave a scannable file behind.
        "qwen3.yaml.4242.7.sh.tmp",
        "qwen3.yml.4242.7.sh.tmp",
    ] {
        assert!(!is_accepted_config(&file_type, Path::new(name)), "{name}");
    }
    // Right name, wrong shape: a directory named like a config is inert.
    assert!(!is_accepted_config(&dir_type, Path::new("d.yaml")));
    assert!(!is_accepted_config(&dir_type, Path::new("d.yml")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pipe/linked-tree hang shape the reviewer feared dies here: a symlink
/// is judged by `file_type()` (never followed), so a `*.yaml` link is inert.
/// Creating symlinks on Windows needs privilege (SeCreateSymbolicLinkPrivilege
/// or Developer Mode) — when the OS refuses, the non-`is_file` rejection is
/// still pinned by the directory case above and the std contract that
/// `DirEntry::file_type` reports the link itself.
#[test]
#[cfg(windows)]
fn a_symlink_is_inert_even_when_it_targets_a_valid_yaml() {
    let dir = crate::testutil::temp_dir("scan-symlink");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let real = dir.join("real.yaml");
    std::fs::write(&real, "engine: mock\nvram_gb: 2\n").expect("write");
    let link = dir.join("linked.yaml");
    match std::os::windows::fs::symlink_file(&real, &link) {
        Ok(()) => {
            let catalog = scan_dir(&dir);
            let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
            assert_eq!(names, ["real"], "the symlink must not publish: {names:?}");
            assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
        }
        Err(error) => {
            // Falsifier role is unchanged either way; the privilege fact is
            // stated, not papered over.
            eprintln!(
                "symlink_file refused on this host ({error}) — unprivileged Windows \
                     without Developer Mode; the non-regular-file rejection stays pinned \
                     by the dir.yaml directory case and the `is_accepted_config` unit test."
            );
            assert!(!link.exists() || link.is_symlink());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `.yml` acceptance falsifier (Doctrine 10's override): a valid
/// `foo.yml` publishes exactly like a `.yaml` row — stem identity, own file
/// path, zero reports — and the uppercase `.YML` shape publishes too
/// (case-insensitive). Falsifier: on the pre-change gate both files were
/// inert, so `catalog.models` came back empty and this assert could never
/// match.
#[test]
fn a_valid_yml_row_publishes_exactly_like_a_yaml_row() {
    let dir = crate::testutil::temp_dir("scan-yml-publish");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("foo.yml"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("BAR.YML"), "engine: mock\nvram_gb: 3\n").expect("write");

    let catalog = scan_dir(&dir);
    let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["BAR", "foo"], "{:?}", catalog.errors);
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    for (name, own) in [("BAR", "BAR.YML"), ("foo", "foo.yml")] {
        let entry = catalog
            .models
            .iter()
            .find(|m| m.name == name)
            .expect("published row");
        assert_eq!(
            entry
                .file
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned()),
            Some(own.to_string()),
            "the row keeps its own file, not a renamed twin"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fork C doctrine travels to `.yml` unchanged: a broken `foo.yml` is a
/// config with problem lines, so it NEVER publishes and one named InvalidYaml
/// report says so (the same voice `.yaml` has always used). Falsifier: on the
/// pre-change gate the file was inert — zero errors and zero rows — and the
/// InvalidYaml report this assert requires did not exist.
#[test]
fn a_broken_yml_is_never_published_and_its_problem_lines_are_named() {
    let dir = crate::testutil::temp_dir("scan-yml-broken");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("broken.yml"),
        "engine: mock\nvram_gb: 2\njust text\nbad key: 1\n",
    )
    .expect("write");

    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "nothing is published blindly: a problem line drops the whole file"
    );
    let errors: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::InvalidYaml)
        .collect();
    assert_eq!(errors.len(), 1, "exactly ONE report: {:?}", catalog.errors);
    assert!(errors[0].file.contains("broken.yml"), "{:?}", errors[0]);
    assert!(
        errors[0]
            .reason
            .starts_with("yaml not loaded \u{2014} problem lines: "),
        "{}",
        errors[0].reason
    );
    assert!(
        errors[0].reason.contains("just text"),
        "{}",
        errors[0].reason
    );
    assert!(errors[0].reason.contains("bad key"), "{}", errors[0].reason);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The anti-overreach pin beside the new `.yml` acceptance: everything that
/// was silently inert before stays silently inert now — `.txt`, a name with
/// no extension, an empty `.yml` twin class stays honest only for accepted
/// files — and non-candidates still produce ZERO reports.
#[test]
fn txt_extensionless_and_backup_names_stay_silent_next_to_an_accepted_yml() {
    let dir = crate::testutil::temp_dir("scan-yml-inert-neighbours");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("ok.yml"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("notes.txt"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("noext"), "oops no colon\n").expect("write");
    std::fs::write(dir.join("ok.yml.bak"), "oops no colon\n").expect("write");

    let catalog = scan_dir(&dir);
    let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["ok"], "only the accepted file publishes: {names:?}");
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Identity stays the file stem, so `m.yaml` beside `m.yml` collapses to one
/// harness identity and must surface the EXISTING `DuplicateName` post-pass
/// refusal — both rows publish, **both** files are reported, each naming
/// every claimant (the same posture as the whitespace-suffix twins pinned in
/// `two_files_claiming_one_name_are_both_reported_and_both_publish`; no new
/// rule). Falsifier: pre-change `m.yml` was inert — one row, zero
/// `DuplicateName` reports — so neither assert could match.
#[test]
fn a_yaml_and_yml_pair_on_one_stem_reports_the_existing_duplicate_name() {
    let dir = crate::testutil::temp_dir("scan-yml-dupname");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("m.yaml"), "engine: mock\nvram_gb: 2\n").expect("write");
    std::fs::write(dir.join("m.yml"), "engine: mock\nvram_gb: 3\n").expect("write");

    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        2,
        "the pair collapses to one identity and both rows still publish: {:?}",
        catalog.errors
    );
    assert!(
        catalog.models.iter().all(|m| m.name == "m"),
        "both rows really share one identity"
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
    for (error, own) in offenders.iter().zip(["m.yaml", "m.yml"]) {
        assert_eq!(error.file, own, "one line per claiming file");
        assert!(
            error.reason.contains("m.yaml") && error.reason.contains("m.yml"),
            "the line names every claimant, not only the other one: {}",
            error.reason
        );
        assert!(
            error.reason.contains("name `m`"),
            "the identity the two files collided on is named: {}",
            error.reason
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
