//! The Project Maya engine adapter.
//!
//! Maya 1.1.x is a Python HTTP server (`serve/server.py`) in front of a native
//! engine. The harness launches that server directly, never the `maya.py`
//! launcher: on Windows the launcher appends `--open` (it opens a browser when
//! the model is ready) and it may repair or calibrate a config before starting,
//! and both are decisions the harness must own. `serve.server` is the
//! documented service/Docker door and takes the same config.
//!
//! The row names the interpreter, the Maya root and the operator's JSON config.
//! That config is an L1 artifact: the adapter parses it, validates every path it
//! names, strips `host`, `port`, `api_key`, `idle_unload_s` and `cors_origins`,
//! forces `model_management` off, and stages the scrubbed bytes
//! content-addressed under `.simpleharn/maya-profiles`; the spawn
//! rewrites `--config` to exactly that file. The bind, the port and the key are
//! harness-forced argv (DOCTRINE 2); idle unload is forced off and model
//! management is forced off — the server enables it by default, and with it a
//! local request could unload the booked resident through `/models/unload` or
//! an Ollama-compat `keep_alive: 0` chat — so a resident
//! the harness booked cannot quietly unload itself.
//!
//! The row may also spell the vision block itself: the typed `maya_vision`
//! overlay is validated through the same path door
//! and merged into the staged profile as the `vision` object, and a row whose
//! config already carries a `vision` object is refused with a named conflict —
//! never a silent precedence. Every other key keeps flowing from the generated
//! JSON as L1, so a new upstream key still needs no adapter change.
//!
//! The config's `--max-context` figure is read exactly once, beside this
//! validation door: the scan captures the digits-only token and carries it on
//! the entry (the scan-derived `maya_max_context` raw key), so `/har/models`
//! shows the context the staged config really serves while the display door
//! never re-reads the config file.
//!
//! Readiness is the served id on the unauthenticated `/health` body. The server
//! binds its port only after the engine load returns, so a refused connection is
//! `NotYet` by construction, with `load_timeout` the harness deadline.

use super::{EngineAdapter, EngineError, LaunchSpec, ReadinessSpec, RenderedProfile};
use crate::model::ModelEntry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// IPv4 loopback address the harness forces as the Maya server bind (`--host`),
/// the same literal every process-engine row forces: the server is never exposed
/// on the segment, so the row never obeys a config `host`.
pub(crate) const MAYA_ENGINE_HOST: &str = "127.0.0.1";

/// The shortest `load_timeout` a Maya row may declare, in seconds. A big pack
/// loads for minutes and the global 120 s default would kill a healthy load;
/// an absent key is a scan refusal, not a silent 120 s deadline.
pub(crate) const MAYA_MIN_LOAD_TIMEOUT_S: u64 = 300;

/// The typed refusal [`validate_row`] answers with, so the scan can map the
/// class without parsing prose: [`MayaRowError::Missing`] is the missing-key
/// door, [`MayaRowError::MissingPath`] is the model-path door, and
/// [`MayaRowError::Bad`] is the named-key refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MayaRowError {
    /// The row carries no value for a key the launch requires.
    Missing {
        /// Harness name of the model.
        model: String,
        /// The missing key.
        field: &'static str,
    },
    /// A path the row or its config names is not on disk.
    MissingPath {
        /// Harness name of the model.
        model: String,
        /// The key naming the absent path.
        key: String,
        /// The path that is absent.
        path: PathBuf,
    },
    /// The row or its config carries a value the launch refuses.
    Bad {
        /// Harness name of the model.
        model: String,
        /// The offending key (row key or config key).
        key: String,
        /// Why the value is refused (the tail of the engine voice).
        reason: String,
    },
}

impl MayaRowError {
    /// The same refusal in the adapter voice [`EngineAdapter::spec`] returns:
    /// a missing key meets [`EngineError::MissingField`], an absent path meets
    /// [`EngineError::Rejected`] naming the key, and a bad value names its key.
    #[must_use]
    pub(crate) fn into_engine_error(self) -> EngineError {
        match self {
            Self::Missing { model, field } => EngineError::MissingField {
                model,
                field: field.to_string(),
            },
            Self::MissingPath { model, key, path } => EngineError::Rejected {
                engine: "maya".to_string(),
                model,
                reason: format!("`{key}` is not an existing path: {}", path.display()),
            },
            Self::Bad { model, key, reason } => EngineError::Rejected {
                engine: "maya".to_string(),
                model,
                reason: format!("`{key}`: {reason}"),
            },
        }
    }
}

/// The scan-derived raw key carrying one maya row's effective served context:
/// the digits-only `--max-context` figure of the L1 config's `args`, captured
/// once beside [`validate_row`] and read by [`crate::engine::effective_context`].
/// It is not a yaml key: a hand-written line of this name is unknown to the
/// scan and replaced (or removed) by the derived figure, so the page can only
/// ever show the context the staged config actually launches with.
pub(crate) const MAYA_MAX_CONTEXT_KEY: &str = "maya_max_context";

/// The config's effective served context: the digits-only token after
/// `--max-context` in the config's own `args`, spelled with a space
/// (`--max-context 131072`) or with `=` (`--max-context=131072`). The last
/// occurrence wins, the way a later argv word overrides an earlier one; a
/// value slot that is not a non-empty run of ASCII digits is no figure — the
/// same honest reading `strata::effective_context` gives its argv.
fn max_context_of(args: &[&str]) -> Option<String> {
    fn digits(value: &str) -> bool {
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
    }
    let mut found: Option<String> = None;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if *word == "--max-context" {
            found = words
                .next()
                .filter(|value| digits(value))
                .map(|value| (*value).to_string());
        } else if let Some(value) = word.strip_prefix("--max-context=") {
            found = digits(value).then(|| value.to_string());
        }
    }
    found
}

/// One fully validated Maya row: every path exists, the config's `model_name`
/// is the row's `serve_model`, and the scrubbed config bytes are ready to stage.
/// Produced only by [`validate_row`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MayaRow {
    /// The interpreter the server runs under (`maya_python`).
    pub python: PathBuf,
    /// The launcher's working directory (`maya_cwd`), where `-m serve.server`
    /// resolves.
    pub cwd: PathBuf,
    /// The served id (`serve_model`), verified at readiness.
    pub alias: String,
    /// The scrubbed config bytes, staged content-addressed.
    pub text: String,
    /// The digits-only `--max-context` figure of the config's `args`, if it
    /// spells one — the effective served context the scan carries for display.
    pub max_context: Option<String>,
}

fn knob<'a>(model: &'a ModelEntry, key: &str) -> Option<&'a str> {
    model
        .raw
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
}

fn bad(model: &ModelEntry, key: &str, reason: impl Into<String>) -> MayaRowError {
    MayaRowError::Bad {
        model: model.name.clone(),
        key: key.to_string(),
        reason: reason.into(),
    }
}

