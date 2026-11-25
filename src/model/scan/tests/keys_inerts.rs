use super::*;

/// The unknown key is *said*, never swallowed — and never
/// fatal: the entry still publishes (README: preserved for reference).
#[test]
fn an_unknown_key_warns_and_the_entry_still_publishes() {
    let dir = crate::testutil::temp_dir("scan-unknown-key");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("typo.yaml"),
        "engine: mock\nvram_gb: 2\nfrobnicate: yes\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        1,
        "warn-only: the model stays loadable"
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::UnknownKey)
        .expect("the unknown key must be reported");
    assert!(error.file.contains("typo.yaml"));
    assert!(error.reason.contains("frobnicate"), "{}", error.reason);
}

#[test]
fn the_host_and_port_policy_keys_get_their_exact_hints() {
    let dir = crate::testutil::temp_dir("scan-policy-keys");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("inert.yaml"),
        "engine: mock\nvram_gb: 2\nhost: 0.0.0.0\nport: 8080\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "policy hints are warn-only too");
    assert!(
        catalog.errors.iter().any(|e| e.kind == ScanKind::UnknownKey
            && e.reason
                == "engine host is harness-forced (loopback, \u{a7}5) \u{2014} this key \
                        does nothing"),
        "{:?}",
        catalog.errors
    );
    assert!(
        catalog.errors.iter().any(|e| e.kind == ScanKind::UnknownKey
            && e.reason
                == "engine port is harness-assigned from category config \u{2014} this \
                        key does nothing"),
        "{:?}",
        catalog.errors
    );
}

/// B of the `cpu_offload` scoping ruling: a non-offload engine carrying the
/// key gets the NEW warn kind (never `UnknownKey` — the key is real harness
/// config, owned by a different engine), and the entry still publishes exactly
/// like the retired `host:`/`port:` hints. Falsifier: any change that swallows
/// the report (removing the engine filter from the check), reports it under
/// `UnknownKey`, or drops the entry would fail here.
#[test]
fn cpu_offload_on_a_non_offload_engine_is_reported_inert_key_and_publishes() {
    let dir = crate::testutil::temp_dir("scan-inert-key");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("strata-off.yaml"),
        "engine: strata\nvram_gb: 8\ncpu_offload: true\n",
    )
    .expect("write strata");
    std::fs::write(
        dir.join("ninfer-off.yaml"),
        "engine: ninfer\nvram_gb: 26\ncpu_offload: false\n",
    )
    .expect("write ninfer");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        2,
        "warn-only: both entries still publish: {:?}",
        catalog.errors
    );
    let inert: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::InertKey)
        .collect();
    assert_eq!(
        inert.len(),
        2,
        "one per non-llama row: {:?}",
        catalog.errors
    );
    assert!(
        catalog
            .errors
            .iter()
            .all(|e| e.kind != ScanKind::UnknownKey),
        "a consumed key owned by another engine is not a typo: {:?}",
        catalog.errors
    );
    for (file, engine) in [("strata-off.yaml", "strata"), ("ninfer-off.yaml", "ninfer")] {
        let hit = inert
            .iter()
            .find(|e| e.file.contains(file))
            .unwrap_or_else(|| panic!("{file} must be reported: {catalog:?}"));
        assert!(
            hit.reason.contains("cpu_offload")
                && hit.reason.contains(engine)
                && hit.reason.contains("llama"),
            "the reason must name key, engine and the owning set: {}",
            hit.reason
        );
    }
    // Value does not matter: `false` reports exactly like `true`.
    assert!(catalog
        .errors
        .iter()
        .any(|e| e.kind == ScanKind::InertKey && e.file.contains("ninfer-off")));
}

/// The same key on llama rows — explicit `engine: llama` and the absent-key
/// default both — stays fully silent, preserving today's posture for the two
/// live catalog rows (`Qwen3-0.6B-GGUF`, `embeddinggemma-2-GGUF`). Falsifier:
/// scoping the const away from llama, or an absent engine key resolving to a
/// non-offload id, produces a spurious report here.
#[test]
fn cpu_offload_on_llama_rows_produces_no_report() {
    let dir = crate::testutil::temp_dir("scan-llama-silent");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("explicit.yaml"),
        "engine: llama\nmodel: mock://m\ncpu_offload: true\n",
    )
    .expect("write explicit");
    std::fs::write(
        dir.join("default.yaml"),
        "model: mock://m\ncpu_offload: true\n",
    )
    .expect("write default");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 2, "{:?}", catalog.errors);
    assert!(
        catalog.errors.is_empty(),
        "llama (incl. the absent-key default) must stay silent: {:?}",
        catalog.errors
    );
    assert_eq!(catalog.models[0].engine, "llama");
    assert_eq!(catalog.models[1].engine, "llama");
}

