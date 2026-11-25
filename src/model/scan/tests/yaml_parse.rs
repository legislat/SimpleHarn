use super::*;

#[test]
fn quotes_and_inline_comments_are_handled() {
    let (map, problems) =
        parse_flat_yaml("name: \"qwen three\"\ncontext: 4096 # 32k\nengine: 'llama'\nempty:\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(map.get("name").map(String::as_str), Some("qwen three"));
    assert_eq!(map.get("context").map(String::as_str), Some("4096"));
    assert_eq!(map.get("engine").map(String::as_str), Some("llama"));
    assert_eq!(map.get("empty").map(String::as_str), Some(""));
}

#[test]
fn hash_inside_quotes_is_not_a_comment() {
    let (map, _) = parse_flat_yaml("model: \"F:/a#b.gguf\"\n");
    assert_eq!(map.get("model").map(String::as_str), Some("F:/a#b.gguf"));
}

/// THE boundary rule at the helper level: the comment begins at the first space-`#`
/// of the **normalized** value text, outside quotes, and the tail is verbatim
/// from that cut; the value is trimmed and unquoted. Leading whitespace can
/// therefore never manufacture a comment (` # tail` is the value `# tail`), a
/// tab never opens one, and a trailing `\r`/leading BOM is invisible to the
/// rule because the helper owns that normalization. Non-ASCII is pinned on
/// both paths (the pre-Fork-B code indexed a char vector with a byte length
/// and panicked on the no-cut path). The reader/writer agreement half of this
/// property is NOT tested here — a helper cannot audit its own callers; see
/// `writeback::tests::the_torture_shapes_round_trip_reader_to_writer_and_back`.
#[test]
fn the_shared_value_comment_split_cuts_only_outside_quotes() {
    assert_eq!(split_value_comment(" 4096 # 32k"), ("4096", Some(" # 32k")));
    assert_eq!(
        split_value_comment(" \"a # b\" # tail"),
        ("a # b", Some(" # tail"))
    );
    assert_eq!(split_value_comment(" 4096#c"), ("4096#c", None));
    assert_eq!(split_value_comment(" plain"), ("plain", None));
    assert_eq!(split_value_comment(""), ("", None));
    assert_eq!(
        split_value_comment(" \"unclosed # x"),
        ("\"unclosed # x", None)
    );
    assert_eq!(
        split_value_comment(" caf\u{e9} # morning"),
        ("caf\u{e9}", Some(" # morning"))
    );
    assert_eq!(
        split_value_comment(" \u{540d}\u{524d}"),
        ("\u{540d}\u{524d}", None)
    );
    assert_eq!(split_value_comment(" # tail"), ("# tail", None));
    assert_eq!(split_value_comment("  # a # b"), ("# a", Some(" # b")));
    assert_eq!(split_value_comment(" #"), ("#", None));
    assert_eq!(split_value_comment(" 4096\t# c"), ("4096\t# c", None));
    assert_eq!(split_value_comment(" 4096   # c"), ("4096", Some(" # c")));
    assert_eq!(split_value_comment(" 4096   "), ("4096", None));
    assert_eq!(split_value_comment(" 4096 # c\r"), ("4096", Some(" # c")));
    assert_eq!(
        split_value_comment("\u{feff} 4096 # c"),
        ("4096", Some(" # c"))
    );
    assert_eq!(split_value_comment("  \"a\""), ("a", None));
    // The helper against an independent expected table (not against its own
    // other face): the reader's published value for every shape.
    for &(raw, expected) in TORTURE_SHAPES {
        assert_eq!(
            split_value_comment(raw).0,
            expected,
            "reader's value for `{raw}`"
        );
    }
}

/// `unquote` peels one byte per side, which
/// is only a char-boundary-safe operation because the accepted quotes are
/// ASCII. Pinned with multi-byte content between the quotes so the assumption
/// is tested, not just documented.
#[test]
fn unquote_peels_ascii_quotes_around_multibyte_content() {
    assert_eq!(unquote("\"\u{540d}\u{524d}\""), "\u{540d}\u{524d}");
    assert_eq!(unquote("'\u{540d}\u{524d}'"), "\u{540d}\u{524d}");
    assert_eq!(unquote("\"\u{2192}\""), "\u{2192}");
    assert_eq!(unquote("\""), "\"");
    assert_eq!(unquote("\"\""), "");
    assert_eq!(unquote("\"\u{540d}\""), "\u{540d}");
    assert_eq!(unquote("'\u{540d}\""), "'\u{540d}\"");
}

#[test]
fn invalid_keys_and_duplicates_are_reported_not_dropped() {
    // `-` is a *valid* key character (the documented charset
    // wins), so the invalid-key half needs a genuinely illegal shape — a space.
    let (map, problems) = parse_flat_yaml("bad key: 1\ngood: 1\ngood: 2\ngpu-layers: 99\n");
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].contains("invalid key"));
    assert!(problems[1].contains("duplicate"));
    assert!(map.contains_key("gpu-layers"), "a hyphenated key parses");
}