fn missing(model: &ModelEntry, field: &'static str) -> MayaRowError {
    MayaRowError::Missing {
        model: model.name.clone(),
        field,
    }
}

/// One absolute path of the required shape: absolute (an argv-relative path
/// would resolve against the child's cwd, not the operator's expectation),
/// existing, and the right kind.
fn checked_path(
    model: &ModelEntry,
    key: &str,
    raw: &str,
    want_dir: bool,
) -> Result<PathBuf, MayaRowError> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(bad(model, key, format!("must be an absolute path: {raw}")));
    }
    if want_dir {
        if !path.exists() {
            return Err(MayaRowError::MissingPath {
                model: model.name.clone(),
                key: key.to_string(),
                path: path.to_path_buf(),
            });
        }
        if !path.is_dir() {
            return Err(bad(
                model,
                key,
                format!("is not an existing directory: {raw}"),
            ));
        }
    } else if !path.is_file() {
        if !path.exists() {
            return Err(MayaRowError::MissingPath {
                model: model.name.clone(),
                key: key.to_string(),
                path: path.to_path_buf(),
            });
        }
        return Err(bad(model, key, format!("is not a file: {raw}")));
    }
    Ok(path.to_path_buf())
}

/// One JSON string value from the config, required and non-blank.
fn config_str(
    model: &ModelEntry,
    config: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, MayaRowError> {
    config
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| bad(model, key, "must be a non-empty string"))
}

/// One absolute path named by the config value `key`, checked on disk.
fn config_path(
    model: &ModelEntry,
    config: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    want_dir: bool,
) -> Result<PathBuf, MayaRowError> {
    let raw = config_str(model, config, key)?;
    checked_path(model, key, &raw, want_dir)
}

/// The config's `gpu`: a whole number, or a list of whole numbers.
fn check_gpu(
    model: &ModelEntry,
    config: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), MayaRowError> {
    let Some(value) = config.get("gpu") else {
        return Ok(());
    };
    let whole = |value: &serde_json::Value| {
        value
            .as_u64()
            .filter(|n| *n <= u64::from(u32::MAX))
            .map(|n| n as u32)
    };
    match value {
        serde_json::Value::Array(cards) => {
            if cards.is_empty() {
                return Err(bad(model, "gpu", "the list is empty"));
            }
            if cards.iter().any(|card| whole(card).is_none()) {
                return Err(bad(
                    model,
                    "gpu",
                    "every entry must be a non-negative whole GPU index",
                ));
            }
        }
        other => {
            if whole(other).is_none() {
                return Err(bad(
                    model,
                    "gpu",
                    "must be a non-negative whole GPU index or a list of them",
                ));
            }
        }
    }
    Ok(())
}

/// The names the harness owns in the child environment, whatever an L1 config
/// spells: the process search path, the interpreter's own setup, the Windows
/// process basics, and the GPU selection the row's `gpu` key and the forced
/// launch decide. Maya writes engine settings to the config's `env` under the
/// names the engine reads (`MAYA_*`, `HY4_*`, `DS4_*`, `IK2_*`, and the older
/// `STRATA_*` fallbacks), so only this owned set is refused; the comparison is
/// ASCII case-insensitive because the Windows environment is. The loader-control
/// namespaces (`LD_*`, `DYLD_*`) are refused by prefix as well, so a loader name
/// missing from this list cannot slip through.
pub(crate) const HARNESS_OWNED_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "SYSTEMROOT",
    "WINDIR",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USERPROFILE",
    "HOME",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONSTARTUP",
    "PYTHONEXECUTABLE",
    "PYTHONUTF8",
    "PYTHONIOENCODING",
    "PYTHONNOUSERSITE",
    "PYTHONUSERBASE",
    "VIRTUAL_ENV",
    "CUDA_VISIBLE_DEVICES",
    "CUDA_DEVICE_ORDER",
    "HIP_VISIBLE_DEVICES",
    "ROCR_VISIBLE_DEVICES",
    "GPU_DEVICE_ORDINAL",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
];

/// Is `name` a harness-owned child-environment name? See [`HARNESS_OWNED_ENV`].
/// The `LD_`/`DYLD_` loader-control namespaces are refused whole: the loader
/// reads more names than this list spells, and every one of them can change what
/// the engine child loads.
fn harness_owned_env(name: &str) -> bool {
    HARNESS_OWNED_ENV
        .iter()
        .any(|owned| name.eq_ignore_ascii_case(owned))
        || name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("LD_"))
        || name
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("DYLD_"))
}

/// Is `name` a legal environment variable name? An empty name, a name with `=`,
/// or a name with NUL is not: Python raises `ValueError` when the server copies
/// it into the engine child, after the harness has admitted the load.
fn env_name_is_legal(name: &str) -> bool {
    !name.is_empty() && !name.contains('=') && !name.contains('\0')
}

/// The config's `env`: the engine's settings, as the installer writes them and
/// Maya documents them (`MAYA_*`, `HY4_*`, `DS4_*`, `IK2_*`, and the older
/// `STRATA_*` fallbacks). A name the harness owns — the process search path,
/// the interpreter setup, or the GPU selection — is refused by name, because
/// the harness decides those; any other legal name is an engine setting and
/// passes, so a new upstream setting still needs no adapter change. Names must
/// be non-empty and free of `=` and NUL; values are scalars free of NUL.
fn check_env(
    model: &ModelEntry,
    config: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), MayaRowError> {
    let Some(value) = config.get("env") else {
        return Ok(());
    };
    let Some(entries) = value.as_object() else {
        return Err(bad(
            model,
            "env",
            "must be an object of NAME=value settings",
        ));
    };
    for (name, value) in entries {
        if !env_name_is_legal(name) {
            return Err(bad(
                model,
                "env",
                format!("`{name}` is not a legal environment name"),
            ));
        }
        if let Some(text) = value.as_str() {
            if text.contains('\0') {
                return Err(bad(
                    model,
                    &format!("env.{name}"),
                    "must not contain a NUL character",
                ));
            }
        }
        if harness_owned_env(name) {
            return Err(bad(
                model,
                &format!("env.{name}"),
                "is harness-owned: the launch forces the process path, the interpreter setup \
                 and the GPU selection",
            ));
        }
        if !(value.is_string() || value.is_number() || value.is_boolean()) {
            return Err(bad(
                model,
                &format!("env.{name}"),
                "must be a string, number or boolean",
            ));
        }
    }
    Ok(())
}

/// The closed `maya_vision` key set, in refusal voice.
const VISION_KEYS: &str = "exe, mmproj, model, gpu";

/// The words that spell a `maya_vision` `gpu` figure, in refusal voice. The
/// set is the harness truth table written out both ways: a value outside it
/// is a named refusal, never a silent `false`.
const GPU_WORDS: &str = "true, false, 1, 0, yes, no, on, off";

