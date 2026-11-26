//! ModelConf scan: flat `key: value` yaml files -> typed catalog,
//! with per-file error reports.
//!
//! v3.3 lessons encoded here: quotes and inline comments are handled (D2 — `name:
//! "foo"` and `context: 4096 # 32k` parse correctly), and a file with problem lines
//! is never published; its lines are named (D3, operator doctrine: "better not
//! publish anything blindly"), while the scan itself stays non-fatal.
//!
//! Catalog-trust rules: identity is the yaml FILE
//! STEM — a `name:` key is legacy-optional and only legal when it equals the stem,
//! because that value is the one component of a yaml that reaches a filesystem path
//! (the engine log name); a llama entry whose local `model:` or declared `mmproj:`
//! file is gone, and a
//! strata entry whose profile JSON is unreadable or references a missing absolute
//! path, are refused here with the file named, before admission can evict a resident
//! engine for a doomed spawn; and a key outside `CONSUMED_KEYS` is reported
//! (warn-only — README doctrine: unknown keys stay "preserved for reference").
//! This module stays log-free: the actor reports every [`ScanError`] it applies.

use crate::engine::ninfer::PROFILE_NAME_MAX_BYTES;
use crate::model::ModelEntry;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

/// What kind of problem a [`ScanError`] reports. The page and the actor's `tracing`
/// lines triage on this; `reason` stays the human sentence (G13). Closed enum:
/// every construction site names one of these, and a new report shape must add a
/// variant so the exhaustiveness audit happens at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    /// The model's backing file — or a declared `mmproj:`, or a path its strata
    /// profile references — is not a regular file on disk; the entry was dropped.
    ModelPathMissing,
    /// A `name:` key contradicts the file stem, or the stem is empty; dropped.
    InvalidName,
    /// A configured engine binary does not exist (a `config` cross-check, raised by
    /// the actor's `apply_catalog`, not by the scan itself).
    EngineBinaryMissing,
    /// The entry names an engine this harness has no adapter for; dropped by the
    /// actor (the scan cannot see the adapter map).
    EngineUnavailable,
    /// A yaml key outside `CONSUMED_KEYS` (and outside `SILENT_KEYS`, the one
    /// reserved forensic class that stays silent by operator ruling); warn-only,
    /// the entry still publishes.
    UnknownKey,
    /// A real harness key — a member of `CONSUMED_KEYS` — that only the rows
    /// of *other* engines list in their `EnginePolicy` `knobs` cell, appearing
    /// on a model whose row does not read it. The opposite of
    /// [`ScanKind::UnknownKey`] (a key the harness owns nowhere, i.e. a typo):
    /// here the key is live config for a *different* engine, so it is not a
    /// typo and cannot be silenced by `CONSUMED_KEYS` membership alone.
    /// Warn-only, exactly the retired `host:`/`port:` policy-hint posture: the
    /// entry still publishes. The mechanism is the knob table (the
    /// generalization of the `cpu_offload` scoping rule): every
    /// engine-scoped key — `cpu_offload` and its llama siblings, the strata,
    /// basalt, maya and ninfer key families — is scoped the same data-driven
    /// way, so a new row knob needs no new scan code.
    InertKey,
    /// A yaml file the scan refuses to publish (Fork C doctrine): it carried at
    /// least one problem line — then the `reason` names every offending line — or it
    /// parsed clean but yielded **no keys at all**, which the `reason` says outright
    /// (the shape `commit_verified` refuses as `EmptyFile`).
    InvalidYaml,
    /// Two published rows collapse to one harness identity, so every name-keyed
    /// path — `/har/load`, the write-back, the measured fold — would silently prefer
    /// whichever row the scan order happened to put first. Warn-only
    /// like the other report classes: **both** files publish and **both** are reported,
    /// naming every claimant; the API's `409` stays the refusal, so nothing here is a
    /// boot-style rejection of a file that is otherwise loadable.
    DuplicateName,
    /// The file claims a harness-owned surface and is **refused** (never published):
    /// an operator-extra word (`extra_args`, `ninfer_serve_args`) outside the engine
    /// row's closed word surface, a policy field (`host`, `port`, key) inside the
    /// strata profile it points at, or a ninfer row still declaring the removed
    /// `ninfer_autostart` launch key (the legacy-removal ruling). Engine-side
    /// files
    /// decide nothing (22\[1]), so the refusal names the word/field and the
    /// mechanism that owns the decision.
    PolicyRefused,
    /// Every pre-existing report shape: unreadable files or dirs, typed field
    /// rejects, duplicate containers, unresolved `default_model`s.
    Other,
}

/// One scan problem: a file (or `config` for cross-file checks) and the reason.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanError {
    /// What kind of problem this is (page/triage key).
    pub kind: ScanKind,
    /// File the problem came from, or `"config"` for cross-file checks.
    pub file: String,
    /// Human-readable reason (G13: names the thing, not a stack trace).
    pub reason: String,
}

/// The scan result: every parseable model plus every problem found.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Catalog {
    /// Parsed entries, sorted by harness name for stable listings.
    pub models: Vec<ModelEntry>,
    /// Problems: bad files, bad keys, unresolved `default_model`s.
    pub errors: Vec<ScanError>,
}

impl Catalog {
    /// Empty catalog (a missing ModelConf dir is not fatal — §0.1 "no config still
    /// works").
    pub fn empty() -> Self {
        Self::default()
    }
}

/// The canonical set of yaml keys the harness reads anywhere: scan identity, the
/// typed entry, the llama flag table and its extra reads, the strata, basalt,
/// maya and ninfer adapters, and the measured triple. This const is THE source of truth for the
/// unknown-key report: a key outside it is published with a warn-only
/// [`ScanKind::UnknownKey`] (entry stays loadable — README: "preserved for
/// reference"). The `consumed_keys_cover_every_key_the_code_reads` test fails when
/// the FLAG_TABLE or a documented engine-specific read is added without joining
/// this list, and the golden-yaml test fails when a member here stops being read.
const CONSUMED_KEYS: &[&str] = &[
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
    "mmproj",
    "context",
    "ngl",
    "batch",
    "ubatch",
    "flash_attn",
    "cache_type_k",
    "cache_type_v",
    "parallel",
    "fit",
    "fit_target",
    "fit_ctx",
    "llama_server_exe",
    "measured_gb",
    "measured_gb_min",
    "measured_ram_gb",
    "measured_ram_gb_min",
    "measured_offload",
    "measured_attribution",
    "measured_profile",
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
    "strata_aliases",
    "strata_reasoning_budget",
    "strata_profile_extra",
    "strata_uncensored",
    "basalt_python",
    "basalt_cwd",
    "basalt_exe",
    "basalt_parallel",
    "basalt_vision",
    "basalt_engine_args",
    "basalt_sampling",
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
    "xolotl_vision",
    "xolotl_parallel",
    "xolotl_profile_extra",
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
    "ninfer_env",
    "external",
];

