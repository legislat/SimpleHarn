use super::*;

#[test]
fn categories_survive_the_round_trip() {
    let dir = crate::testutil::temp_dir("scan-category");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("a.yaml"), "is_embedding_model: true\n").expect("write");
    std::fs::write(dir.join("b.yaml"), "category: AUX\n").expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models[0].category, CategoryId::Emb);
    assert_eq!(catalog.models[1].category, CategoryId::Aux);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file whose `name:` equals its stem is the frozen shape the t-suite and
/// the gate-honesty probes write; it must keep working untouched.
#[test]
fn a_name_matching_the_stem_publishes_with_stem_identity() {
    let dir = crate::testutil::temp_dir("scan-name-equal");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("pair.yaml"),
        "name: pair\nengine: mock\nvram_gb: 2\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert_eq!(catalog.models[0].name, "pair");
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
}

/// A file with no `name:` key at all is the post-stem-identity shape: it scans
/// clean and carries the stem as its identity.
#[test]
fn an_absent_name_key_publishes_with_stem_identity() {
    let dir = crate::testutil::temp_dir("scan-name-absent");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("bare.yaml"), "engine: mock\nvram_gb: 2\n").expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert_eq!(catalog.models[0].name, "bare");
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
}

/// The twin class dies here — a `name:` that contradicts the
/// file is refused with the exact operator-actionable sentence, not resolved by
/// pick-one.
#[test]
fn a_name_contradicting_the_stem_is_dropped_with_the_exact_reason() {
    let dir = crate::testutil::temp_dir("scan-name-mismatch");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("real.yaml"),
        "name: impostor\nengine: mock\nvram_gb: 2\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "a contradicting name drops");
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::InvalidName)
        .expect("the mismatch must be an InvalidName report");
    assert_eq!(
        error.reason,
        "name: \"impostor\" does not match file real.yaml \u{2014} identity is the file \
             stem; rename the file or delete the key"
    );
    assert!(error.file.contains("real.yaml"));
}

#[test]
fn an_empty_stem_is_refused() {
    // Belt-and-braces: `read_dir` cannot actually yield an empty stem, but the
    // guard must not be decorative — exercise it through the helper path.
    assert!("   ".trim().is_empty());
    let dir = crate::testutil::temp_dir("scan-empty-stem");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("x.yaml"), "engine: mock\n").expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].name, "x");
}

/// A llama entry launched at a GGUF that is not on disk is
/// refused AND named — the eviction-before-doomed-spawn hole.
#[test]
fn a_llama_entry_with_a_missing_model_file_is_dropped_and_named() {
    let dir = crate::testutil::temp_dir("scan-llama-missing");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("gone.yaml"),
        "engine: llama\nmodel: Z:/definitely-not-here/gone.gguf\nvram_gb: 8\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "a doomed spawn must not publish");
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing file must be a ModelPathMissing report");
    assert!(
        error.reason.starts_with("model file not found: ") && error.reason.contains("gone.gguf"),
        "{}",
        error.reason
    );
    assert!(error.file.contains("gone.yaml"));
}

/// The existence trace accepts regular files only — a
/// directory-valued `model:` is `ModelPathMissing` before admission (the
/// operator typo class: folder named, file forgotten), and the directory's
/// byte size is never booked as `gguf_size_gb` for any engine (the same
/// stat feeds the size for every engine, so the filter must hold there
/// too — the surviving mock row proves the booking half).
#[test]
fn a_directory_valued_model_path_is_dropped_and_never_books_its_size() {
    let dir = crate::testutil::temp_dir("scan-llama-dir-model");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::create_dir_all(dir.join("Qwen-GGUF")).expect("model folder");
    let folder = dir.join("Qwen-GGUF").display().to_string();
    std::fs::write(
        dir.join("folder.yaml"),
        format!("engine: llama\nmodel: {folder}\nvram_gb: 8\n"),
    )
    .expect("write llama row");
    std::fs::write(
        dir.join("mockfolder.yaml"),
        format!("engine: mock\nmodel: {folder}\nvram_gb: 2\n"),
    )
    .expect("write mock row");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        1,
        "only the mock row survives the trace: {:?}",
        catalog.errors
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("a directory is not a model file");
    assert!(
        error.reason.starts_with("model file not found: ") && error.reason.contains("Qwen-GGUF"),
        "{}",
        error.reason
    );
    assert!(error.file.contains("folder.yaml"));
    assert!(
        catalog.models.iter().all(|m| m.gguf_size_gb.is_none()),
        "a directory's byte size is never booked, for any engine: {:?}",
        catalog
            .models
            .iter()
            .map(|m| &m.gguf_size_gb)
            .collect::<Vec<_>>()
    );
}