/// Sibling-knob scoping: the mechanism that scoped
/// `cpu_offload` now covers every row knob — a llama key on a ninfer row
/// warns `InertKey` and publishes exactly like its offload sibling, and
/// the key stays silent on its own row. This is the live catalog's shape:
/// the ninfer/strata yamls carry `context:` the harness only reads for
/// llama, and the page must be able to say so (warn-only, ruling D posture).
/// Falsifier: a hardcoded `cpu_offload` key filter, or scoping the knob
/// table away from the scan, fails one leg here.
#[test]
fn sibling_llama_knobs_are_inert_off_the_llama_row() {
    let dir = crate::testutil::temp_dir("scan-sibling-knobs");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("ninfer-row.yaml"),
        "engine: ninfer\nvram_gb: 26\ncontext: 4096\nngl: 99\n",
    )
    .expect("write ninfer");
    std::fs::write(
        dir.join("llama-row.yaml"),
        "engine: llama\nmodel: mock://m\ncontext: 4096\n",
    )
    .expect("write llama");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        2,
        "warn-only: both entries still publish: {:?}",
        catalog.errors
    );
    for key in ["context", "ngl"] {
        let named = format!("`{key}` is read only by the llama engine(s)");
        assert!(
            catalog.errors.iter().any(|e| e.kind == ScanKind::InertKey
                && e.file.contains("ninfer-row")
                && e.reason.contains(&named)),
            "`{key}` on the ninfer row must warn: {:?}",
            catalog.errors
        );
    }
    assert!(
        catalog
            .errors
            .iter()
            .all(|e| !e.file.contains("llama-row") || e.kind != ScanKind::InertKey),
        "a knob on its own row stays silent: {:?}",
        catalog.errors
    );
}

/// The row-binary rule, scan half: a llama row override that cannot launch —
/// blank or
/// relative — is refused with the key named and never published; an
/// absolute value publishes silently (existence stays the adapter's
/// floor). On a foreign row the key is the usual `InertKey` hint, never a
/// refusal. Falsifier: drop the scan gate and a relative override
/// publishes as a loadable row that dies at spawn after eviction.
#[test]
fn a_llama_row_override_that_cannot_launch_is_refused_naming_the_key() {
    let dir = crate::testutil::temp_dir("scan-llama-exe");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("m.gguf"), b"weights").expect("seed model file");
    std::fs::write(
        dir.join("relative.yaml"),
        "engine: llama\nmodel: m.gguf\nllama_server_exe: llama-server.exe\n",
    )
    .expect("write relative");
    std::fs::write(
        dir.join("blank.yaml"),
        "engine: llama\nmodel: m.gguf\nllama_server_exe: \"   \"\n",
    )
    .expect("write blank");
    // The gate checks this host's absolute-path shape, so the absolute
    // leg must be spelled in the host grammar to exercise the pass door.
    #[cfg(windows)]
    let abs_override = "D:/llama/llama-server.exe";
    #[cfg(not(windows))]
    let abs_override = "/opt/llama/llama-server";
    std::fs::write(
        dir.join("absolute.yaml"),
        format!("engine: llama\nmodel: m.gguf\nllama_server_exe: {abs_override}\n"),
    )
    .expect("write absolute");
    std::fs::write(
        dir.join("foreign.yaml"),
        "engine: strata\nllama_server_exe: X\n",
    )
    .expect("write foreign");
    let catalog = scan_dir(&dir);
    let names: Vec<&str> = catalog.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["absolute", "foreign"],
        "unlaunchable overrides are refused, the rest publish: {:?}",
        catalog.errors
    );
    for file in ["relative.yaml", "blank.yaml"] {
        let refused = catalog
            .errors
            .iter()
            .find(|e| e.file.contains(file) && e.kind == ScanKind::PolicyRefused)
            .unwrap_or_else(|| panic!("{file} must be refused: {:?}", catalog.errors));
        assert!(
            refused.reason.contains("llama_server_exe"),
            "the refusal names the key: {}",
            refused.reason
        );
    }
    assert!(
        catalog.errors.iter().any(|e| {
            e.kind == ScanKind::InertKey
                && e.file.contains("foreign.yaml")
                && e.reason
                    .contains("`llama_server_exe` is read only by the llama engine(s)")
        }),
        "on a strata row the key is the usual inert hint: {:?}",
        catalog.errors
    );
    assert!(
        catalog
            .errors
            .iter()
            .all(|e| !e.file.contains("absolute.yaml")),
        "an absolute override on its own row is silent: {:?}",
        catalog.errors
    );
}