/// Reserved forensic keys the operator hand-writes into a yaml: one-shot snapshot
/// notes and relevant comments, for the forensic manual only (operator ruling,
/// on the `bak:` node). The harness never reads them, never warns
/// about them, never logs them, and never surfaces them: they are stripped from
/// the parsed raw map in [`crate::model::ModelEntry::from_flat`] exactly like the
/// retired `external`, so the value can neither reach the page nor be re-emitted
/// by write-back — the operator's file bytes are never altered. Deliberately NOT
/// members of [`CONSUMED_KEYS`]: membership there claims a real read, and the
/// golden-yaml test enforces that every member is actually read.
pub(crate) const SILENT_KEYS: &[&str] = &["bak"];

/// The `host:` key is inert by policy, not by accident: the
/// harness forces the loopback bind itself (§5).
const HOST_POLICY_HINT: &str =
    "engine host is harness-forced (loopback, \u{a7}5) \u{2014} this key does nothing";

/// Same for `port:` — allocated from the category config, never self-chosen.
const PORT_POLICY_HINT: &str =
    "engine port is harness-assigned from category config \u{2014} this key does nothing";

/// Builds the [`ScanKind::InertKey`] sentence for one engine-scoped key: the
/// same named-policy-hint posture as [`HOST_POLICY_HINT`]/[`PORT_POLICY_HINT`]
/// (the Fork C knob table: the owners come from the table itself,
/// so every row knob is scoped by the same mechanism, not by a key list here).
/// `pub(crate)` because the API's refusal to WRITE an inert key quotes the
/// identical sentence (the scan hint and the 400 must never diverge).
pub(crate) fn inert_key_reason(key: &str, engine: &str) -> String {
    format!(
        "`{key}` is read only by the {} engine(s) \u{2014} engine `{engine}` never reads it; \
         this key does nothing here",
        crate::engine::policy::knob_readers(key).join("/")
    )
}

/// Parses one flat yaml document into a key map plus per-line problems.
///
/// Rules: `key: value` with `key` matching `[A-Za-z_][A-Za-z0-9_-]*`; surrounding
/// single/double quotes stripped; ` # ...` inline comments removed outside quotes;
/// blank/comment lines skipped; anything else is reported, never dropped silently.
///
/// The comment rule, exactly (`split_value_comment` is the one owner of it):
/// a `#` opens a comment only when an ASCII **space** precedes it outside quotes.
/// Two deliberate consequences, symmetric on both sides of the reader/writer pair:
/// a **tab never opens a comment**
/// (`key:\t# note` publishes `# note` as the value), and leading whitespace is
/// trimmed *before* the walk, so a `#` at the very start of the value text is
/// value too (`key: # tail` publishes `# tail`). Quote such a value (`"# tail"`)
/// when you mean it literally and want nothing read as a comment after it.
pub fn parse_flat_yaml(text: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let mut map = BTreeMap::new();
    let mut problems = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = normalized_line(raw_line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            problems.push(format!(
                "line {}: not a `key: value` line: `{line}`",
                index + 1
            ));
            continue;
        };
        let key = line[..colon].trim();
        if key.is_empty()
            || !key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            problems.push(format!("line {}: invalid key `{key}`", index + 1));
            continue;
        }
        let value = strip_comment(&line[colon + 1..]);
        if map.contains_key(key) {
            problems.push(format!(
                "line {}: duplicate key `{key}` (first value kept)",
                index + 1
            ));
            continue;
        }
        map.insert(key.to_string(), value);
    }
    (map, problems)
}

/// Splits a raw `key:` value text into (parsed value, verbatim inline comment).
///
/// This is THE boundary rule **and the only normalizer of value text**:
/// scan parses through here and the write-back compares a line's value text
/// through here, so no caller can hand the rule a differently-normalized text and
/// the reader and the writer cannot disagree about where the value ends. The
/// incoming word a caller asks to write is NOT pre-normalized at all: it is the
/// byte-exact target that a candidate line form must read back to
/// (`crate::model::writeback::emit_form` feeds candidates through this same
/// helper). Normalization
/// (surrounding whitespace, a line's trailing `\r`, a leading BOM) happens
/// **inside**, before the walk: the comment begins at the first space `#`
/// **outside** quotes of the *normalized* text and is returned verbatim from that
/// cut (so its ` #` opening and inner spacing survive); the value is what precedes
/// the cut, trimmed, with one surrounding quote pair stripped. `None` when no such
/// cut exists. Trimming first has two pinned consequences: a `#` that only looked
/// space-preceded because of leading whitespace is value, and a tab never opens a
/// comment.
/// Walking `char_indices` (never a char vector indexed by a byte length, which is
/// what panicked on non-ASCII before this rewrite) keeps every cut a char
/// boundary and needs no allocation; the final character is visited with no
/// successor, which can never form a ` #` cut, so no rule depends on looking one
/// character ahead.
pub(crate) fn split_value_comment(raw_value: &str) -> (&str, Option<&str>) {
    let text = normalize_value_text(raw_value);
    let mut in_quote: Option<char> = None;
    let mut cut: Option<usize> = None;
    let mut chars = text.char_indices().peekable();
    while let Some((index, current)) = chars.next() {
        match in_quote {
            Some(q) if current == q => in_quote = None,
            Some(_) => {}
            None => {
                if current == '"' || current == '\'' {
                    in_quote = Some(current);
                } else if current == ' ' && chars.peek().is_some_and(|&(_, next)| next == '#') {
                    cut = Some(index);
                    break;
                }
            }
        }
    }
    match cut {
        Some(index) => (unquote(text[..index].trim()), Some(&text[index..])),
        None => (unquote(text), None),
    }
}

/// The single normalization a value text passes through, so the walk sees one
/// shape no matter which caller produced it: surrounding whitespace (including a
/// CRLF line's trailing `\r`) and a leading BOM, which `str::trim` leaves alone.
/// Trimmed twice because a BOM can ride inside the leading whitespace, and the
/// helper must still return slices of the caller's own bytes.
fn normalize_value_text(raw_value: &str) -> &str {
    raw_value.trim().trim_start_matches('\u{feff}').trim()
}

/// The one line normalizer both sides of the reader/writer pair use: a document
/// line is its BOM-stripped, trimmed text. `str::trim` alone does not remove
/// U+FEFF (category Cf, not White_Space), so the explicit strip is what keeps a
/// BOM'd first line visible to the reader and the writer alike; the rule lives
/// here and nowhere else, so the two cannot disagree about which line a key is on.
pub(crate) fn normalized_line(line: &str) -> &str {
    line.trim_start_matches('\u{feff}').trim()
}