/// The strict `gpu` reading of the overlay: one of the eight words (ASCII
/// case-insensitive, trimmed), or a named `maya_vision.gpu` refusal. Unlike
/// the loose [`crate::model::truthy`] table, an unknown or empty value here
/// is refused: a typo must not silently move the encoder off-card.
fn gpu_flag(model: &ModelEntry, value: &str) -> Result<bool, MayaRowError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(bad(
            model,
            "maya_vision.gpu",
            format!("must be one of {GPU_WORDS}: {value}"),
        )),
    }
}

/// The `maya_vision` block: whitespace-separated `k=v` tokens over the closed
/// key set — `exe`, `mmproj` and `model` are absolute file paths (required
/// together, the strata and xolotl rule: the vision server cannot start
/// without them), checked through [`checked_path`] like every other launch
/// path; `gpu` is a strict figure from [`GPU_WORDS`] and DEFAULTS ON here (the
/// operator's engine-side block carries `gpu: true`, so the overlay renders
/// the same figure a hand-kept config spells); a key spelled twice is refused,
/// never a silent last-wins. A declared-but-blank line never reaches this
/// parser: [`knob`] trims it to absent. The block merges into the staged
/// profile as its `vision` object; the conflict with a config-side `vision`
/// object is refused by [`validate_row`] before this parser runs, so the conflict always wins.
fn vision_overlay(
    model: &ModelEntry,
    line: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, MayaRowError> {
    let mut taken = serde_json::Map::new();
    let mut gpu = true;
    let mut gpu_spelled = false;
    for token in line.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            return Err(bad(
                model,
                "maya_vision",
                format!("tokens must be k=v over {VISION_KEYS}: {token}"),
            ));
        };
        if !matches!(key, "exe" | "mmproj" | "model" | "gpu") {
            return Err(bad(
                model,
                "maya_vision",
                format!("key `{key}` is not one of {VISION_KEYS}"),
            ));
        }
        let already = if key == "gpu" {
            gpu_spelled
        } else {
            taken.contains_key(key)
        };
        if already {
            return Err(bad(
                model,
                "maya_vision",
                format!("sets `{key}` twice; the block spells each key once"),
            ));
        }
        if key == "gpu" {
            gpu_spelled = true;
            gpu = gpu_flag(model, value)?;
            continue;
        }
        checked_path(model, &format!("maya_vision.{key}"), value, false)?;
        taken.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    for required in ["exe", "mmproj", "model"] {
        if !taken.contains_key(required) {
            return Err(bad(
                model,
                "maya_vision",
                format!("requires `{required}` when a vision block is declared"),
            ));
        }
    }
    taken.insert("gpu".to_string(), serde_json::Value::Bool(gpu));
    Ok(taken)
}

/// The engine-side vision block, when present: every program and weights file
/// it names must exist before the load gesture. The harness does not manage the
/// block; it refuses a doomed launch instead of letting the server die after
/// the manager has evicted a resident. Only [`validate_row`]'s no-overlay leg
/// reads it: a row that spells `maya_vision` never reaches this door.
fn check_vision(
    model: &ModelEntry,
    config: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), MayaRowError> {
    let Some(value) = config.get("vision") else {
        return Ok(());
    };
    let Some(vision) = value.as_object() else {
        return Err(bad(model, "vision", "must be an object"));
    };
    for key in ["exe", "mmproj", "model"] {
        let raw = config_str(model, vision, key)?;
        checked_path(model, &format!("vision.{key}"), &raw, false)?;
    }
    Ok(())
}