/// The legacy-removal ruling: a row that still declares the removed
/// legacy `ninfer_autostart` path is **refused** at scan and never published,
/// with the migration sentence naming every template key the row must adopt;
/// the pair with `ninfer_image` is refused too, and a modern template row
/// stays silent. Falsifier: the interim behavior published the legacy row with
/// a `Deprecated` warn, so `models.len()` and the refusal-kind checks fail on
/// the old code; a message missing one migration key fails its own loop.
#[test]
fn a_legacy_autostart_row_is_refused_and_never_published() {
    let dir = crate::testutil::temp_dir("scan-legacy-refused");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
            dir.join("legacy-row.yaml"),
            "engine: ninfer\nvram_gb: 20\nninfer_autostart: /opt/legacy.sh\nninfer_container: legacy-box\n",
        )
        .expect("write legacy");
    std::fs::write(
            dir.join("both-row.yaml"),
            "engine: ninfer\nvram_gb: 20\nninfer_autostart: /opt/both.sh\nninfer_image: ninfer:local\nninfer_container: both-box\n",
        )
        .expect("write both");
    std::fs::write(
        dir.join("modern-row.yaml"),
        "engine: ninfer\nvram_gb: 20\nninfer_image: ninfer:local\nninfer_container: modern-box\n",
    )
    .expect("write modern");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        1,
        "only the template row publishes: {:?}",
        catalog.errors
    );
    assert_eq!(catalog.models[0].name, "modern-row");
    let refused: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::PolicyRefused)
        .collect();
    assert_eq!(
        refused.len(),
        2,
        "both the verbatim legacy row and the pair with `ninfer_image` are refused: {:?}",
        catalog.errors
    );
    for error in &refused {
        for needle in [
            "`ninfer_autostart`",
            "removed",
            "`ninfer_image`",
            "`ninfer_model_name`",
            "`ninfer_artifact`",
            "`ninfer_model_roots`",
        ] {
            assert!(
                error.reason.contains(needle),
                "the refusal must name `{needle}`: {}",
                error.reason
            );
        }
    }
    assert!(
        catalog
            .errors
            .iter()
            .all(|e| !e.file.contains("modern-row")),
        "the template row must stay completely silent: {:?}",
        catalog.errors
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Full `extra_args` enforcement: a hostile extra word refuses
/// the FILE at rescan — never published, never reaching a spawn that could
/// evict a resident for the doomed launch — and the refusal names both the
/// word and the mechanism that owns the decision. The benign family the
/// live catalog uses still publishes untouched.
#[test]
fn a_hostile_extra_word_refuses_the_file_naming_the_word_and_owner() {
    let dir = crate::testutil::temp_dir("scan-hostile-extra");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("llama-bad.yaml"),
        "engine: llama\nmodel: mock://m\nextra_args: --api-key sekrit\n",
    )
    .expect("write bad llama");
    std::fs::write(
        dir.join("llama-good.yaml"),
        "engine: llama\nmodel: mock://m\nextra_args: --embedding --pooling last\n",
    )
    .expect("write good llama");
    std::fs::write(
        dir.join("ninfer-bad.yaml"),
        "engine: ninfer\nvram_gb: 26\nninfer_serve_args: --max-context 999\n",
    )
    .expect("write bad ninfer");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        1,
        "only the benign row publishes: {:?}",
        catalog.errors
    );
    assert_eq!(catalog.models[0].name, "llama-good");
    let bad = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::PolicyRefused && e.file.contains("llama-bad"))
        .expect("the hostile llama row must be refused");
    assert!(
        bad.reason.contains("`--api-key`")
            && bad
                .reason
                .contains("request authentication is harness-owned")
            && !bad.reason.contains("sekrit"),
        "the refusal names the word and its owner, not the secret value: {}",
        bad.reason
    );
    let bad = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::PolicyRefused && e.file.contains("ninfer-bad"))
        .expect("the hostile ninfer row must be refused");
    assert!(
        bad.reason.contains("`--max-context`")
            && bad.reason.contains("ninfer_profile")
            && bad.reason.contains("harness-owned flag"),
        "the refusal names the word and the profile line that owns it: {}",
        bad.reason
    );
}

/// The 22[1] doctrine, row 19 shape: policy fields are harness-forced
/// and never obeyed from engine-side files — and the render makes claiming
/// them structurally impossible: a rendered profile carries no `host`, no
/// `port`, no key (the bind and port ride the forced argv), while the
/// defense-in-depth leg of `validate_profile_text` still refuses any JSON
/// that shows one. The scan-side witness is that every clean template row
/// publishes with those fields absent; the leg itself is pinned in the
/// adapter's module.
#[test]
fn a_template_row_publishes_with_the_policy_fields_absent_by_construction() {
    let dir = crate::testutil::temp_dir("scan-profile-policy");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("strata.exe"), b"x").expect("write exe");
    std::fs::write(dir.join("tokenizer.bin"), b"t").expect("write tokenizer");
    std::fs::write(
        dir.join("clean.yaml"),
        format!(
            "engine: strata\nvram_gb: 8\nserve_model: clean\nstrata_exe: {}\n\
                 strata_tokenizer: {}\nstrata_cwd: {}\n",
            dir.join("strata.exe").display(),
            dir.join("tokenizer.bin").display(),
            dir.display(),
        ),
    )
    .expect("write clean yaml");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert_eq!(catalog.models[0].name, "clean");
}