/// Both directions pinned: the parser accepts exactly the
/// charset its doc comment promises — `[A-Za-z_][A-Za-z0-9_-]*` — and rejects
/// everything else, so neither side of the contract can drift again.
#[test]
fn the_parser_accepts_exactly_the_documented_key_charset() {
    // The doc claim, literally: every member of the documented classes parses
    // without a problem line.
    let (map, problems) =
        parse_flat_yaml("a: 1\nZ: 1\n_: 1\nA0: 1\na0-Z_9-x-y: 1\nwith-hyphen: 1\nx-a-b-c: 1\n");
    assert!(problems.is_empty(), "{problems:?}");
    for key in ["a", "Z", "_", "A0", "a0-Z_9-x-y", "with-hyphen", "x-a-b-c"] {
        assert!(map.contains_key(key), "`{key}` must parse");
    }
    // And the classes the doc excludes are still reported, never accepted.
    let (_, problems) = parse_flat_yaml("9lead: 1\nwith.dot: 1\nwith space: 1\n-hyphenfirst: 1\n");
    assert_eq!(problems.len(), 4, "{problems:?}");
    assert!(
        problems.iter().all(|p| p.contains("invalid key")),
        "{problems:?}"
    );
}

/// N4, first layer of [`adversarial_docs`]: every corpus document must (a)
/// never panic the parser, (b) respect its output shape - every reported
/// problem names a real line of that document and every published key obeys
/// the documented charset, (c) build the entry without a panic, and a clean
/// non-empty parse must land on a published entry or a **typed** reject, with
/// the three numeric-overflow shapes pinned to their exact variant, and
/// (d) parse identically on a second run - the reproducibility a generating
/// fuzzer would owe, asserted explicitly because the corpus replaced it.
#[test]
fn the_adversarial_corpus_never_panics_and_lands_typed_or_published() {
    use crate::model::ModelError;
    let reported = ["duplicate-keys", "colon-only", "junk-line", "space-key"];
    let empty_map = ["empty", "whitespace", "comment-only"];
    let overflow = ["vram-overflow", "timeout-overflow", "vram-negative-zero"];
    for (name, doc) in adversarial_docs() {
        let parsed = std::panic::catch_unwind(|| parse_flat_yaml(&doc))
            .unwrap_or_else(|_| panic!("case `{name}`: the parser panicked"));
        let (map, problems) = &parsed;
        let line_count = doc.lines().count();
        for problem in problems {
            let rest = problem
                .strip_prefix("line ")
                .unwrap_or_else(|| panic!("case `{name}`: a problem names no line: {problem}"));
            let number: usize = rest
                .split(':')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| {
                    panic!("case `{name}`: unparsable problem line number: {problem}")
                });
            assert!(
                (1..=line_count).contains(&number),
                "case `{name}`: a problem points outside the document: {problem}"
            );
        }
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
                "case `{name}`: published key outside the documented charset: `{key}`"
            );
        }
        assert_eq!(
            parsed,
            parse_flat_yaml(&doc),
            "case `{name}`: parsing the same input twice disagreed"
        );
        if reported.contains(&name) {
            assert!(
                !problems.is_empty(),
                "case `{name}`: a malformed document parsed clean: {map:?}"
            );
            continue;
        }
        if empty_map.contains(&name) {
            assert!(
                    problems.is_empty() && map.is_empty(),
                    "case `{name}`: blank input must yield no keys and no problems, got map {map:?} problems {problems:?}"
                );
            continue;
        }
        assert!(problems.is_empty(), "case `{name}`: {problems:?}");
        let built = std::panic::catch_unwind(|| ModelEntry::from_flat(map, "corpus"))
            .unwrap_or_else(|_| panic!("case `{name}`: from_flat panicked"));
        match built {
            Err(error) => match name {
                "vram-overflow" => assert!(
                    matches!(&error, ModelError::BadNumber { field, value }
                            if field == "vram_gb" && value == "1e400"),
                    "case `{name}`: {error}"
                ),
                "timeout-overflow" => assert!(
                    matches!(&error, ModelError::BadNumber { field, value }
                            if field == "load_timeout" && value == "99999999999999999999999999"),
                    "case `{name}`: {error}"
                ),
                "vram-negative-zero" => assert!(
                    matches!(&error, ModelError::BadNumber { field, .. } if field == "vram_gb"),
                    "case `{name}`: {error}"
                ),
                _ => panic!("case `{name}`: a clean shape answered a reject: {error}"),
            },
            Ok(entry) => {
                assert!(
                    !overflow.contains(&name),
                    "case `{name}`: a numeric overflow published an entry"
                );
                match name {
                    "valid-round-trip" => {
                        assert_eq!(entry.category, CategoryId::Work);
                        assert_eq!(entry.vram_gb, Some(4.5));
                        assert_eq!(entry.raw.get("context").map(String::as_str), Some("4096"));
                    }
                    "huge-line" => {
                        assert_eq!(entry.raw.get("note").map(String::len), Some(100_000));
                    }
                    "unknown-key" => assert_eq!(
                        entry.raw.get("frobnicate").map(String::as_str),
                        Some("yes"),
                        "unknown keys stay preserved for reference"
                    ),
                    "deep-quotes" => assert_eq!(
                        entry.raw.get("note").map(String::len),
                        Some(12),
                        "one quote pair peels, the chain residue is data"
                    ),
                    "deep-hash-chain" => {
                        assert_eq!(entry.raw.get("note").map(String::as_str), Some("a"));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The hyphenated-key rule: a hyphenated key is a *key*, not a
/// problem line — an unconsumed one publishes with the UnknownKey warn.
/// (`cache_type_k` uses an underscore, so `gpu-layers` is a genuinely unknown
/// hyphenated key.)
#[test]
fn a_hyphenated_key_is_a_key_not_a_problem_line() {
    let dir = crate::testutil::temp_dir("scan-hyphen-key");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("hp.yaml"),
        "engine: mock\nvram_gb: 2\ngpu-layers: 99\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(
        catalog.models.len(),
        1,
        "a hyphenated key never blocks publication: {:?}",
        catalog.errors
    );
    assert!(
        !catalog
            .errors
            .iter()
            .any(|e| e.kind == ScanKind::InvalidYaml),
        "the hyphenated key must not be a problem line: {:?}",
        catalog.errors
    );
    let error = catalog
        .errors
        .iter()
        .find(|e| e.kind == ScanKind::UnknownKey)
        .expect("an unconsumed hyphenated key is warn-only, like any unknown key");
    assert!(error.reason.contains("gpu-layers"), "{}", error.reason);
    assert!(
        catalog.models[0].raw.contains_key("gpu-layers"),
        "README: unknown keys stay preserved for reference"
    );
}

/// The reserved forensic key `bak:` publishes
/// with ZERO reports — no UnknownKey, no Other, nothing naming `bak` — while
/// a genuinely unknown key in the same file still reaches `raw` and still
/// earns its warn (silence is surgical, not a blanket mute). The value
/// exercises both `strip_comment` paths: a `#` inside quotes survives, an
/// inline ` # comment` after the quotes is cut.
#[test]
fn the_reserved_bak_key_publishes_silently_and_never_reaches_raw() {
    let dir = crate::testutil::temp_dir("scan-bak-silent");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("bak.yaml"),
        "engine: mock\nvram_gb: 2\n\
             bak: \"pre-quant snapshot # keep: F:/bak/x.gguf\" # forensic manual only\n\
             frobnicate: yes\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert_eq!(catalog.models.len(), 1, "{:?}", catalog.errors);
    assert_eq!(
        catalog.errors.len(),
        1,
        "only the genuinely unknown key may be reported: {:?}",
        catalog.errors
    );
    let warn = &catalog.errors[0];
    assert_eq!(warn.kind, ScanKind::UnknownKey);
    assert!(warn.reason.contains("frobnicate"), "{}", warn.reason);
    assert!(
        catalog.errors.iter().all(|e| !e.reason.contains("bak")),
        "no report may mention the bak node: {:?}",
        catalog.errors
    );
    let entry = &catalog.models[0];
    assert!(
        !entry.raw.contains_key("bak"),
        "the forensic value never reaches the raw passthrough: {:?}",
        entry.raw
    );
    assert_eq!(
        entry.raw.get("frobnicate").map(String::as_str),
        Some("yes"),
        "silence must not mute real unknown keys"
    );
}

/// Fork C consistency: the reserved word does not exempt a file from the
/// structural rules. A second `bak:` line is a duplicate-key problem line —
/// one InvalidYaml report naming it, entry never published.
#[test]
fn a_duplicate_bak_line_is_a_problem_line_and_drops_the_file() {
    let dir = crate::testutil::temp_dir("scan-bak-dup");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("twice.yaml"),
        "engine: mock\nvram_gb: 2\nbak: first\nbak: second\n",
    )
    .expect("write");
    let catalog = scan_dir(&dir);
    assert!(
        catalog.models.is_empty(),
        "a duplicate bak line must not publish: {:?}",
        catalog.models
    );
    let reports: Vec<&ScanError> = catalog
        .errors
        .iter()
        .filter(|e| e.kind == ScanKind::InvalidYaml)
        .collect();
    assert_eq!(reports.len(), 1, "{:?}", catalog.errors);
    assert!(reports[0].file.contains("twice.yaml"));
    assert!(
        reports[0].reason.contains("duplicate key `bak`"),
        "{}",
        reports[0].reason
    );
}

/// A key cannot be both read and silent: the two lists must not intersect,
/// and the silent word must stay out of `did_you_mean`'s candidate source
/// (CONSUMED_KEYS only), so forensic silence carries no breadcrumb either.
#[test]
fn the_silent_and_consumed_key_lists_are_disjoint() {
    for silent in SILENT_KEYS {
        assert!(
            !CONSUMED_KEYS.contains(silent),
            "`{silent}` cannot be both consumed and silent"
        );
        // The one-edit misspelling of a silent key owes no suggestion: the
        // candidate list is CONSUMED_KEYS alone.
        assert_eq!(did_you_mean(&format!("{silent}x")), "");
    }
    assert!(SILENT_KEYS.contains(&"bak"));
}

#[test]
fn junk_lines_are_reported() {
    let (_, problems) = parse_flat_yaml("just text\n");
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("not a `key: value` line"));
}