/// Validates one row into a [`MayaRow`]: the single door the adapter and the
/// scan both pass through, so a launch can never be built from a row the other
/// consumer would refuse.
///
/// # Errors
///
/// [`MayaRowError::Missing`] for an absent required row key,
/// [`MayaRowError::MissingPath`] for a config or weights path that is not on
/// disk, [`MayaRowError::Bad`] for a present value the launch refuses.
pub(crate) fn validate_row(model: &ModelEntry) -> Result<MayaRow, MayaRowError> {
    let alias = model
        .serve_model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| missing(model, "serve_model"))?
        .to_string();
    let python = {
        let raw = knob(model, "maya_python").ok_or_else(|| missing(model, "maya_python"))?;
        checked_path(model, "maya_python", raw, false)?
    };
    let cwd = {
        let raw = knob(model, "maya_cwd").ok_or_else(|| missing(model, "maya_cwd"))?;
        checked_path(model, "maya_cwd", raw, true)?
    };
    let config_file = {
        let raw = knob(model, "maya_config").ok_or_else(|| missing(model, "maya_config"))?;
        checked_path(model, "maya_config", raw, false)?
    };
    // A big pack loads for minutes: an absent key would inherit the global
    // 120 s deadline and kill a healthy load. The row states its own number.
    match model.load_timeout_s {
        Some(seconds) if seconds >= MAYA_MIN_LOAD_TIMEOUT_S => {}
        Some(seconds) => {
            return Err(bad(
                model,
                "load_timeout",
                format!(
                    "must be {MAYA_MIN_LOAD_TIMEOUT_S} seconds or more for a Maya load: {seconds}"
                ),
            ));
        }
        None => return Err(missing(model, "load_timeout")),
    }
    match model.cost {
        crate::model::CostClass::Fixed => {
            if model.vram_gb.is_none() {
                return Err(missing(model, "vram_gb"));
            }
        }
        crate::model::CostClass::Greedy => {}
        crate::model::CostClass::Exact => {
            return Err(bad(
                model,
                "cost",
                "maya rows book a fixed footprint (`cost: fixed`) or run greedy \
                 (`cost: greedy`: the expert cache fills VRAM and spills to RAM/SSD; \
                 the launch floor is the gate); the engine reports no computed \
                 requirement to book as `exact`",
            ));
        }
    }
    let bytes = std::fs::read(&config_file).map_err(|_error| MayaRowError::MissingPath {
        model: model.name.clone(),
        key: "maya_config".to_string(),
        path: config_file.clone(),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| MayaRowError::Bad {
        model: model.name.clone(),
        key: "maya_config".to_string(),
        reason: "is not UTF-8 JSON".to_string(),
    })?;
    let text = text.trim_start_matches('\u{feff}');
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| MayaRowError::Bad {
            model: model.name.clone(),
            key: "maya_config".to_string(),
            reason: format!("is not JSON: {error}"),
        })?;
    let mut config = value
        .as_object()
        .cloned()
        .ok_or_else(|| bad(model, "maya_config", "must hold a JSON object"))?;
    let model_name = config_str(model, &config, "model_name")?;
    if model_name != alias {
        return Err(bad(
            model,
            "serve_model",
            format!(
                "`{alias}` is not the config's `model_name` (`{model_name}`): the served id \
                 would not match the row"
            ),
        ));
    }
    config_path(model, &config, "exe", false)?;
    config_path(model, &config, "cwd", true)?;
    let tokenizer = config_path(model, &config, "tokenizer", true)?;
    if !tokenizer.join("vocab.json").is_file() {
        return Err(MayaRowError::MissingPath {
            model: model.name.clone(),
            key: "tokenizer".to_string(),
            path: tokenizer.join("vocab.json"),
        });
    }
    // The server treats a missing `args` as none; a present value must still
    // be a list of string tokens, or the argv the server builds from the
    // config is not what the harness validated.
    if let Some(args) = config.get("args") {
        if args
            .as_array()
            .is_none_or(|args| args.iter().any(|arg| !arg.is_string()))
        {
            return Err(bad(model, "args", "must be a list of string tokens"));
        }
    }
    // The effective context rides the config's own argv, read once here beside
    // the shape check that proves the tokens are strings: the display door
    // reads this figure, never the config file again.
    let max_context = config
        .get("args")
        .and_then(serde_json::Value::as_array)
        .and_then(|args| {
            let words: Vec<&str> = args.iter().filter_map(serde_json::Value::as_str).collect();
            max_context_of(&words)
        });
    if let Some(log) = config
        .get("log")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|log| !log.is_empty())
    {
        let path = Path::new(log);
        if !path.is_absolute() {
            return Err(bad(
                model,
                "log",
                format!("must be an absolute path: {log}"),
            ));
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            if !parent.is_dir() {
                return Err(MayaRowError::MissingPath {
                    model: model.name.clone(),
                    key: "log".to_string(),
                    path: parent.to_path_buf(),
                });
            }
        }
    }
    check_gpu(model, &config)?;
    check_env(model, &config)?;
    // The vision door has two shapes: a row that spells
    // `maya_vision` renders its `vision` object into the staged profile and is
    // refused when the engine-side config already carries one — the conflict
    // is judged FIRST, before any overlay path is touched, so it always wins
    // and no silent precedence exists either way. A row that spells none
    // keeps today's rule: the engine-side block is existence-checked, never
    // managed. A declared-but-blank line is absent by [`knob`]'s trim.
    match knob(model, "maya_vision") {
        Some(line) => {
            if config.contains_key("vision") {
                return Err(bad(
                    model,
                    "maya_vision",
                    "conflicts with the config's own `vision` object: remove one, \
                     the harness refuses a silent precedence",
                ));
            }
            let vision = vision_overlay(model, line)?;
            config.insert("vision".to_string(), serde_json::Value::Object(vision));
        }
        None => check_vision(model, &config)?,
    }
    // The harness owns these decisions: the bind and the port are forced
    // argv, the key is absent (loopback only), idle unload would break the
    // harness's residency, and CORS is harness-owned (loopback only). Remove
    // them from the staged copy so a fallback can never read them from the
    // engine-side file.
    for key in ["host", "port", "api_key", "idle_unload_s", "cors_origins"] {
        config.remove(key);
    }
    // Model management is the server's own default ON: with it on, a local
    // request could unload the booked resident (`/models/unload`, or the
    // Ollama-compat `keep_alive: 0` chat) without a scheduler release — the
    // harness is the single writer of the residency. Force it off in the
    // staged copy, after the scrub, so an L1 value can never turn it back on.
    config.insert(
        "model_management".to_string(),
        serde_json::Value::Bool(false),
    );
    // The engine binary and cwd stay in the staged copy: the server reads them
    // from the config, not from argv.
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(config))
        .expect("invariant: a JSON object serializes");
    Ok(MayaRow {
        python,
        cwd,
        alias,
        text,
        max_context,
    })
}

/// The Python child needs a home directory (Maya writes caches and conversation
/// state below it) and UTF-8 pipes (the server prints model labels). The harness
/// scrubs the parent environment, so both ride the launch spec.
fn python_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("PYTHONUTF8".to_string(), "1".to_string());
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = home.to_string_lossy().into_owned();
        env.insert("USERPROFILE".to_string(), home.clone());
        env.insert("HOME".to_string(), home);
    }
    env
}

/// Maya adapter: argv from the row and the scrubbed config, `/health` reporting
/// the exact served id.
#[derive(Debug, Clone, Default)]
pub struct MayaAdapter;

impl EngineAdapter for MayaAdapter {
    fn engine_id(&self) -> &str {
        "maya"
    }