/// Strips one surrounding quote pair (`"…"` / `'…'`) from a trimmed value.
///
/// Sound by assumption: both accepted quotes are one ASCII byte, so peeling one
/// byte per side always lands on a char boundary — `unquote_peels_ascii_quotes_
/// around_multibyte_content` pins that with multi-byte content between the quotes.
fn unquote(trimmed: &str) -> &str {
    if trimmed.len() >= 2 {
        let first = trimmed.chars().next().expect("non-empty checked");
        let last = trimmed.chars().next_back().expect("non-empty checked");
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

/// Removes an inline ` # comment` (outside quotes) and surrounding quote pairs.
fn strip_comment(value: &str) -> String {
    split_value_comment(value).0.to_string()
}

/// The torture set both layers are pinned against: every value-text shape whose
/// boundary the reader and the writer must
/// agree on, paired with the value the reader publishes. The cross-layer pins in
/// `writeback` feed each published value back through the writer and demand a
/// byte-identical document, so a writer-local comment parser cannot survive the
/// suite — the acceptance criterion the self-comparison it replaces could not
/// meet (that loop was `strip_comment` asserted against `split_value_comment`,
/// one function in two faces, and stayed green with a private parser in the
/// writer).
#[cfg(test)]
pub(crate) const TORTURE_SHAPES: &[(&str, &str)] = &[
    (" 4096 # 32k", "4096"),
    (" \"a # b\" # tail", "a # b"),
    (" 'a # b' # tail", "a # b"),
    (" \"4096\"", "4096"),
    (" 'a'", "a"),
    (" 4096#c", "4096#c"),
    (" 4096 #32k", "4096"),
    (" plain", "plain"),
    (" 4096   ", "4096"),
    (" 4096   # c", "4096"),
    ("", ""),
    (" ", ""),
    (" #", "#"),
    (" #tail", "#tail"),
    (" # tail", "# tail"),
    ("  # a # b", "# a"),
    (" 4096\t# c", "4096\t# c"),
    (" 4096\t# c # tail", "4096\t# c"),
    (" \"unclosed # x", "\"unclosed # x"),
    (" caf\u{e9} # morning", "caf\u{e9}"),
    (" \u{540d}\u{524d}", "\u{540d}\u{524d}"),
    (" \u{2192}\u{2192} 4096 # arrows", "\u{2192}\u{2192} 4096"),
    (" caf\u{65}\u{301} # morning", "caf\u{65}\u{301}"),
    // The peel-edge class: the quote peel happens AFTER the
    // slice is trimmed, so a published value may legitimately carry edge whitespace
    // or a residual quote pair. Every one of these must round-trip byte-exactly.
    (" \"a b \" # tail", "a b "),
    (" \" a\"", " a"),
    (" 'a ' # tail", "a "),
    (" \"a\t\" # tail", "a\t"),
    (" \"\"\"\" # tail", "\"\""),
    (" '''' # tail", "''"),
    (" \"\u{feff}x\" # tail", "\u{feff}x"),
];

/// The deterministic adversarial corpus for the hand-rolled flat-YAML reader
/// (N4, "no fuzz/property test"): one fixed document list shared by both
/// layers — `scan::tests` drives parse and entry building through it,
/// `writeback::tests` drives the identical-write no-op and the value
/// round-trip through it. A true property test needs a generating fuzz
/// harness, which would be a new dependency; the dependency policy refuses
/// that, so this corpus is the deterministic stand-in: every input fixed,
/// every outcome asserted, and the reproducibility a fuzzer owes is asserted
/// explicitly (parse twice, compare). Invalid UTF-8 is not a case and cannot
/// be: the reader's input is a `&str`, so a byte-sequence reject happens
/// upstream of this module, at the file read.
#[cfg(test)]
pub(crate) fn adversarial_docs() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("whitespace", "   \t \n \n".to_string()),
        ("comment-only", "# just a note\n\n   \n".to_string()),
        ("bom", "\u{feff}name: corpus\nengine: mock\n".to_string()),
        (
            "crlf",
            "name: corpus\r\nengine: mock\r\nvram_gb: 4\r\n".to_string(),
        ),
        ("quotes", "name: \"corpus\"\nengine: 'mock'\n".to_string()),
        (
            "inline-comments",
            "name: corpus # named\ncontext: 4096 # 32k\nengine: mock\n".to_string(),
        ),
        (
            "duplicate-keys",
            "name: corpus\nengine: mock\nengine: llama\n".to_string(),
        ),
        (
            "unknown-key",
            "name: corpus\nengine: mock\nfrobnicate: yes\n".to_string(),
        ),
        ("huge-line", format!("name: corpus\nnote: {}\n", "x".repeat(100_000))),
        (
            "control-chars",
            "name: corpus\nweird: a\u{1}b\u{7}\nengine: mock\n".to_string(),
        ),
        (
            "deep-quotes",
            format!("name: corpus\nnote: {}\nengine: mock\n", "\"".repeat(14)),
        ),
        (
            "deep-hash-chain",
            "name: corpus\nnote: a # b # c # d # e # f # g\nengine: mock\n".to_string(),
        ),
        ("colon-only", ":\n:::\n".to_string()),
        ("junk-line", "just text\nname: corpus\n".to_string()),
        ("space-key", "bad key: 1\nname: corpus\n".to_string()),
        (
            "vram-overflow",
            "name: corpus\nengine: mock\nvram_gb: 1e400\n".to_string(),
        ),
        (
            "timeout-overflow",
            "name: corpus\nengine: mock\nload_timeout: 99999999999999999999999999\n".to_string(),
        ),
        (
            "vram-negative-zero",
            "name: corpus\nengine: mock\nvram_gb: -0.0\n".to_string(),
        ),
        (
            "many-keys",
            (1..=200)
                .map(|i| format!("filler_key_{i:03}: {i}\n"))
                .collect(),
        ),
        (
            "valid-round-trip",
            "# operator header\nname: corpus # identity\nengine: mock\n category: WORK\nvram_gb: 4.5 # budget\ncontext: \"4096\" # quoted\r\n".to_string(),
        ),
    ]
}

/// xorshift64* — the hand-rolled seeded PRNG behind the seeded property
/// tests (algorithm: Vigna's xorshift64*: `x ^= x >> 12; x ^= x << 25;
/// x ^= x >> 27;` then multiply by `0x2545F4914F6CDD1D`). Pure wrapping integer
/// math, so every run on every platform replays the identical case sequence; a
/// zero state is illegal for xorshift, so seed `0` is replaced by the golden
/// ratio constant. This replaces a generating fuzz dependency the policy
/// refuses: fixed seeds in, deterministic cases out, asserted reproducible.
#[cfg(test)]
pub(crate) struct FuzzRng {
    state: u64,
}