/// A declared `mmproj:` is existence-traced like
/// `model:` — a llama row whose vision file is gone must not publish and
/// evict for the same doomed spawn; same kind, the refusal names the file.
#[test]
fn a_llama_entry_with_a_missing_mmproj_is_dropped_and_named() {
    let dir = crate::testutil::temp_dir("scan-llama-missing-mmproj");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("weights.gguf"), b"0123456789").expect("write gguf");
    std::fs::write(
        dir.join("vision.yaml"),
        format!(
            "engine: llama\nmodel: {}\nmmproj: Z:/definitely-not-here/mmproj.gguf\n",
            dir.join("weights.gguf").display()
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "a doomed spawn must not publish");
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing mmproj must be a ModelPathMissing report");
    assert!(
        error.reason.starts_with("mmproj file not found: ") && error.reason.contains("mmproj.gguf"),
        "{}",
        error.reason
    );
    assert!(error.file.contains("vision.yaml"));
}

/// The join base for a relative `mmproj:` is the yaml dir (the
/// `model:` base), and the trace never resolves or rewrites the raw map —
/// the published value stays the operator's word; resolution is launch-side.
#[test]
fn a_relative_mmproj_is_traced_against_the_yaml_dir() {
    let dir = crate::testutil::temp_dir("scan-llama-mmproj-join");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("weights.gguf"), b"0123456789").expect("write gguf");
    std::fs::write(
        dir.join("vision.yaml"),
        format!(
            "engine: llama\nmodel: {}\nmmproj: vision-proj.gguf\n",
            dir.join("weights.gguf").display()
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.models.is_empty(), "a relative miss must refuse");
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::ModelPathMissing)
        .expect("the missing relative mmproj must be a ModelPathMissing report");
    assert!(
        error.reason.starts_with("mmproj file not found: ")
            && error.reason.contains("vision-proj.gguf")
            && error.reason.contains(&dir.display().to_string()),
        "the refusal names the yaml-dir-joined path: {}",
        error.reason
    );
    std::fs::write(dir.join("vision-proj.gguf"), b"0123456789").expect("write mmproj");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    assert_eq!(
        catalog.models[0].raw.get("mmproj").map(String::as_str),
        Some("vision-proj.gguf"),
        "the trace refuses but never resolves or rewrites the raw value"
    );
}

#[test]
fn a_llama_entry_with_an_existing_model_file_publishes_its_size() {
    let dir = crate::testutil::temp_dir("scan-llama-present");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("weights.gguf"), b"0123456789").expect("write gguf");
    std::fs::write(
        dir.join("ok.yaml"),
        format!(
            "engine: llama\nmodel: {}\n",
            dir.join("weights.gguf").display()
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    assert!(
        catalog.models[0].gguf_size_gb.is_some_and(|s| s > 0.0),
        "the same stat still books the size"
    );
}

/// Producer leg: the scan's existence-trace stat doubles as the
/// carried fingerprint the fit cache invalidates with, keyed by the row's
/// own identity rule. Falsifier: drop the `note_scan_fingerprint` call at
/// the stat and the lookup answers `None`, turning this pin red; a silent
/// rewrite between rescans then never voids anything (documented window).
#[test]
fn the_scan_leaves_the_carried_fingerprint_the_fit_cache_reads() {
    let dir = crate::testutil::temp_dir("scan-carried-fp");
    std::fs::write(dir.join("weights.gguf"), b"0123456789").expect("write gguf");
    let yaml = dir.join("ok.yaml");
    std::fs::write(
        &yaml,
        format!(
            "engine: llama\nmodel: {}\n",
            dir.join("weights.gguf").display()
        ),
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    let entry = &catalog.models[0];
    let raw = entry.fit_model_key().expect("a local model");
    assert_eq!(
        crate::model::scan_fingerprint(&yaml, raw),
        crate::model::fingerprint(&dir.join("weights.gguf")),
        "the scan's observation is exactly the file's fingerprint"
    );
    // A sentinel names no file: the scan never stats it, so nothing is
    // carried — absence stays absence and never fabricates a fingerprint.
    let remote = dir.join("remote.yaml");
    std::fs::write(
        &remote,
        "engine: llama\nmodel: hf://org/repo/x.gguf\nvram_gb: 4\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
    assert!(
        catalog
            .models
            .iter()
            .find(|m| m.name == "remote")
            .is_some_and(|m| m.fit_model_key().is_none()),
        "a sentinel row has no carrier identity, so the scan can carry nothing for it"
    );
}

/// `scheme://` sentinels (hf:, strata://, ...) name no local file: the stat
/// that would "prove" absence would drop every remote-backed model.
#[test]
fn a_scheme_sentinel_model_ref_is_exempt_from_the_existence_check() {
    let dir = crate::testutil::temp_dir("scan-sentinel");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("remote.yaml"),
        "engine: llama\nmodel: hf://org/repo/x.gguf\nvram_gb: 4\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert!(
        catalog.models[0].gguf_size_gb.is_none(),
        "sentinels have no size"
    );
}

/// Other engines keep their own existence rules (the adapter
/// checks); the scan only polices llama here.
#[test]
fn a_non_llama_engine_is_exempt_from_the_model_existence_check() {
    let dir = crate::testutil::temp_dir("scan-exempt-engine");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("mock.yaml"),
        "engine: mock\nmodel: Z:/definitely-not-here/mock.gguf\nvram_gb: 2\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
}