    fn spec(&self, model: &ModelEntry, port: u16) -> Result<LaunchSpec, EngineError> {
        let row =
            super::policy::engine_policy("maya").expect("the compiled-in table ships a maya row");
        let v = validate_row(model).map_err(MayaRowError::into_engine_error)?;
        let staged_name = crate::process::staged_profile_name(&model.name, &v.text);
        let mut argv = vec![v.python.to_string_lossy().into_owned()];
        argv.extend(["-m".to_string(), "serve.server".to_string()]);
        argv.extend(super::policy::render_forced(row, port, Some(&staged_name)));
        Ok(LaunchSpec {
            argv,
            cwd: v.cwd,
            env: python_env(),
            log_name: model.name.clone(),
            readiness: ReadinessSpec::HttpString {
                path: "/health".to_string(),
                expect_status: 200,
                pointer: "/model".to_string(),
                expect: v.alias.clone(),
            },
            container: None,
            script: None,
            rendered_profile: Some(RenderedProfile {
                engine: "maya",
                text: v.text,
            }),
            custody: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CategoryId;
    use crate::model::{CostClass, ModelEntry};

    fn raw(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// One temp install: interpreter, engine, tokenizer and a config shaped
    /// like the operator's installs.
    fn fixture(overrides: &[(&str, &str)]) -> (std::path::PathBuf, std::path::PathBuf, ModelEntry) {
        let dir = crate::testutil::temp_dir("maya-row");
        let tokenizer = dir.join("tokenizer");
        std::fs::create_dir_all(&tokenizer).expect("tokenizer dir");
        std::fs::write(tokenizer.join("vocab.json"), b"{}").expect("vocab");
        std::fs::write(dir.join("python.exe"), b"p").expect("python");
        std::fs::write(dir.join("maya.exe"), b"e").expect("engine");
        let config = dir.join("maya.json");
        std::fs::write(
            &config,
            serde_json::json!({
                "exe": dir.join("maya.exe"),
                "args": ["--glm-pack", "pack", "--max-context", "131072"],
                "cwd": dir,
                "tokenizer": tokenizer,
                "model_name": "maya-test",
                "gpu": [0],
                "log": dir.join("maya.log"),
            })
            .to_string(),
        )
        .expect("config");
        let python = dir.join("python.exe");
        let mut pairs: Vec<(&str, &str)> = vec![
            ("maya_python", python.to_str().expect("utf-8")),
            ("maya_cwd", dir.to_str().expect("utf-8")),
            ("maya_config", config.to_str().expect("utf-8")),
            ("load_timeout", "900"),
        ];
        pairs.extend(overrides);
        let mut model = ModelEntry {
            name: "maya-test".to_string(),
            engine: "maya".to_string(),
            category: CategoryId::Work,
            vram_gb: Some(10.0),
            ram_gb: None,
            gguf_size_gb: None,
            cost: CostClass::Fixed,
            vram_gate: crate::model::VramGate::default(),
            load_timeout_s: Some(900),
            min_turn_s: None,
            measured_gb: None,
            measured_min_gb: None,
            measured_offload: None,
            measured_attribution: None,
            measured_profile: None,
            measured_ram_gb: None,
            measured_ram_min_gb: None,
            fit_gb: None,
            model_ref: None,
            serve_model: Some("maya-test".to_string()),
            file: None,
            raw: raw(&pairs),
        };
        for (key, value) in overrides {
            match *key {
                "serve_model" => model.serve_model = Some((*value).to_string()),
                "load_timeout" => model.load_timeout_s = value.parse().ok(),
                "cost" if *value == "greedy" => model.cost = CostClass::Greedy,
                "cost" if *value == "exact" => model.cost = CostClass::Exact,
                "vram_gb" if value.is_empty() => model.vram_gb = None,
                _ => {}
            }
        }
        (dir, config, model)
    }

    fn config_of(model: &ModelEntry) -> serde_json::Value {
        let row = validate_row(model).expect("fixture validates");
        serde_json::from_str(&row.text).expect("rendered JSON")
    }

    #[test]
    fn the_argv_is_the_forced_server_prefix_and_the_staged_config() {
        let (dir, _config, model) = fixture(&[]);
        let spec = MayaAdapter.spec(&model, 18199).expect("fixture renders");
        let expected: Vec<String> = vec![
            dir.join("python.exe").to_string_lossy().into_owned(),
            "-m".to_string(),
            "serve.server".to_string(),
            "--engine".to_string(),
            "maya".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--config".to_string(),
            spec.argv[8].clone(),
            "--port".to_string(),
            "18199".to_string(),
            "--idle-unload".to_string(),
            "0".to_string(),
        ];
        assert_eq!(spec.argv, expected);
        assert!(
            spec.argv[8].ends_with(".json") && spec.argv[8].contains("maya-test"),
            "the staged config slot carries the content-addressed name"
        );
        assert_eq!(
            spec.argv[8],
            crate::process::staged_profile_name(
                &model.name,
                &validate_row(&model).expect("fixture validates").text
            ),
            "the `--config` slot carries exactly the name the staging door mints for these bytes"
        );
        assert_eq!(spec.cwd, dir);
        assert_eq!(spec.env.get("PYTHONUTF8").map(String::as_str), Some("1"));
        assert_eq!(spec.log_name, "maya-test");
        assert_eq!(
            spec.readiness,
            ReadinessSpec::HttpString {
                path: "/health".to_string(),
                expect_status: 200,
                pointer: "/model".to_string(),
                expect: "maya-test".to_string(),
            }
        );
        let profile = spec.rendered_profile.expect("maya stages a config");
        assert_eq!(profile.engine, "maya");
    }

    #[test]
    fn the_staged_config_strips_the_harness_owned_decisions() {
        let (dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        let object = value.as_object_mut().expect("object");
        object.insert("host".to_string(), serde_json::json!("0.0.0.0"));
        object.insert("port".to_string(), serde_json::json!(8006));
        object.insert("api_key".to_string(), serde_json::json!("secret"));
        object.insert("idle_unload_s".to_string(), serde_json::json!(600));
        object.insert(
            "cors_origins".to_string(),
            serde_json::json!(["http://evil.example"]),
        );
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let rendered = config_of(&model);
        for key in ["host", "port", "api_key", "idle_unload_s", "cors_origins"] {
            assert!(
                rendered.get(key).is_none(),
                "the staged config must not carry `{key}`"
            );
        }
        assert_eq!(rendered["model_name"], serde_json::json!("maya-test"));
        assert_eq!(rendered["cwd"], serde_json::json!(dir.to_string_lossy()));
    }

    /// Model management is the server's own default ON, and with it a local
    /// request frees the booked resident outside the scheduler (`/models/unload`,
    /// `keep_alive: 0`). The staged bytes therefore carry `false` whatever L1
    /// says: both an absent key and an explicit `true` are forced off.
    /// Falsifier: dropping the post-scrub insert keeps the absent-key leg
    /// green only until this pin reads the staged bytes, and an `true` L1
    /// passes straight through.
    #[test]
    fn model_management_is_forced_off_whatever_the_config_says() {
        let (_dir, config, model) = fixture(&[]);
        let rendered = config_of(&model);
        assert_eq!(
            rendered["model_management"],
            serde_json::json!(false),
            "a config that omits the key still stages it off (the server's default is on)"
        );
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["model_management"] = serde_json::json!(true);
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let rendered = config_of(&model);
        assert_eq!(
            rendered["model_management"],
            serde_json::json!(false),
            "an L1 `model_management: true` must not turn the lifecycle door back on"
        );
    }

    /// Maya treats a missing `args` as none; only a present value of the wrong
    /// shape is refused. Falsifier: requiring the key refuses a config Maya
    /// would serve happily, and dropping the shape check lets a JSON object
    /// reach the argv the server builds from the config.
    #[test]
    fn an_absent_args_key_is_accepted_and_a_foreign_shape_is_refused() {
        let (_dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value.as_object_mut().expect("object").remove("args");
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        assert!(validate_row(&model).is_ok(), "absent `args` is legal Maya");
        value["args"] = serde_json::json!({"not": "a list"});
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let error = validate_row(&model)
            .expect_err("args shape")
            .into_engine_error()
            .to_string();
        assert!(error.contains("args"), "must name `args`: {error}");
        value["args"] = serde_json::json!(["ok", 7]);
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        assert!(
            validate_row(&model).is_err(),
            "a non-string entry is refused"
        );
    }

    #[test]
    fn the_render_is_byte_stable() {
        let (_dir, _config, model) = fixture(&[]);
        let first = validate_row(&model).expect("first").text;
        let second = validate_row(&model).expect("second").text;
        assert_eq!(first, second);
    }

    /// The effective served context is the config's own `--max-context`
    /// token. Falsifier: accepting a non-digit value slot, losing the `=`
    /// spelling, letting the first occurrence win, or reading the row keys
    /// instead of the argv each fails one leg here.
    #[test]
    fn the_figure_reads_the_config_max_context_token() {
        let cases: &[(&[&str], Option<&str>)] = &[
            (
                &["--glm-pack", "pack", "--max-context", "131072"],
                Some("131072"),
            ),
            (&["--max-context=262144"], Some("262144")),
            (&["--glm-pack", "pack"], None),
            (&["--max-context", "auto"], None),
            (&["--max-context", "--glm-pack"], None),
            (&["--max-context"], None),
            (&["--max-context="], None),
            (&["--max-context=auto"], None),
            (&["--max-context", "-5"], None),
            (&["--max-contexts", "8192"], None),
            // argv truth: the last spelling wins, and a later malformed slot
            // does not resurrect an earlier figure.
            (
                &["--max-context", "8192", "--max-context=131072"],
                Some("131072"),
            ),
            (&["--max-context=131072", "--max-context"], None),
        ];
        for (args, want) in cases {
            assert_eq!(max_context_of(args).as_deref(), *want, "args {args:?}");
        }
    }

    /// The effective context through the single door: the figure the config
    /// spells rides the
    /// validated row. Falsifier: dropping the capture from `validate_row`
    /// leaves the row silent whatever the config says.
    #[test]
    fn validate_row_carries_the_figure_the_config_spells() {
        let (_dir, config, model) = fixture(&[]);
        let row = validate_row(&model).expect("fixture validates");
        assert_eq!(
            row.max_context.as_deref(),
            Some("131072"),
            "the fixture config spells `--max-context 131072`"
        );
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["args"] = serde_json::json!(["--max-context=262144"]);
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let row = validate_row(&model).expect("`=` form validates");
        assert_eq!(row.max_context.as_deref(), Some("262144"));
        value["args"] = serde_json::json!(["--glm-pack", "pack"]);
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let row = validate_row(&model).expect("args without a figure validate");
        assert_eq!(
            row.max_context, None,
            "a config without the token spells no figure"
        );
    }

    #[test]
    fn a_greedy_row_needs_no_nominal_size() {
        let (_dir, _config, model) = fixture(&[("cost", "greedy"), ("vram_gb", "")]);
        assert!(validate_row(&model).is_ok());
    }

    #[test]
    fn refusals_name_the_offending_key_or_field() {
        let cases: &[(&str, &str, &str)] = &[
            ("maya_python", "Z:/absent/python.exe", "maya_python"),
            ("load_timeout", "120", "load_timeout"),
            ("serve_model", "other", "serve_model"),
            ("cost", "exact", "cost"),
        ];
        for (key, value, named) in cases {
            let (_dir, _config, model) = fixture(&[(key, value)]);
            let error = validate_row(&model)
                .expect_err(key)
                .into_engine_error()
                .to_string();
            assert!(
                error.contains(named),
                "`{key}={value}` refusal must name `{named}`: {error}"
            );
        }
        let (_dir, _config, model) = fixture(&[("maya_config", "Z:/absent.json")]);
        assert!(validate_row(&model).is_err());
    }

    #[test]
    fn a_config_model_name_mismatch_is_refused() {
        let (_dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["model_name"] = serde_json::json!("other");
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let error = validate_row(&model)
            .expect_err("mismatch")
            .into_engine_error()
            .to_string();
        assert!(error.contains("serve_model") && error.contains("other"));
    }

    /// The documented engine settings in the L1 `env` ride the staged profile:
    /// Maya writes them there under the engine's own names (`HY4_DENSE_MAX` on
    /// Hy4-Flash; `DS4_*`, `IK2_*`, and the older `STRATA_*` fallbacks on the
    /// others), so the scan must not refuse them. Falsifier: on the pre-fix
    /// tree the `MAYA_*`-only door refused `HY4_DENSE_MAX`, so the Hy4-Flash
    /// row could not scan or load.
    #[test]
    fn documented_engine_env_settings_ride_the_staged_profile() {
        let settings = serde_json::json!({
            "HY4_DENSE_MAX": "8192",
            "DS4_KEEP_THINKING": 1,
            "STRATA_GLM_CPU_LANE": true,
            "SOME_FUTURE_SETTING": "1",
        });
        let (_dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["env"] = settings.clone();
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let rendered = config_of(&model);
        assert_eq!(
            rendered["env"], settings,
            "the engine settings stay in the staged bytes as they were spelled"
        );
        value["env"] = serde_json::json!({"HY4_DENSE_MAX": {"nested": true}});
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let error = validate_row(&model)
            .expect_err("non-scalar value")
            .into_engine_error()
            .to_string();
        assert!(
            error.contains("env.HY4_DENSE_MAX"),
            "a value that is not a scalar must be refused by name: {error}"
        );
    }

    /// The names the harness owns are still refused by name, whatever their
    /// case: the process search path, the interpreter setup, and the GPU
    /// selection the row's `gpu` key decides. Falsifier: an open door would
    /// let the engine-side file rewrite the child's `PATH`, `PYTHONPATH`, or
    /// `CUDA_VISIBLE_DEVICES` outside the harness's decision.
    #[test]
    fn a_harness_owned_env_key_is_refused_by_name() {
        for name in [
            "PATH",
            "cuda_visible_devices",
            "PYTHONPATH",
            "VIRTUAL_ENV",
            "TMPDIR",
            "LD_PRELOAD",
            "LD_AUDIT",
        ] {
            let (_dir, config, model) = fixture(&[]);
            let mut value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&config).expect("read"))
                    .expect("json");
            value["env"] = serde_json::json!({name: "x"});
            std::fs::write(&config, serde_json::to_string(&value).expect("serialize"))
                .expect("write");
            let error = validate_row(&model)
                .expect_err(name)
                .into_engine_error()
                .to_string();
            assert!(
                error.contains(&format!("env.{name}")),
                "`{name}` refusal must name it: {error}"
            );
            assert!(
                error.contains("harness-owned"),
                "`{name}` refusal must say why: {error}"
            );
        }
    }

    /// A name or value that cannot become an engine environment entry is refused
    /// at the scan: the empty name, `=`, and NUL are not legal, and the Maya
    /// server's `os.environ` copy raises `ValueError` after the harness has
    /// already admitted the load. Falsifier: the old `MAYA_*` gate refused these
    /// shapes; the open door must not lose that refusal.
    #[test]
    fn an_illegal_env_name_or_value_is_refused() {
        for name in ["", "A=B", "A\u{0}B"] {
            let (_dir, config, model) = fixture(&[]);
            let mut value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&config).expect("read"))
                    .expect("json");
            value["env"] = serde_json::json!({name: "x"});
            std::fs::write(&config, serde_json::to_string(&value).expect("serialize"))
                .expect("write");
            let error = validate_row(&model)
                .expect_err(name)
                .into_engine_error()
                .to_string();
            assert!(
                error.contains("legal environment name"),
                "`{name:?}` must be refused as an illegal name: {error}"
            );
        }
        let (_dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["env"] = serde_json::json!({"OK": "x\u{0}y"});
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let error = validate_row(&model)
            .expect_err("NUL value")
            .into_engine_error()
            .to_string();
        assert!(
            error.contains("env.OK") && error.contains("NUL"),
            "a NUL value must be refused naming the key: {error}"
        );
    }

    #[test]
    fn a_missing_vision_asset_is_a_missing_path() {
        let (dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["vision"] = serde_json::json!({
            "exe": dir.join("maya-vision.exe"),
            "mmproj": dir.join("mmproj.gguf"),
            "model": dir.join("vision.gguf"),
        });
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let error = validate_row(&model).expect_err("vision path");
        assert!(
            matches!(error, MayaRowError::MissingPath { .. }),
            "unexpected refusal: {error:?}"
        );
    }

    /// The three real overlay assets under one temp dir, in the line order
    /// `exe`, `mmproj`, `model`.
    fn overlay_files(dir: &Path) -> [PathBuf; 3] {
        let files = [
            dir.join("vision.exe"),
            dir.join("mmproj.gguf"),
            dir.join("vision.gguf"),
        ];
        std::fs::create_dir_all(dir).expect("assets dir");
        for file in &files {
            std::fs::write(file, b"v").expect("overlay asset");
        }
        files
    }

    /// The full `k=v` line naming `overlay_files`, plus any extra tail.
    fn overlay_line(files: &[PathBuf; 3], extra: &str) -> String {
        let mut line = format!(
            "exe={} mmproj={} model={}",
            files[0].to_str().expect("utf-8"),
            files[1].to_str().expect("utf-8"),
            files[2].to_str().expect("utf-8"),
        );
        if !extra.is_empty() {
            line.push(' ');
            line.push_str(extra);
        }
        line
    }

    /// The typed `maya_vision` overlay: the row yaml alone
    /// renders the staged `vision` object, the staged bytes stay byte-stable,
    /// and a row without the overlay stages none. Falsifier: on the pre-fix
    /// tree `maya_vision` was an ignored key — the staged profile carried no
    /// `vision` object whatever the row spelled.
    #[test]
    fn the_row_overlay_renders_the_vision_object_from_the_row_alone() {
        let assets = crate::testutil::temp_dir("maya-vision-overlay");
        let files = overlay_files(&assets);
        let line = overlay_line(&files, "");
        let (_dir, _config, model) = fixture(&[("maya_vision", line.as_str())]);
        let rendered = config_of(&model);
        for (key, file) in [
            ("exe", &files[0]),
            ("mmproj", &files[1]),
            ("model", &files[2]),
        ] {
            assert_eq!(
                rendered["vision"][key],
                serde_json::json!(file.to_str().expect("utf-8")),
                "the overlay renders `{key}` as spelled"
            );
        }
        assert_eq!(
            rendered["vision"]["gpu"],
            serde_json::json!(true),
            "the operator's engine-side block carries `gpu: true`, so the overlay default matches"
        );
        let first = validate_row(&model).expect("overlay validates").text;
        let second = validate_row(&model).expect("overlay revalidates").text;
        assert_eq!(first, second, "the overlaid bytes stay byte-stable");
        let without = config_of(&fixture(&[]).2);
        assert!(
            without.get("vision").is_none(),
            "the row alone is what changes the vision object"
        );
    }

    /// `exe`, `mmproj` and `model` are required together: a missing one is
    /// refused before the load gesture, naming the overlay key, the missing
    /// member and the row. Falsifier: accepting a half block lets the server
    /// die on a vision spawn the scan could have refused.
    #[test]
    fn the_overlay_requires_every_path_key_naming_the_row_and_the_key() {
        let assets = crate::testutil::temp_dir("maya-vision-required");
        let files = overlay_files(&assets);
        let names = ["exe", "mmproj", "model"];
        for omit in 0..3 {
            let tokens: Vec<String> = names
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != omit)
                .map(|(i, name)| format!("{name}={}", files[i].to_str().expect("utf-8")))
                .collect();
            let (_dir, _config, model) = fixture(&[("maya_vision", tokens.join(" ").as_str())]);
            let error = validate_row(&model)
                .expect_err("half block")
                .into_engine_error()
                .to_string();
            assert!(
                error.contains("maya_vision") && error.contains(&format!("`{}`", names[omit])),
                "the refusal must name `maya_vision` and the missing `{}`: {error}",
                names[omit]
            );
            assert!(
                error.contains("maya-test"),
                "the refusal must name the row: {error}"
            );
        }
    }

    /// The overlay and an engine-side `vision` object refuse each other, and
    /// the conflict is judged before any overlay path is touched: no silent
    /// precedence either way. Falsifier: merging one over the other silently
    /// changes what the operator sees on disk versus what serves.
    #[test]
    fn the_overlay_refuses_a_conflict_with_the_config_vision_object() {
        let assets = crate::testutil::temp_dir("maya-vision-conflict");
        let files = overlay_files(&assets);
        let line = overlay_line(&files, "");
        let (_dir, config, model) = fixture(&[("maya_vision", line.as_str())]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["vision"] = serde_json::json!({
            "exe": files[0], "mmproj": files[1], "model": files[2], "gpu": true,
        });
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let error = validate_row(&model)
            .expect_err("conflict")
            .into_engine_error()
            .to_string();
        assert!(
            error.contains("maya_vision") && error.contains("vision"),
            "the conflict refusal names both spellings: {error}"
        );
        assert!(
            error.contains("conflicts") && error.contains("precedence"),
            "the refusal must read as the named conflict, not a generic reject: {error}"
        );
    }

    /// The key set is closed: an unknown key is refused naming the set, and a
    /// token that is not `k=v` is refused the same way. Falsifier: an open set
    /// lets an unreported engine key ride the overlay.
    #[test]
    fn the_overlay_refuses_an_unknown_key_naming_the_closed_set() {
        let assets = crate::testutil::temp_dir("maya-vision-closed");
        let files = overlay_files(&assets);
        for tail in ["threads=8", "gpu"] {
            let line = overlay_line(&files, tail);
            let (_dir, _config, model) = fixture(&[("maya_vision", line.as_str())]);
            let error = validate_row(&model)
                .expect_err(tail)
                .into_engine_error()
                .to_string();
            assert!(
                error.contains("exe, mmproj, model, gpu"),
                "`{tail}` refusal must name the closed set: {error}"
            );
            assert!(
                error.contains("maya_vision"),
                "`{tail}` refusal must name `maya_vision`: {error}"
            );
        }
    }

    /// An overlay asset that is not on disk is the [`MayaRowError::MissingPath`]
    /// door naming `maya_vision.<key>` (the same class as the config-side door);
    /// a relative path is the named bad value, not a path miss. Falsifier:
    /// checking only the shape lets a doomed vision spawn evict a resident.
    #[test]
    fn a_missing_overlay_asset_is_a_missing_path_naming_the_row_key() {
        let assets = crate::testutil::temp_dir("maya-vision-missing");
        std::fs::create_dir_all(&assets).expect("assets dir");
        std::fs::write(assets.join("vision.exe"), b"v").expect("overlay exe");
        let line = format!(
            "exe={} mmproj={} model={}",
            assets.join("vision.exe").to_str().expect("utf-8"),
            assets.join("absent-mmproj.gguf").to_str().expect("utf-8"),
            assets.join("absent.gguf").to_str().expect("utf-8"),
        );
        let (_dir, _config, model) = fixture(&[("maya_vision", line.as_str())]);
        let error = validate_row(&model).expect_err("overlay path");
        match error {
            MayaRowError::MissingPath { key, path, .. } => {
                assert_eq!(
                    key, "maya_vision.mmproj",
                    "the key names the overlay and the member"
                );
                assert!(path.to_string_lossy().contains("absent-mmproj"), "{path:?}");
            }
            other => panic!("unexpected refusal: {other:?}"),
        }
        let files = overlay_files(&assets);
        let relative = format!(
            "exe=vision.exe mmproj={} model={}",
            files[1].to_str().expect("utf-8"),
            files[2].to_str().expect("utf-8"),
        );
        let (_dir, _config, model) = fixture(&[("maya_vision", relative.as_str())]);
        let error = validate_row(&model).expect_err("relative path");
        match error {
            MayaRowError::Bad { key, reason, .. } => {
                assert_eq!(key, "maya_vision.exe");
                assert!(reason.contains("absolute"), "{reason}");
            }
            other => panic!("a relative overlay path is a bad value: {other:?}"),
        }
    }

    /// `gpu` is a strict figure with a default: `gpu=false` renders `false`,
    /// a truthy word renders `true`, and an omitted `gpu` renders `true` —
    /// the operator's engine-side block spells `gpu: true`, so the overlay
    /// default keeps that figure. Falsifier: dropping the default insert (or
    /// flipping it) silently moves the encoder off-card.
    #[test]
    fn the_overlay_gpu_flag_defaults_on_and_reads_the_spelled_figure() {
        let assets = crate::testutil::temp_dir("maya-vision-gpu");
        let files = overlay_files(&assets);
        let off = overlay_line(&files, "gpu=false");
        let (_dir, _config, model) = fixture(&[("maya_vision", off.as_str())]);
        assert_eq!(
            config_of(&model)["vision"]["gpu"],
            serde_json::json!(false),
            "`gpu=false` renders false"
        );
        let on = overlay_line(&files, "gpu=on");
        let (_dir, _config, model) = fixture(&[("maya_vision", on.as_str())]);
        assert_eq!(
            config_of(&model)["vision"]["gpu"],
            serde_json::json!(true),
            "a truthy word renders true"
        );
        let omitted = overlay_line(&files, "");
        let (_dir, _config, model) = fixture(&[("maya_vision", omitted.as_str())]);
        assert_eq!(
            config_of(&model)["vision"]["gpu"],
            serde_json::json!(true),
            "an omitted `gpu` renders true, the engine-side figure"
        );
    }

    /// `gpu` accepts exactly the eight words: any other value — a typo, a
    /// number outside the set, or the blank of `gpu=` — is a named
    /// `maya_vision.gpu` refusal naming the accepted words, never a silent
    /// `false`. Falsifier: the loose truthy table reads every unknown value
    /// (and every blank) as false and moves the encoder off-card unseen.
    #[test]
    fn the_overlay_refuses_a_gpu_value_outside_the_accepted_words() {
        let assets = crate::testutil::temp_dir("maya-vision-gpu-strict");
        let files = overlay_files(&assets);
        for value in ["maybe", "2", ""] {
            let tail = format!("gpu={value}");
            let line = overlay_line(&files, &tail);
            let (_dir, _config, model) = fixture(&[("maya_vision", line.as_str())]);
            let error = validate_row(&model)
                .expect_err("gpu value")
                .into_engine_error()
                .to_string();
            assert!(
                error.contains("maya_vision.gpu")
                    && error.contains("true, false, 1, 0, yes, no, on, off"),
                "`{tail}` refusal must name `maya_vision.gpu` and the accepted words: {error}"
            );
        }
    }

    /// The block spells each key once: a duplicate of any of the four is
    /// refused naming `maya_vision` and the key, never a silent last-wins.
    /// Falsifier: a plain `insert` lets a tail token silently replace the
    /// head path or flip the head figure.
    #[test]
    fn the_overlay_refuses_a_duplicate_key_naming_the_key() {
        let assets = crate::testutil::temp_dir("maya-vision-duplicate");
        let files = overlay_files(&assets);
        let base = overlay_line(&files, "gpu=true");
        for (key, value) in [
            ("exe", files[0].to_str().expect("utf-8")),
            ("mmproj", files[1].to_str().expect("utf-8")),
            ("model", files[2].to_str().expect("utf-8")),
            ("gpu", "false"),
        ] {
            let line = format!("{base} {key}={value}");
            let (_dir, _config, model) = fixture(&[("maya_vision", line.as_str())]);
            let error = validate_row(&model)
                .expect_err("duplicate")
                .into_engine_error()
                .to_string();
            assert!(
                error.contains("maya_vision") && error.contains(&format!("`{key}`")),
                "the duplicate refusal must name `maya_vision` and `{key}`: {error}"
            );
        }
    }

    /// Regression pin (not a falsifier — this leg passed before the overlay
    /// and must pass after it): an engine-side `vision` block still flows
    /// through the staged bytes as validated (a doomed one is refused by the
    /// existing `check_vision` door), and a config without vision stages
    /// none. A future change that runs the overlay conflict check
    /// unconditionally would refuse the untouched engine-side shape here.
    #[test]
    fn an_absent_overlay_leaves_the_engine_side_vision_door_unchanged() {
        let assets = crate::testutil::temp_dir("maya-vision-engine-side");
        let files = overlay_files(&assets);
        let (_dir, config, model) = fixture(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
        value["vision"] = serde_json::json!({
            "exe": files[0], "mmproj": files[1], "model": files[2], "gpu": true,
        });
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let rendered = config_of(&model);
        assert_eq!(
            rendered["vision"]["exe"],
            serde_json::json!(files[0].to_str().expect("utf-8")),
            "the engine-side block still rides the staged bytes unmanaged"
        );
        value.as_object_mut().expect("object").remove("vision");
        std::fs::write(&config, serde_json::to_string(&value).expect("serialize")).expect("write");
        let rendered = config_of(&model);
        assert!(
            rendered.get("vision").is_none(),
            "a config without vision stages none: {rendered}"
        );
    }

    #[test]
    fn a_fixed_row_without_vram_is_refused() {
        let (_dir, _config, model) = fixture(&[("vram_gb", "")]);
        let error = validate_row(&model)
            .expect_err("vram")
            .into_engine_error()
            .to_string();
        assert!(error.contains("vram_gb"));
    }

    #[test]
    fn a_bom_config_is_read() {
        let (_dir, config, model) = fixture(&[]);
        let text = std::fs::read_to_string(&config).expect("read");
        std::fs::write(&config, format!("\u{feff}{text}")).expect("write");
        assert!(validate_row(&model).is_ok());
    }
}