#[cfg(test)]
impl FuzzRng {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(crate) fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }

    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
}

/// The fixed nasty-value corpus for the property tests: empty and blank values,
/// tabs, NUL and control characters, unicode (including astral), quotes and
/// quote-parity traps, backslashes (including a trailing one), `#` in every
/// position that matters, inline comments, leading/trailing whitespace, a
/// ride-along BOM, huge integers and very long payloads. No value carries `\n`
/// (the reader splits the document on `\n` and the writer refuses such a word
/// outright — that refusal is pinned separately in `writeback`).
#[cfg(test)]
pub(crate) fn fuzz_value_pool() -> Vec<String> {
    let mut pool: Vec<String> = [
        "",
        "   ",
        "\t",
        "4096",
        "4096 # 32k",
        "4096 #32k",
        "4096#c",
        " # tail",
        "\"a # b\"",
        "'a # b'",
        "\"unclosed",
        "unclosed\"",
        "\"\"",
        "''",
        "a \\ b",
        "C:\\str\\ing",
        "trail\\",
        "F:/a#b.gguf",
        "a b ",
        " a",
        "a\tb",
        "\ta\t",
        "a\u{0}b",
        "a\u{1}b\u{7}",
        "\u{7f}",
        "\u{feff}",
        "caf\u{e9} \u{540d}\u{524d} \u{2192}",
        "\u{1F600}",
        "99999999999999999999999999999999999999",
        "-0.0",
        "1e400",
        "a=b",
        "key: inside",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    pool.push("x".repeat(5_000));
    pool.push(format!("preamble {} trailer", "#".repeat(200)));
    pool.push(" ".repeat(40));
    pool.push("\u{feff}x".to_string());
    pool
}

/// Builds one generated document from `rng`: a run of 1..=24 lines drawn from
/// the nasty shapes — valid pairs (sometimes repeating a key to drive the
/// duplicate class, sometimes carrying a pool value), invalid-key pairs, colon
/// lines, bare junk, comments, blanks, and tab/whitespace variants — joined by
/// `\n` or CRLF at random, sometimes BOM-led. Every case is a pure function of
/// the seed.
#[cfg(test)]
pub(crate) fn fuzz_document(rng: &mut FuzzRng) -> String {
    const KEYS: [&str; 8] = [
        "alpha",
        "Z9_k",
        "x-y",
        "_lead",
        "kebab-case-key",
        "engine",
        "vram_gb",
        "q_1",
    ];
    const BAD_KEYS: [&str; 6] = ["9bad", "bad key", "bad.key", "-lead", "9", "dot.dot"];
    let values = fuzz_value_pool();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let count = 1 + rng.below(24);
    for _ in 0..count {
        let shape = rng.below(6);
        let line = match shape {
            0 => {
                let key = if !seen.is_empty() && rng.chance(25) {
                    seen[rng.below(seen.len())]
                } else {
                    KEYS[rng.below(KEYS.len())]
                };
                seen.push(key);
                format!("{key}: {}", values[rng.below(values.len())])
            }
            1 => {
                let key = BAD_KEYS[rng.below(BAD_KEYS.len())];
                format!("{key}: {}", values[rng.below(values.len())])
            }
            2 => match rng.below(4) {
                0 => "just text".to_string(),
                1 => "!! bang".to_string(),
                2 => String::new(),
                _ => "   ".to_string(),
            },
            3 => match rng.below(3) {
                0 => "# note".to_string(),
                1 => "   # indented note".to_string(),
                _ => "\t# tabbed note".to_string(),
            },
            4 => match rng.below(4) {
                0 => ":".to_string(),
                1 => ":::".to_string(),
                2 => "engine:".to_string(),
                _ => "alpha: \t value \t ".to_string(),
            },
            _ => {
                let key = if !seen.is_empty() && rng.chance(25) {
                    seen[rng.below(seen.len())]
                } else {
                    KEYS[rng.below(KEYS.len())]
                };
                seen.push(key);
                format!("  {key}: {}  ", values[rng.below(values.len())])
            }
        };
        lines.push(line);
    }
    let joiner = if rng.chance(40) { "\r\n" } else { "\n" };
    let mut doc = lines.join(joiner);
    if rng.chance(60) {
        doc.push_str(joiner);
    }
    if rng.chance(20) {
        doc.insert(0, '\u{feff}');
    }
    doc
}

/// The 13[5] Fork B acceptance gate, `.yml`-extended by Doctrine 10's override:
/// a config is exactly
/// a **regular file** whose final extension is `.yaml` or `.yml`,
/// case-insensitively. Never a symlink — the caller passes `DirEntry::file_type()`,
/// which does not follow the link, so a symlink (to a file, a directory, a pipe or
/// a linked tree) is judged as itself: inert, never opened, never stat'd.
/// Everything else in ModelConf (`README.md`, the single-slot `.yaml.bak` incident
/// copies, `x.yaml.bak`, `Thumbs.db`, `notes.txt`, a name with no extension, even
/// a directory named `x.yaml`) is silently inert: non-candidates are not configs,
/// so they produce **no catalog_error** — the Fork C InvalidYaml voice stays
/// reserved for actual accepted config files.
fn is_accepted_config(file_type: &std::fs::FileType, path: &Path) -> bool {
    file_type.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
}

/// The existence-trace file-type predicate: the traces accept
/// **regular files** only. `fs::metadata` succeeds
/// for a directory, so without this filter an operator typo
/// (`model: models/Qwen-GGUF` — the folder, filename forgotten) publishes,
/// admission evicts victims, and the engine dies at Spawn on the directory path
/// — the exact doomed-spawn swap the trace exists to refuse — and the same stat
/// books the directory's byte size (often 0 on NTFS) into `gguf_size_gb`.
fn regular_file_metadata(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::metadata(path).ok().filter(|meta| meta.is_file())
}

/// The one base an existence trace joins a relative yaml value onto: the yaml's
/// own directory — the same base `model:` uses (launch resolves `model:`
/// against the same dir; the engine worker owns relative resolution on the
/// launch side, the scan only refuses).
fn trace_under_yaml_dir(dir: &Path, raw: &str) -> std::path::PathBuf {
    let path = Path::new(raw);
    if path.is_relative() {
        dir.join(path)
    } else {
        path.to_path_buf()
    }
}

/// Scans a ModelConf directory: every regular (non-symlink) `*.yaml` or `*.yml`
/// file becomes an entry or an error; every other name is **silently inert** (the
/// 13\[5] Fork B acceptance gate, `is_accepted_config` — `.yml` accepted by
/// Doctrine 10's override); a missing directory is one error, not a
/// panic.
pub fn scan_dir(dir: &Path) -> Catalog {
    let mut catalog = Catalog::empty();
    // Test-only hang seam: parks this scan (and only a scan of the armed directory)
    // until the test releases it, so a rescan can be held in flight to prove the
    // actor never waits on it. Production never arms it and pays one lock-free skip.
    #[cfg(test)]
    {
        let pause = {
            let mut guard = scan_pause().lock().expect("scan pause lock");
            if guard.as_ref().is_some_and(|p| p.dir == dir) {
                guard.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            let _ = pause.started.send(());
            let _ = pause.release.recv();
        }
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            catalog.errors.push(ScanError {
                kind: ScanKind::Other,
                file: dir.display().to_string(),
                reason: format!("cannot read ModelConf dir: {error}"),
            });
            return catalog;
        }
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            // The acceptance gate reads `DirEntry::file_type()` — never
            // `metadata()`, so a symlink is judged as itself and never followed
            // (or opened): links, pipes and linked trees are inert, not hung.
            entry
                .file_type()
                .ok()
                .is_some_and(|file_type| is_accepted_config(&file_type, &entry.path()))
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    let mut seen_containers: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for path in files {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        // Identity is the file stem: the stem is the only
        // name-shaped value that reaches a `File::create` path as `log_name`. Tying
        // identity to the file is what makes a `name:`-key twin unconstructable; it
        // does NOT stop same-stem files from collapsing — a whitespace-suffixed
        // stem (`qwen3.yaml` beside `qwen3 .yaml`, which `from_flat` trims to one
        // name) or, since `.yml` joined the acceptance gate (Doctrine 10's
        // override), an `m.yaml` beside its `m.yml` twin — and those cases are
        // what the `DuplicateName` post-pass below reports.
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if stem.trim().is_empty() {
            catalog.errors.push(ScanError {
                kind: ScanKind::InvalidName,
                file: file_name,
                reason: "file stem is empty; identity is the file stem \u{2014} rename the file"
                    .to_string(),
            });
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                catalog.errors.push(ScanError {
                    kind: ScanKind::Other,
                    file: file_name.clone(),
                    reason: format!("cannot read: {error}"),
                });
                continue;
            }
        };
        let (map, problems) = parse_flat_yaml(&text);
        if !problems.is_empty() {
            // Fork C doctrine (operator ruling): a file with ANY problem line is
            // never published — "better not publish anything blindly". One report
            // names the file and every offending line; the empty-map case (every
            // line bad) folds into the same shape, so the file is still named.
            catalog.errors.push(ScanError {
                kind: ScanKind::InvalidYaml,
                file: file_name.clone(),
                reason: format!(
                    "yaml not loaded \u{2014} problem lines: {}",
                    problems.join("; ")
                ),
            });
            continue;
        }
        if map.is_empty() {
            // The same Fork C voice as the gate above, and the mirror of
            // `commit_verified`'s `EmptyFile` refusal: the
            // write-back will not produce a zero-key model file, so the scan must
            // not invent one either. Without this, `blank.yaml` publishes as a
            // spawnable-looking `engine: llama` row named by its file stem with no
            // `model:` at all — the existence check cannot fire because the key is
            // absent, not wrong — and reports nothing. Silently skipping would be
            // the blind-publish this doctrine already rejected; it is reported.
            catalog.errors.push(ScanError {
                kind: ScanKind::InvalidYaml,
                file: file_name.clone(),
                reason: format!(
                    "yaml not loaded \u{2014} no `key: value` lines: an empty or \
                     comment-only file {file_name} is not a model config"
                ),
            });
            continue;
        }
        if let Some(claimed) = map.get("name") {
            if claimed != &stem {
                catalog.errors.push(ScanError {
                    kind: ScanKind::InvalidName,
                    file: file_name.clone(),
                    reason: format!(
                        "name: \"{claimed}\" does not match file {file_name} \u{2014} identity \
                         is the file stem; rename the file or delete the key"
                    ),
                });
                continue;
            }
        }
        match ModelEntry::from_flat(&map, &stem) {
            Ok(mut entry) => {
                // The legacy-removal ruling: the legacy `ninfer_autostart`
                // launch path is gone. A ninfer row still declaring it is refused
                // outright with the adapter's migration sentence (the strata
                // `strata_config` precedent) and never published. The row must
                // not ride a launch the harness no longer has.
                if entry.engine == crate::ownership::NINFER_ENGINE {
                    if let Some(script) = map
                        .get("ninfer_autostart")
                        .map(|v| v.trim())
                        .filter(|v| !v.is_empty())
                    {
                        catalog.errors.push(ScanError {
                            kind: ScanKind::PolicyRefused,
                            file: file_name.clone(),
                            reason: crate::engine::ninfer::legacy_removed_sentence(script),
                        });
                        continue;
                    }
                }
                // The row-binary rule, scan half: a llama row may override the
                // global
                // binary per row, but a value that cannot launch — blank or
                // relative — is refused here with the key named, before
                // admission can evict a resident for a doomed spawn. The
                // existence check stays in the adapter's spec (same posture
                // as `strata_exe`): the scan only pins the shape.
                if entry.engine == "llama" {
                    if let Some(value) = map.get("llama_server_exe") {
                        let trimmed = value.trim();
                        if trimmed.is_empty() || !Path::new(trimmed).is_absolute() {
                            catalog.errors.push(ScanError {
                                kind: ScanKind::PolicyRefused,
                                file: file_name.clone(),
                                reason: "`llama_server_exe` must name an absolute path to \
                                         llama-server.exe; this row is not published until \
                                         the key holds an absolute path or is removed \
                                         (then the global `llama_server_exe` applies)"
                                    .to_string(),
                            });
                            continue;
                        }
                    }
                }
                // One container name may back exactly one model: unload reaps by
                // name, so a duplicate would let either model's release destroy
                // the other's live engine (review, 2026-10-26).
                if entry.engine == crate::ownership::NINFER_ENGINE {
                    if let Some(name) = map.get("ninfer_container").map(|v| v.trim()) {
                        if !name.is_empty() {
                            if let Some(first) =
                                seen_containers.insert(name.to_string(), file_name.clone())
                            {
                                catalog.errors.push(ScanError {
                                    kind: ScanKind::Other,
                                    file: file_name.clone(),
                                    reason: format!(
                                        "duplicate ninfer_container `{name}` (also in {first})"
                                    ),
                                });
                                continue;
                            }
                        }
                    }
                }
                // Existence traces: the stat the size needs is
                // the same stat that proves the file is there — take it once and
                // refuse a llama entry launched against nothing, before admission
                // can evict a resident for the doomed spawn. `scheme://` sentinels
                // and every other engine are exempt (their checks live elsewhere).
                // A trace passes only on a regular file: a directory
                // neither passes the trace nor books its byte size as the model's.
                let local = entry
                    .model_ref
                    .as_deref()
                    .filter(|r| !r.contains("://"))
                    .map(|raw| trace_under_yaml_dir(dir, raw));
                let stat = local.as_ref().and_then(|p| regular_file_metadata(p));
                // The trace's stat IS the fingerprint reading — carry it.
                // The fit-cache guard compares against this scan-carried
                // value on the admit turn instead of re-statting the GGUF on
                // the actor. Keyed by `(yaml, trimmed model:)` from the shared
                // rule; rows refused below this line keep only an inert
                // tombstone (never looked up without a published row), and a
                // later rescan refreshes every row it reads.
                if let (Some(raw), Some(meta)) = (entry.fit_model_key(), stat.as_ref()) {
                    if let Some(fingerprint) = crate::model::FileFingerprint::from_metadata(meta) {
                        crate::model::note_scan_fingerprint(&path, raw, fingerprint);
                    }
                }
                if entry.engine == "llama" && stat.is_none() {
                    if let Some(missing) = &local {
                        catalog.errors.push(ScanError {
                            kind: ScanKind::ModelPathMissing,
                            file: file_name.clone(),
                            reason: format!("model file not found: {}", missing.display()),
                        });
                        continue;
                    }
                }
                // A declared `mmproj:` is the second
                // weights file (policy.rs names it so) — llama-server dies
                // without it, after victims are gone. Existence-trace it like
                // `model:`: same yaml-dir join, same regular-file predicate,
                // same `ModelPathMissing`. The raw map is not resolved or
                // rewritten — relative values resolve at launch in the engine
                // worker; the scan only refuses here.
                if entry.engine == "llama" {
                    let mmproj_missing = map
                        .get("mmproj")
                        .map(|v| v.trim())
                        .filter(|v| !v.is_empty() && !v.contains("://"))
                        .map(|raw| trace_under_yaml_dir(dir, raw))
                        .filter(|path| regular_file_metadata(path).is_none());
                    if let Some(missing) = mmproj_missing {
                        catalog.errors.push(ScanError {
                            kind: ScanKind::ModelPathMissing,
                            file: file_name.clone(),
                            reason: format!("mmproj file not found: {}", missing.display()),
                        });
                        continue;
                    }
                }
                // A basalt `.basalt` pack is not a GGUF: its bytes must never
                // feed the size×1.15+2 estimate. The booking source of a basalt
                // row is the declared `vram_gb` with `cost: fixed`, or the room
                // at admit with `cost: greedy`,
                // so a pack stat stays display-blind here.
                if entry.engine != "basalt" {
                    entry.gguf_size_gb = stat.map(|meta| meta.len() as f64 / (1024.0_f64).powi(3));
                }
                // A strata row is either template mode — the row yaml owns the
                // profile's fields and the harness renders the JSON, so the
                // scan renders the same bytes and refuses a referenced path
                // the disk lacks before any load gesture (row 19) — or still
                // declares `strata_config`, the mode SUPERSEDED by
                // DECISIONS.md row 19: such a row is refused outright with the
                // migration sentence, and the live manual profile it points at
                // stays the operator's own file, read by nothing on the serve
                // path. Relative rendered paths are exempt (the engine
                // resolves them against the profile dir — the custody backlog);
                // the `log` key is exempt entirely (a log target's existence
                // is not a load precondition).
                if entry.engine == "strata" {
                    let template = crate::engine::strata::template_mode(&entry);
                    if let Some(profile) = map
                        .get("strata_config")
                        .map(|v| v.trim())
                        .filter(|v| !v.is_empty())
                    {
                        // A row that already carries the imported surface only
                        // needs its `strata_config` line deleted — name that
                        // door, not the import bridge it does not need; the
                        // adapter refuses the same sentence.
                        let reason = if template {
                            crate::engine::strata::mutual_exclusion_sentence(profile)
                        } else {
                            crate::engine::strata::superseded_sentence(profile)
                        };
                        catalog.errors.push(ScanError {
                            kind: ScanKind::PolicyRefused,
                            file: file_name.clone(),
                            reason,
                        });
                        continue;
                    }
                    if template {
                        // The default cwd resolves exactly as the adapter's
                        // does (the `engines:` default — scan runs before the
                        // config-wired adapter exists, so the constant is the
                        // single shared home); the value only joins the
                        // rendered bytes, and a cwd only the config names is
                        // re-checked with its true value at spec time.
                        let cwd = map
                            .get("strata_cwd")
                            .map(|v| v.trim())
                            .filter(|v| !v.is_empty())
                            .unwrap_or(crate::config::STRATA_CWD_DEFAULT);
                        let rendered =
                            crate::engine::strata::render_profile(&entry, Path::new(cwd));
                        match rendered {
                            Ok(text) => {
                                if let Err(problem) =
                                    crate::engine::strata::validate_profile_text(&text)
                                {
                                    let kind = match &problem {
                                        crate::engine::strata::ProfileProblem::MissingPath {
                                            ..
                                        } => ScanKind::ModelPathMissing,
                                        crate::engine::strata::ProfileProblem::PolicyField {
                                            ..
                                        } => ScanKind::PolicyRefused,
                                    };
                                    catalog.errors.push(ScanError {
                                        kind,
                                        file: file_name.clone(),
                                        reason: problem
                                            .describe(&format!("rendered:{}", entry.name)),
                                    });
                                    continue;
                                }
                            }
                            Err(error) => {
                                // A shape refusal (a key that cannot carry its
                                // value) is a typed reject, not a policy claim:
                                // the refusal already names the yaml key.
                                catalog.errors.push(ScanError {
                                    kind: ScanKind::Other,
                                    file: file_name.clone(),
                                    reason: error.to_string(),
                                });
                                continue;
                            }
                        }
                    }
                }
                // A basalt row passes through the adapter's single door
                // (`basalt::validate_row`, stage 1), so the scan refuses at
                // rescan exactly what the adapter would refuse at spec time —
                // a doomed launch can never evict a resident first. The honest
                // mapping: an absent or blank required key (`model`,
                // `serve_model`, `basalt_python`, `basalt_cwd`, `basalt_exe`,
                // and `vram_gb` on a `cost: fixed` row - a greedy row may
                // legally omit the nominal) names
                // its field as a typed reject; a `.basalt`
                // weights file the disk lacks travels the same
                // `ModelPathMissing` door as llama's `model:`; a server-owned
                // word in `basalt_engine_args` meets the policy surface — the
                // row's `extra_words` contract already refuses it in
                // `from_flat`, and this leg keeps the direct door consistent
                // (the refusal voice). The `vram_gb`/`cost` booking requirements
                // are `validate_row`'s, mapped through unchanged.
                if entry.engine == "basalt" {
                    if let Err(problem) = crate::engine::basalt::validate_row(&entry) {
                        let kind = match &problem {
                            crate::engine::basalt::BasaltRowError::MissingPath { .. } => {
                                ScanKind::ModelPathMissing
                            }
                            crate::engine::basalt::BasaltRowError::Bad { key, .. } => {
                                if key == "basalt_engine_args" {
                                    ScanKind::PolicyRefused
                                } else {
                                    ScanKind::Other
                                }
                            }
                            crate::engine::basalt::BasaltRowError::Missing { .. } => {
                                ScanKind::Other
                            }
                        };
                        catalog.errors.push(ScanError {
                            kind,
                            file: file_name.clone(),
                            reason: problem.into_engine_error().to_string(),
                        });
                        continue;
                    }
                }
                // A maya row passes through the adapter's single door: the row
                // names the interpreter, the Maya root and the operator's JSON
                // config, and the config is read here exactly as the adapter
                // reads it, so the scan refuses at rescan what the spawn would
                // refuse after eviction. An absent path is the model-path door;
                // a missing key or a bad value is a typed reject; the config's
                // own keys travel in the sentence.
                if entry.engine == "maya" {
                    match crate::engine::maya::validate_row(&entry) {
                        Ok(row) => {
                            // Capture the effective context once, here,
                            // beside the door that already read and validated
                            // the config. The display home reads this carry and
                            // never the file; the derived figure REPLACES any
                            // hand-written line of the same name (and its
                            // absence removes one), so the page can only ever
                            // show what the staged config launches with. The
                            // carry is scan-derived, never a yaml key, so a
                            // clean row earns no unknown/inert-key report of
                            // its own.
                            match row.max_context {
                                Some(context) => {
                                    entry.raw.insert(
                                        crate::engine::maya::MAYA_MAX_CONTEXT_KEY.to_string(),
                                        context,
                                    );
                                }
                                None => {
                                    entry.raw.remove(crate::engine::maya::MAYA_MAX_CONTEXT_KEY);
                                }
                            }
                        }
                        Err(problem) => {
                            let kind = match &problem {
                                crate::engine::maya::MayaRowError::MissingPath { .. } => {
                                    ScanKind::ModelPathMissing
                                }
                                crate::engine::maya::MayaRowError::Missing { .. }
                                | crate::engine::maya::MayaRowError::Bad { .. } => ScanKind::Other,
                            };
                            catalog.errors.push(ScanError {
                                kind,
                                file: file_name.clone(),
                                reason: problem.into_engine_error().to_string(),
                            });
                            continue;
                        }
                    }
                }
                // A xolotl row passes through the adapter's single door
                // (`xolotl::validate_row`), so the scan
                // refuses at rescan exactly what the adapter would refuse at
                // spec time — a doomed launch can never evict a resident
                // first. A path the row names and the disk lacks (engine
                // exe, launcher, weights, `--engram`/`--rank` entries, a
                // named DSpark sidecar) travels the `ModelPathMissing` door;
                // an absent required key or a bad shape is a typed reject;
                // a rendered claim of a harness-forced field meets the
                // policy surface.
                if entry.engine == "xolotl" {
                    if let Err(problem) = crate::engine::xolotl::validate_row(&entry) {
                        let kind = match &problem {
                            crate::engine::xolotl::XolotlRowError::MissingPath { .. } => {
                                ScanKind::ModelPathMissing
                            }
                            crate::engine::xolotl::XolotlRowError::Claim { .. } => {
                                ScanKind::PolicyRefused
                            }
                            crate::engine::xolotl::XolotlRowError::Missing { .. }
                            | crate::engine::xolotl::XolotlRowError::Bad { .. } => ScanKind::Other,
                        };
                        catalog.errors.push(ScanError {
                            kind,
                            file: file_name.clone(),
                            reason: problem.into_engine_error().to_string(),
                        });
                        continue;
                    }
                }
                // The CPU-resident row shapes:
                // a `cpu_offload`-truthy row books host RAM, so the VRAM
                // gate keys describe a resource it does not hold. `whole` is
                // a contradiction that would refuse the row at every load
                // forever, so the scan refuses it here with the named
                // problem — the doomed-launch-before-eviction posture, same
                // door as the basalt validation above. A numeric launch
                // floor is only dead config: the inert-key warn, the entry
                // publishes (ruling D's posture).
                if entry.cpu_resident() {
                    if entry.vram_gate == crate::model::VramGate::Whole {
                        catalog.errors.push(ScanError {
                            kind: ScanKind::PolicyRefused,
                            file: file_name.clone(),
                            reason: "`cpu_offload` books host RAM, never VRAM \u{2014} the \
                                     `whole` machine gate can never apply to this row; \
                                     remove `launch_only_with_at_least_vram_free: whole` \
                                     (or the `cpu_offload` line)"
                                .to_string(),
                        });
                        continue;
                    }
                    if entry.vram_gate.floor().is_some() {
                        catalog.errors.push(ScanError {
                            kind: ScanKind::InertKey,
                            file: file_name.clone(),
                            reason: "`launch_only_with_at_least_vram_free` gates VRAM \u{2014} a \
                                     `cpu_offload` row holds no VRAM booking; this key does \
                                     nothing here"
                                .to_string(),
                        });
                    }
                }
                // Unknown keys are reported, never fatal (README doctrine), but
                // never silent: an operator's typo'd `contex:` is a config that
                // does nothing, and the page must be able to say so. The one
                // silent class is [`SILENT_KEYS`] — reserved forensic keys
                // ("forensic manual only", operator ruling), which are neither
                // warned nor logged nor surfaced. The same walk also scopes
                // every engine knob (the generalization of the
                // `cpu_offload` scoping rule): a consumed key the row's engine never
                // reads is live config for a *different* row — not a typo, so
                // not `UnknownKey`; a policy hint, so not silent: `InertKey`
                // warns and the entry publishes (ruling D stands).
                for key in map.keys() {
                    if SILENT_KEYS.contains(&key.as_str()) {
                        continue;
                    }
                    // `ninfer_profile_<name>` is the family of custom-profile
                    // override keys (template adoption, 2026-10-31): the suffix is
                    // operator-chosen, so the const cannot enumerate it. The exact
                    // name `ninfer_profile` stays a const member; this prefix is the
                    // one read-side exception, and the adapter still refuses a
                    // selected custom profile whose override line is absent. A
                    // suffix the adapter could never name as a profile is dead
                    // config, so it stays an unknown key rather than passing on
                    // prefix alone (the shape mirrors `valid_profile_name` in
                    // engine/ninfer.rs — including its first-byte-lowercase rule,
                    // which keeps `ninfer_profile_9x` reportable — and that module
                    // remains the launch-time authority).
                    let profile_override_key =
                        key.strip_prefix("ninfer_profile_").is_some_and(|suffix| {
                            !suffix.is_empty() && suffix.len() <= PROFILE_NAME_MAX_BYTES
                        }) && key
                            .strip_prefix("ninfer_profile_")
                            .unwrap_or_default()
                            .bytes()
                            .next()
                            .is_some_and(|b| b.is_ascii_lowercase())
                            && key
                                .strip_prefix("ninfer_profile_")
                                .unwrap_or_default()
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                    if profile_override_key {
                        // On a non-ninfer row the line is dead config like every
                        // foreign knob: warn (InertKey), never silently pass.
                        if entry.engine != crate::ownership::NINFER_ENGINE {
                            catalog.errors.push(ScanError {
                                kind: ScanKind::InertKey,
                                file: file_name.clone(),
                                reason: inert_key_reason(key, &entry.engine),
                            });
                        }
                        continue;
                    }
                    if !crate::engine::policy::knob_live(key, &entry.engine)
                        && !crate::engine::policy::knob_readers(key).is_empty()
                    {
                        catalog.errors.push(ScanError {
                            kind: ScanKind::InertKey,
                            file: file_name.clone(),
                            reason: inert_key_reason(key, &entry.engine),
                        });
                        continue;
                    }
                    if !CONSUMED_KEYS.contains(&key.as_str()) {
                        let reason = match key.as_str() {
                            "host" => HOST_POLICY_HINT.to_string(),
                            "port" => PORT_POLICY_HINT.to_string(),
                            // The slot is honest about its fate: unlike ordinary
                            // unknown keys this one is stripped, never preserved.
                            "fit_gb" => "`fit_gb` is the engine-calculator's projection \
                                         slot, computed at load; a hand-written value is \
                                         ignored and stripped, it never books"
                                .to_string(),
                            other => format!(
                                "unknown key `{other}` is not read by the harness (preserved \
                                 for reference){}",
                                did_you_mean(other)
                            ),
                        };
                        catalog.errors.push(ScanError {
                            kind: ScanKind::UnknownKey,
                            file: file_name.clone(),
                            reason,
                        });
                    }
                }
                entry.file = Some(path.clone());
                catalog.models.push(entry);
            }
            Err(error) => {
                // The extra-words refusal is the policy surface,
                // not a typed-field reject: triage must be able to tell a
                // harness-owned-surface claim from a bad number.
                let kind = match &error {
                    crate::model::ModelError::ExtraWordsRefused { .. } => ScanKind::PolicyRefused,
                    _ => ScanKind::Other,
                };
                catalog.errors.push(ScanError {
                    kind,
                    file: file_name,
                    reason: error.to_string(),
                });
            }
        }
    }
    // One identity may belong to exactly one row. `from_flat` trims
    // the stem it is handed, so two files can collapse to one name — `qwen3.yaml`
    // beside `qwen3 .yaml` — and every name-keyed path would then answer for the model
    // with whichever row the scan order put first: `/har/load` would load one engine
    // and the measured fold would book this launch's footprint into the other's bytes.
    // Report every claimant and publish every claimant — the scan's posture is "name
    // the problem, keep the file visible", never a boot-style
    // rejection — and the API's `409` stays the place an ambiguous query is refused.
    let mut claimants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for model in &catalog.models {
        if let Some(file) = model
            .file
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
        {
            claimants
                .entry(model.name.to_ascii_lowercase())
                .or_default()
                .push(file);
        }
    }
    for (identity, files) in claimants {
        if files.len() < 2 {
            continue;
        }
        for file in &files {
            catalog.errors.push(ScanError {
                kind: ScanKind::DuplicateName,
                file: file.clone(),
                reason: format!(
                    "name `{identity}` is claimed by {} files ({}) \u{2014} identity is the \
                     file stem: rename one, or every load of it stays refused as ambiguous",
                    files.len(),
                    files.join(", ")
                ),
            });
        }
    }
    catalog.models.sort_by(|a, b| a.name.cmp(&b.name));
    catalog
}

/// The did-you-mean tail of an unknown-key report: only when exactly one consumed
/// key sits one edit away, so the advice is a fact, never a coin flip. The
/// candidate list is [`CONSUMED_KEYS`] alone, so a [`SILENT_KEYS`] member can
/// never surface as a suggestion — reserved-forensic silence means no breadcrumb
/// either.
fn did_you_mean(key: &str) -> String {
    let mut candidate = String::new();
    let mut count = 0usize;
    for consumed in CONSUMED_KEYS {
        if within_one_edit(key, consumed) {
            count += 1;
            candidate = (*consumed).to_string();
        }
    }
    if count == 1 {
        format!(" (did you mean '{candidate}'?)")
    } else {
        String::new()
    }
}

/// True when `a` and `b` differ by at most one insertion, deletion, or
/// substitution. Keys are ASCII by construction (`parse_flat_yaml`'s charset), so
/// a byte walk is exact here.
fn within_one_edit(a: &str, b: &str) -> bool {
    if a == b {
        return false; // not a typo, the key itself
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let (mut i, mut j, mut mismatched) = (0usize, 0usize, false);
    while i < long.len() && j < short.len() {
        if long[i] == short[j] {
            i += 1;
            j += 1;
            continue;
        }
        if mismatched {
            return false;
        }
        mismatched = true;
        if long.len() == short.len() {
            i += 1;
            j += 1;
        } else {
            i += 1;
        }
    }
    true
}

/// Test-only: parks `scan_dir` for exactly one armed directory (signal on entry,
/// block until release) so a scheduler unit test can hold a rescan in flight and
/// prove the actor keeps answering. Production never arms it: one uncontended
/// mutex peek is the entire cost.
#[cfg(test)]
pub(crate) struct ScanPause {
    dir: PathBuf,
    started: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
fn scan_pause() -> &'static std::sync::Mutex<Option<ScanPause>> {
    static PAUSE: std::sync::OnceLock<std::sync::Mutex<Option<ScanPause>>> =
        std::sync::OnceLock::new();
    PAUSE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Arms the pause for `dir`; returns (started, release) ends. The next `scan_dir`
/// call for exactly that directory signals `started` and parks until the test
/// drops or sends on the release end; the seam then disarms itself.
#[cfg(test)]
pub(crate) fn arm_scan_pause(
    dir: &Path,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *scan_pause().lock().expect("scan pause lock") = Some(ScanPause {
        dir: dir.to_path_buf(),
        started: started_tx,
        release: release_rx,
    });
    (started_rx, release_tx)
}

#[cfg(test)]
mod tests;
