//! The xolotl engine adapter.
//!
//! Xolotl's DeepSeek-V4.1-Flash serve is the engine binary `ds41_generate.exe`
//! driven behind the operator's `serve_ds41.py` shim: the shim reads the same
//! serve-profile JSON dialect the strata server does, answers `/health` with
//! the `loaded` flag and `/metrics` with the `engine` block, so readiness and
//! measurement ride the strata contract unchanged. The launch is
//! `python -I <launcher> --engine strata --host 127.0.0.1 --config <staged
//! profile> --port <slot>`: `-I` keeps the interpreter isolated from
//! environment-import noise, and every forced token rides the `xolotl` row of
//! the [`crate::engine::policy`] table (engine tag, loopback bind, rendered
//! profile position, harness-assigned port) exactly as the strata row forces
//! its own.
//!
//! Render mode is the strata idiom (ruling 2): the harness renders its own
//! profile from row knobs at every load, stages it content-addressed under
//! `.simpleharn/xolotl-profiles`, and rewrites `--config` to exactly that
//! path. The operator JSON is a value source, never launch truth. `host`,
//! `port` and key fields are absent by construction — the bind is the forced
//! loopback `--host` argv and the port the forced `--port` — so ruling 22\[1\]
//! holds by rendering, not by policing. The shim `chdir`s to its tree root
//! itself (the spawn cwd is not load-bearing) and absolutizes `--config`.
//!
//! The row knobs mirror the strata shape (ruling 5): open `xolotl_args` and
//! `xolotl_env` with the same forced refusals, plus the surface the tested
//! 256K line needs and strata does not spell today: `xolotl_python`,
//! `xolotl_launcher`, `xolotl_cwd` (the relative `-m` cache workaround
//! resolves against it), `xolotl_exe`, `xolotl_tokenizer`, `xolotl_aliases`
//! and `xolotl_reasoning_budget`. The env allow-list is `STRATA_*` plus the
//! single curated `GGML_NO_BACKTRACE` (the only non-`STRATA_` variable in the
//! tested config). A DSpark sidecar the row names but the disk lacks
//! (`STRATA_SERVE_DSPARK`, `STRATA_DSPARK_KEEP`) refuses at scan and spec by
//! path (ruling 6); absence means no DSpark.
//!
//! The memory posture (ruling 3b): `cost: greedy` with
//! `launch_only_with_at_least_vram_free: whole` — a whole-card resident the
//! greedy admission never evicts for, replaced only in its own slot; the row
//! row carries the `expert_offload` cell so a host-RAM reclaim may drain CPU
//! residents above the threshold. One 256K row only (`--ctx 262144`,
//! `--kv-q8`); the 32K serve and the manual `:8011` instance stay
//! operator-side and never coexist with a harness load (ruling 7).

use super::{EngineAdapter, EngineError, LaunchSpec, ReadinessSpec, RenderedProfile};
use crate::model::ModelEntry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The shortest `load_timeout` a xolotl row may declare, in seconds. The cold
/// first load (requant plus weight stream) measured 43-46 s on top of a
/// ~25 s requant with a cold cache, and a warm load still pages ~71 GiB of
/// experts; the global 120 s default would kill a healthy load. The row
/// states its own number (the maya `MAYA_MIN_LOAD_TIMEOUT_S` posture).
pub(crate) const XOLOTL_MIN_LOAD_TIMEOUT_S: u64 = 600;

/// The only non-`STRATA_` environment variable the allow-list curates
/// (ruling 5: the tested config shows `GGML_NO_BACKTRACE` as its lone
/// exception; any other foreign name keeps the strata refusal voice).
const CURATED_ENV: &str = "GGML_NO_BACKTRACE";

/// The environment variables whose value is a tree path that must exist at
/// scan and spec time (ruling 6): a DSpark sidecar the row names and the disk
/// lacks is a doomed load, refused by path before admission can evict a
/// resident. Absence means no DSpark.
const PATH_ENV_CHECKS: &[&str] = &["STRATA_SERVE_DSPARK", "STRATA_DSPARK_KEEP"];

/// The profile fields that claim a harness-owned decision — the xolotl voice
/// of the strata `PROFILE_POLICY_FIELDS` table (the `DECISIONS`
/// LOCKED "external engines obey only harness-owned configuration"). The
/// renderer structurally cannot emit one; reaching this leg means a renderer
/// bug, and the launch is refused anyway.
const PROFILE_POLICY_FIELDS: &[(&str, &str)] = &[
    (
        "host",
        "the engine bind is harness-forced (loopback, \u{a7}5)",
    ),
    (
        "port",
        "the engine port is harness-assigned from category config",
    ),
    ("key", "request authentication is harness-owned"),
    ("api_key", "request authentication is harness-owned"),
];

/// Why a xolotl row is not loadable. The scan maps each leg to its
/// `ScanKind`, the adapter refuses every leg the same
/// [`EngineError`] way — one door shape at both doors (the basalt
/// `BasaltRowError` idiom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum XolotlRowError {
    /// The row carries no value for a key the launch requires.
    Missing {
        /// Harness name of the model.
        model: String,
        /// The absent required key.
        field: &'static str,
    },
    /// A path the row names that the disk lacks, keyed by the yaml key
    /// (or the engine flag) the operator has to fix — the
    /// `ModelPathMissing` door.
    MissingPath {
        /// Harness name of the model.
        model: String,
        /// The yaml key or engine flag carrying the path.
        key: String,
        /// The missing path as spelled (cwd-resolved when it was relative).
        path: String,
    },
    /// The rendered profile claims top-level fields the harness forces
    /// (22[1]). Defense in depth: the renderer cannot emit one.
    Claim {
        /// Harness name of the model.
        model: String,
        /// `(field, owner)` for each claimed field.
        claims: Vec<(&'static str, &'static str)>,
    },
    /// A present value the key cannot carry, naming the key.
    Bad {
        /// Harness name of the model.
        model: String,
        /// The key whose shape the value broke.
        key: String,
        /// The shape explanation (G6).
        reason: String,
    },
}

impl XolotlRowError {
    /// The same refusal in the adapter voice [`EngineAdapter::spec`] returns:
    /// a missing key meets [`EngineError::MissingField`], every shaped
    /// refusal meets [`EngineError::Rejected`] naming the key or path.
    #[must_use]
    pub(crate) fn into_engine_error(self) -> EngineError {
        match self {
            Self::Missing { model, field } => EngineError::MissingField {
                model,
                field: field.to_string(),
            },
            Self::MissingPath { model, key, path } => EngineError::Rejected {
                engine: "xolotl".to_string(),
                model,
                reason: format!("xolotl profile references missing path: {key} = {path}"),
            },
            Self::Claim { model, claims } => EngineError::Rejected {
                engine: "xolotl".to_string(),
                model,
                reason: policy_field_claims(&claims),
            },
            Self::Bad { model, key, reason } => EngineError::Rejected {
                engine: "xolotl".to_string(),
                model,
                reason: format!("`{key}` {reason}"),
            },
        }
    }
}

/// The refusal sentence for a profile that claims harness-forced fields, in
/// the xolotl voice (every claim named at once with its owning mechanism —
/// the strata `policy_field_claims` posture).
fn policy_field_claims(claims: &[(&str, &str)]) -> String {
    let listed = claims
        .iter()
        .map(|(field, owner)| format!("`{field}` \u{2014} {owner}"))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "xolotl profile declares {listed}; policy fields are harness-forced and \
         never obeyed from engine-side files: delete them from the row"
    )
}

/// The validated row [`validate_row`] hands the adapter and the scan: the
/// launch inputs the spec needs plus the exact rendered profile bytes.
#[derive(Debug)]
pub(crate) struct XolotlRow {
    /// The existence-checked interpreter (`argv[0]`).
    pub python: PathBuf,
    /// The existence-checked engine working directory.
    pub cwd: PathBuf,
    /// The rendered profile JSON (the staged bytes, content-addressed).
    pub text: String,
}

/// One optional yaml value, trimmed; an empty or whitespace-only value reads
/// as absent (the strata idiom).
fn knob<'a>(model: &'a ModelEntry, key: &str) -> Option<&'a str> {
    model
        .raw
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
}

/// The `xolotl_args` tokenizer, the `--host`/`--port` deny, and the path
/// existence checks the launch depends on, fused: each comma entry is a
/// whitespace-separated group of engine argv words (`--backend cuda`), so the
/// render emits the engine's one-word-per-element `args` array the tested
/// profile spells. The `--engram <L1>;<L14>` value carries no whitespace and
/// no comma — one word, so the comma splitter stays safe (ruling 5).
///
/// # Errors
///
/// [`XolotlRowError::Bad`] for a malformed line or a harness-forced flag the
/// row spelled; [`XolotlRowError::MissingPath`] for a `-m`/`--engram`/`--rank`
/// path that does not exist (relative values resolved against `cwd` first —
/// the engine reads them from the tree root).
fn args_tokens(model: &ModelEntry, line: &str, cwd: &Path) -> Result<Vec<String>, XolotlRowError> {
    let entries = super::strata::well_formed_tokens(line).ok_or_else(|| XolotlRowError::Bad {
        model: model.name.clone(),
        key: "xolotl_args".to_string(),
        reason: format!("must be comma-separated non-empty engine tokens: {line}"),
    })?;
    let tokens: Vec<String> = entries
        .iter()
        .flat_map(|entry| entry.split_whitespace().map(str::to_string))
        .collect();
    if let Some(banned) = tokens.iter().find(|token| {
        // The ban reads the option token's *name*, not the whole token: the
        // eq form is spawn truth too (ruling 10[1]), so `--host=0.0.0.0`
        // must not smuggle past a whole-token comparison.
        let name = token
            .split_once('=')
            .map_or(token.as_str(), |(name, _)| name);
        name == "--host" || name == "--port"
    }) {
        return Err(XolotlRowError::Bad {
            model: model.name.clone(),
            key: "xolotl_args".to_string(),
            reason: format!("`{banned}` is harness-forced argv and must not be spelled by the row"),
        });
    }
    // The three flags whose value the launch cannot survive without: the
    // weights (`-m`, engine-side long alias `--model` accepted as the same
    // truth), the engram expert file list (host-separated inside one token)
    // and the rank sidecar. Relative spellings resolve against the tree root
    // (`xolotl_cwd`), exactly as the engine resolves them.
    let mut missing: Option<(&'static str, String)> = None;
    let mut it = tokens.iter();
    while let Some(token) = it.next() {
        let (name, inline) = match token.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (token.as_str(), None),
        };
        let flag = match name {
            "-m" | "--model" => "-m",
            "--engram" => "--engram",
            "--rank" => "--rank",
            _ => continue,
        };
        let value = match inline {
            Some(value) if !value.is_empty() => value.to_string(),
            Some(_) => {
                return Err(XolotlRowError::Bad {
                    model: model.name.clone(),
                    key: format!("xolotl_args {flag}"),
                    reason: format!("must carry a path, the eq value is empty: {token}"),
                })
            }
            None => match it.next() {
                Some(next) if !next.starts_with('-') => next.clone(),
                _ => {
                    return Err(XolotlRowError::Bad {
                        model: model.name.clone(),
                        key: format!("xolotl_args {flag}"),
                        reason: format!("must be followed by a path: {token}"),
                    })
                }
            },
        };
        if flag == "--engram" {
            for entry in value.split(ENGRAM_SEPARATOR) {
                let entry = entry.trim();
                if entry.is_empty() {
                    return Err(XolotlRowError::Bad {
                        model: model.name.clone(),
                        key: "xolotl_args --engram".to_string(),
                        reason: format!("entries are `;`-separated non-empty paths: {value}"),
                    });
                }
                if let Some(path) = missing_arg_path(entry, cwd) {
                    missing = Some(("--engram", path));
                    break;
                }
            }
        } else if let Some(path) = missing_arg_path(&value, cwd) {
            missing = Some((flag, path));
        }
        if missing.is_some() {
            break;
        }
    }
    if let Some((flag, path)) = missing {
        return Err(XolotlRowError::MissingPath {
            model: model.name.clone(),
            key: flag.to_string(),
            path,
        });
    }
    Ok(tokens)
}

/// The first engram/weights path that is not on disk, resolved against `cwd`
/// when relative; `None` when the path exists.
/// The engram file-list separator inside one argument: Windows uses `;`
/// (docs/WINDOWS.md), Linux uses `:` (the engine's POSIX spelling).
#[cfg(windows)]
const ENGRAM_SEPARATOR: char = ';';
/// The engram file-list separator inside one argument on Linux.
#[cfg(unix)]
const ENGRAM_SEPARATOR: char = ':';
fn missing_arg_path(raw: &str, cwd: &Path) -> Option<String> {
    let path = if super::strata::is_absolute_shape(raw) {
        PathBuf::from(raw)
    } else {
        cwd.join(raw)
    };
    (std::fs::metadata(&path).is_err()).then(|| path.to_string_lossy().into_owned())
}

/// The `xolotl_env` surface: whitespace-separated `NAME=VALUE` tokens where
/// the name is a `STRATA_`-prefixed engine setting or the curated
/// [`CURATED_ENV`] (ruling 5). Values keep the shapes the tested config
/// carries — plain words, floats and Windows paths — so the value charset is
/// wider than strata's (`._+-` plus `:/\`): the DSpark values ride drive
/// paths. The value still carries no whitespace (tokens are
/// whitespace-separated).
fn env_map(
    model: &ModelEntry,
    line: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, XolotlRowError> {
    let mut map = serde_json::Map::new();
    let bad = |why: &str, token: &str| XolotlRowError::Bad {
        model: model.name.clone(),
        key: "xolotl_env".to_string(),
        reason: format!(
            "tokens must be STRATA_<NAME>=<VALUE> or `{CURATED_ENV}=<VALUE>` ({why}): {token}"
        ),
    };
    for token in line.split_whitespace() {
        let Some((name, value)) = token.split_once('=') else {
            return Err(bad("one `=`", token));
        };
        let name_ok = if name == CURATED_ENV {
            true
        } else {
            name.strip_prefix("STRATA_").is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            })
        };
        if !name_ok {
            return Err(bad(
                "uppercase `STRATA_`-prefixed name or GGML_NO_BACKTRACE",
                token,
            ));
        }
        let value_ok = !value.is_empty()
            && value.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '.' | '_' | '-' | '+' | ':' | '\\' | '/' | '~')
            });
        if !value_ok {
            return Err(bad("plain value (letters, digits, `._+-:/\\~`)", token));
        }
        map.insert(
            name.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    Ok(map)
}

/// The DSpark existence door (ruling 6): an absolute-shaped value the disk
/// lacks refuses by path; a relative value is exempt (the shim resolves it
/// against the tree root — the same exemption the rendered-path walk carries)
/// and a missing key means no DSpark. The `:float` suffix shape
/// (`STRATA_DSPARK_KEEP <path>:64`) is stripped before the check — the path,
/// not the keep figure, is what must exist.
fn missing_dspark_path(
    env: &serde_json::Map<String, serde_json::Value>,
) -> Option<(&'static str, String)> {
    for key in PATH_ENV_CHECKS {
        let Some(value) = env.get(*key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(path) = super::strata::profile_path_candidate(value) {
            if std::fs::metadata(&path).is_err() {
                return Some((key, path));
            }
        }
    }
    None
}

/// The `xolotl_sampling` block: whitespace-separated `key=value` sampling
/// defaults (the server's `sampling` profile dict) — every value a number or
/// a bool literal, the two shapes the server validates there (the strata
/// `sampling_map` rule, xolotl key voice).
fn sampling_map(
    model: &ModelEntry,
    line: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, XolotlRowError> {
    let mut map = serde_json::Map::new();
    for token in line.split_whitespace() {
        let bad = || XolotlRowError::Bad {
            model: model.name.clone(),
            key: "xolotl_sampling".to_string(),
            reason: format!("tokens must be key=value with a number or bool: {token}"),
        };
        let Some((key, value)) = token.split_once('=') else {
            return Err(bad());
        };
        if key.is_empty() {
            return Err(bad());
        }
        let json = match value {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            other => match other.parse::<u64>() {
                Ok(n) => serde_json::json!(n),
                Err(_) => other
                    .parse::<f64>()
                    .map(serde_json::Value::from)
                    .map_err(|_| XolotlRowError::Bad {
                        model: model.name.clone(),
                        key: "xolotl_sampling".to_string(),
                        reason: format!("value must be a number or bool: {token}"),
                    })?,
            },
        };
        map.insert(key.to_string(), json);
    }
    Ok(map)
}

/// A comma-separated path list (`xolotl_lib_dirs`): the comma is the only
/// separator, so paths may hold spaces but never commas (the strata
/// `path_list` rule, xolotl key voice).
fn path_list(model: &ModelEntry, key: &str, line: &str) -> Result<Vec<String>, XolotlRowError> {
    let mut out = Vec::new();
    for entry in line.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(XolotlRowError::Bad {
                model: model.name.clone(),
                key: key.to_string(),
                reason: format!(
                    "entries must be non-empty, comma-separated absolute paths: {line}"
                ),
            });
        }
        if !super::strata::is_absolute_shape(entry) {
            return Err(XolotlRowError::Bad {
                model: model.name.clone(),
                key: key.to_string(),
                reason: format!("entries must be absolute paths (drive-letter or UNC): {entry}"),
            });
        }
        out.push(entry.to_string());
    }
    Ok(out)
}

/// An optional unsigned integer key (`xolotl_gpu`, `xolotl_reasoning_budget`).
fn unsigned(model: &ModelEntry, key: &str, value: &str) -> Result<u64, XolotlRowError> {
    value.parse::<u64>().map_err(|_| XolotlRowError::Bad {
        model: model.name.clone(),
        key: key.to_string(),
        reason: format!("must be a non-negative integer: {value}"),
    })
}

/// The pool range `xolotl_parallel` admits: the shim caps the server's worker
/// pool at 16 for this line (`ds41_parallel_args`), so a row above the cap
/// would render a figure the launch quietly shrinks — refused at scan instead.
const PARALLEL_MIN: u64 = 1;
/// The shim's worker-pool ceiling for `xolotl_parallel`.
const PARALLEL_MAX: u64 = 16;

/// The closed `xolotl_vision` key set, in refusal voice.
const VISION_KEYS: &str = "exe, python, mmproj, model, reference_dir, weights, \
                           dtype, device, offload, tf32, warmup_tokens, threads, \
                           max_tokens, min_tokens, gpu";

/// The `xolotl_vision` block: whitespace-separated `k=v` tokens over the
/// shim's full vision contract — `Ds41Vision` in `serve_ds41.py:299-327`
/// replaces the spawn's program and layers its own keys on the inherited
/// server `Vision` class. `exe`, `python`, `mmproj`, `model`, `reference_dir`
/// and `weights` are absolute paths (`exe`, `mmproj` and `model` required
/// together — the strata rule, the spawn cannot start without them); `dtype`
/// is `f32|bf16|f16` and `device` is `cpu|cuda` (the encoder's own spellings);
/// `offload`, `tf32` and `gpu` are truthy flags — `gpu` DEFAULTS OFF here,
/// unlike `strata_vision`: the tested 256K line keeps the encoder off-card
/// unless the row opts it onto one; `warmup_tokens`, `threads`, `max_tokens`
/// and `min_tokens` are unsigned counts inherited from the server Vision
/// class. Rendering the block appends exactly one `--vision` to `args`; the row
/// spells the vision ONCE, in the block.
fn vision_block(
    model: &ModelEntry,
    line: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, XolotlRowError> {
    let bad = |reason: String| XolotlRowError::Bad {
        model: model.name.clone(),
        key: "xolotl_vision".to_string(),
        reason,
    };
    let mut taken = serde_json::Map::new();
    for token in line.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            return Err(bad(format!(
                "tokens must be k=v over {VISION_KEYS}: {token}"
            )));
        };
        match key {
            "exe" | "python" | "mmproj" | "model" | "reference_dir" | "weights" => {
                if !super::strata::is_absolute_shape(value) {
                    return Err(bad(format!(
                        "{key} must be an absolute path (drive-letter or UNC): {value}"
                    )));
                }
                taken.insert(
                    key.to_string(),
                    serde_json::Value::String(value.to_string()),
                );
            }
            "dtype" => {
                let mode = value.to_ascii_lowercase();
                if !matches!(mode.as_str(), "f32" | "bf16" | "f16") {
                    return Err(bad(format!("dtype must be f32, bf16 or f16: {value}")));
                }
                taken.insert("dtype".to_string(), serde_json::Value::String(mode));
            }
            "device" => {
                let place = value.to_ascii_lowercase();
                if !matches!(place.as_str(), "cpu" | "cuda") {
                    return Err(bad(format!("device must be cpu or cuda: {value}")));
                }
                taken.insert("device".to_string(), serde_json::Value::String(place));
            }
            "offload" | "tf32" | "gpu" => {
                taken.insert(
                    key.to_string(),
                    serde_json::Value::Bool(crate::model::truthy(value)),
                );
            }
            "warmup_tokens" | "threads" | "max_tokens" | "min_tokens" => {
                let n = value
                    .parse::<u64>()
                    .map_err(|_| bad(format!("{key} must be a number: {value}")))?;
                taken.insert(key.to_string(), serde_json::json!(n));
            }
            other => return Err(bad(format!("key `{other}` is not one of {VISION_KEYS}"))),
        }
    }
    for required in ["exe", "mmproj", "model"] {
        if !taken.contains_key(required) {
            return Err(bad(format!(
                "requires `{required}` when a vision block is declared"
            )));
        }
    }
    if !taken.contains_key("gpu") {
        taken.insert("gpu".to_string(), serde_json::Value::Bool(false));
    }
    Ok(taken)
}

/// The profile keys `xolotl_profile_extra` may never carry — the xolotl voice
/// of the strata `PROFILE_EXTRA_DENIED` table (the same three families: the
/// harness-forced fields, the keys a typed knob already renders, and the
/// admission-affecting families only the witness may size, plus the
/// `before_load` hook pending its own ruling). Every other top-level profile
/// key the forked server or the shim reads merges through.
const PROFILE_EXTRA_DENIED: &[(&str, &str)] = &[
    ("host", "the bind is harness-forced (loopback, §5)"),
    ("port", "the port is harness-assigned from category config"),
    ("key", "request authentication is harness-owned"),
    ("api_key", "request authentication is harness-owned"),
    ("idle_unload_s", "residency is the harness scheduler's"),
    ("cors_origins", "CORS is harness-owned (loopback only)"),
    ("trusted_origins", "CORS is harness-owned (loopback only)"),
    ("allowed_hosts", "the host allow-list is harness-owned"),
    ("telemetry_read_key", "key material is harness-owned"),
    ("min_free_vram_mib", "the memory floor is the witness's"),
    ("slot_save_path", "slot custody paths are harness-owned"),
    ("vram_elastic", "the memory budget is the harness witness's"),
    (
        "vram_segment_mib",
        "the memory budget is the harness witness's",
    ),
    (
        "fit_max_tokens",
        "the memory budget is the harness witness's",
    ),
    (
        "lazy_load",
        "load timing is the harness scheduler's decision",
    ),
    (
        "replicas",
        "replicas multiply memory: admission must size them",
    ),
    (
        "replica_cpus",
        "replicas multiply memory: admission must size them",
    ),
    (
        "replica_load_tokens",
        "replicas multiply memory: admission must size them",
    ),
    (
        "split_serve",
        "split placement is the witness's (one-GPU §5)",
    ),
    (
        "split_skip_if_fits",
        "split placement is the witness's (one-GPU §5)",
    ),
    (
        "layer_split",
        "multi-GPU placement is the witness's (one-GPU §5)",
    ),
    (
        "gpu_order",
        "multi-GPU placement is the witness's (one-GPU §5)",
    ),
    (
        "hip_ordinal",
        "multi-GPU placement is the witness's (one-GPU §5)",
    ),
    (
        "before_load",
        "a profile hook that runs a program needs its own ruling",
    ),
    (
        "before_load_strict",
        "a profile hook that runs a program needs its own ruling",
    ),
    (
        "before_load_timeout_s",
        "a profile hook that runs a program needs its own ruling",
    ),
    (
        "before_load_ignore_failure",
        "a profile hook that runs a program needs its own ruling",
    ),
    ("model", "the weights are harness-owned custody"),
    ("config", "the profile path is harness-owned custody"),
    ("exe", "`xolotl_exe` renders this key"),
    ("args", "`xolotl_args` renders this key"),
    ("cwd", "`xolotl_cwd` renders this key"),
    ("tokenizer", "`xolotl_tokenizer` renders this key (custody)"),
    ("model_name", "the served id is harness-owned"),
    ("log", "the harness owns the log path"),
    ("lib_dirs", "`xolotl_lib_dirs` renders this key"),
    ("env", "`xolotl_env` renders this key"),
    ("gpu", "`xolotl_gpu` renders this key"),
    ("parallel", "`xolotl_parallel` renders this key"),
    ("vision", "`xolotl_vision` renders this key"),
    ("sampling", "`xolotl_sampling` renders this key"),
    ("aliases", "`xolotl_aliases` renders this key"),
    (
        "reasoning_budget_tokens",
        "`xolotl_reasoning_budget` renders this key",
    ),
];

/// The `xolotl_profile_extra` map: one operator JSON object merged into the
/// rendered profile AFTER the typed keys — the policy-filtered passthrough
/// door for the fork's unmodeled profile keys (thinking, structured_compact,
/// request_log, keep_cut_tool_calls, the conversation-cache block …). Refuses
/// every harness-owned or admission-affecting key by name, all at once, in
/// the `policy_field_claims` voice; merged path values still meet the
/// renderer-independent existence walk.
fn profile_extra(
    model: &ModelEntry,
) -> Result<serde_json::Map<String, serde_json::Value>, XolotlRowError> {
    let mut out = serde_json::Map::new();
    let Some(text) = knob(model, "xolotl_profile_extra") else {
        return Ok(out);
    };
    let bad = |reason: String| XolotlRowError::Bad {
        model: model.name.clone(),
        key: "xolotl_profile_extra".to_string(),
        reason,
    };
    let parsed: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| bad(format!("must hold one JSON object ({e}): {text}")))?;
    let map = parsed
        .as_object()
        .ok_or_else(|| bad(format!("must hold one JSON object: {text}")))?;
    let mut denied: Vec<String> = Vec::new();
    for (key, value) in map {
        match PROFILE_EXTRA_DENIED
            .iter()
            .find(|(name, _)| *name == key.as_str())
        {
            Some((_, owner)) => denied.push(format!("`{key}` \u{2014} {owner}")),
            None => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    if !denied.is_empty() {
        return Err(bad(format!(
            "carries harness-owned keys ({}); delete them from the object — \
             the typed knobs and forced fields own those decisions",
            denied.join("; ")
        )));
    }
    Ok(out)
}

/// Why a rendered profile is not loadable *after* its knobs validated: the
/// referenced-path walk and the harness-forced-field claim over the rendered
/// JSON (the strata `validate_profile_text` door, xolotl voice). The
/// top-level `env` map is exempt from the walk: its path values are judged
/// by the DSpark door alone, because the server may legitimately create a
/// cache directory (`STRATA_DENSE_REQUANT_CACHE`) that a fresh row does not
/// carry yet.
fn validate_rendered(json: &str) -> Result<(), XolotlRowError> {
    let root = serde_json::from_str::<serde_json::Value>(json)
        .expect("the renderer's own JSON parses; a failure here is a renderer bug");
    if let Some((key, missing)) = missing_rendered_path(&root) {
        return Err(XolotlRowError::MissingPath {
            model: String::new(),
            key,
            path: missing,
        });
    }
    let claims: Vec<(&'static str, &'static str)> = root.as_object().map_or(Vec::new(), |map| {
        PROFILE_POLICY_FIELDS
            .iter()
            .filter(|(name, _)| map.contains_key(*name))
            .copied()
            .collect()
    });
    if !claims.is_empty() {
        return Err(XolotlRowError::Claim {
            model: String::new(),
            claims,
        });
    }
    Ok(())
}

/// The first (key, path) the rendered profile references that the disk lacks.
/// Every string under every key is judged; the `log` key is exempt (its
/// target's existence is irrelevant to loading) and so is the top-level
/// `env` map (the DSpark door judges it). Relative paths stay exempt (the
/// engine resolves those against the tree root — the custody backlog rule).
/// The host separator (`;` on Windows, `:` on Linux) splits the engram
/// file-list shape inside one string, so each element is judged on its own.
fn missing_rendered_path(root: &serde_json::Value) -> Option<(String, String)> {
    walk_rendered(root, "profile")
}

fn walk_rendered(value: &serde_json::Value, key: &str) -> Option<(String, String)> {
    match value {
        serde_json::Value::Object(map) => map.iter().find_map(|(inner_key, inner)| {
            if inner_key == "log" || inner_key == "env" {
                None
            } else {
                walk_rendered(inner, inner_key)
            }
        }),
        serde_json::Value::Array(items) => items.iter().find_map(|inner| walk_rendered(inner, key)),
        serde_json::Value::String(text) => text
            .split(ENGRAM_SEPARATOR)
            .filter_map(super::strata::profile_path_candidate)
            .find(|path| std::fs::metadata(path).is_err())
            .map(|path| (key.to_string(), path)),
        _ => None,
    }
}

/// The single door the scan and the adapter share (the basalt `validate_row`
/// posture): every shape the launch requires is refused naming its key, and
/// every path the rendered profile names is existence-checked, so a doomed
/// launch refuses at rescan — before admission can evict a resident for it.
///
/// # Errors
///
/// [`XolotlRowError::Missing`] for an absent required key,
/// [`XolotlRowError::MissingPath`] for a path the disk lacks,
/// [`XolotlRowError::Bad`] for a value the key cannot carry,
/// [`XolotlRowError::Claim`] when the renderer would claim a harness-forced
/// field (defense in depth — it structurally cannot).
pub(crate) fn validate_row(model: &ModelEntry) -> Result<XolotlRow, XolotlRowError> {
    let missing = |field: &'static str| XolotlRowError::Missing {
        model: model.name.clone(),
        field,
    };
    let bad = |key: &str, reason: String| XolotlRowError::Bad {
        model: model.name.clone(),
        key: key.to_string(),
        reason,
    };
    let missing_path = |key: &str, path: &str| XolotlRowError::MissingPath {
        model: model.name.clone(),
        key: key.to_string(),
        path: path.to_string(),
    };
    // The interpreter is `argv[0]`: the one yaml string that chooses which
    // program the host starts, so it must be an absolute, existing FILE
    // (`.exe`-named on Windows — the strata shape floor; a batch shim renamed
    // to `.exe` still meets the launch there, and the floor refuses typos,
    // not malice).
    let Some(python) = knob(model, "xolotl_python") else {
        return Err(missing("xolotl_python"));
    };
    if !super::strata::is_absolute_shape(python) {
        return Err(bad(
            "xolotl_python",
            format!("must be an absolute path (drive-letter or UNC): {python}"),
        ));
    }
    let python_path = PathBuf::from(python);
    match std::fs::metadata(&python_path) {
        Err(_) => return Err(missing_path("xolotl_python", python)),
        Ok(meta) if !meta.is_file() => {
            return Err(bad(
                "xolotl_python",
                format!("is not a file (a directory cannot be the launched program): {python}"),
            ))
        }
        Ok(_) => {}
    }
    #[cfg(windows)]
    if !python_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err(bad(
            "xolotl_python",
            format!("must be an existing .exe file: {python}"),
        ));
    }
    // The launcher is the python script `-I` runs; the cwd is the tree root
    // the relative `-m` cache workaround resolves against (required here,
    // no harness default — ruling 5).
    let Some(launcher) = knob(model, "xolotl_launcher") else {
        return Err(missing("xolotl_launcher"));
    };
    if !super::strata::is_absolute_shape(launcher) {
        return Err(bad(
            "xolotl_launcher",
            format!("must be an absolute path (drive-letter or UNC): {launcher}"),
        ));
    }
    if !Path::new(launcher).is_file() {
        return Err(missing_path("xolotl_launcher", launcher));
    }
    let Some(cwd) = knob(model, "xolotl_cwd") else {
        return Err(missing("xolotl_cwd"));
    };
    if !super::strata::is_absolute_shape(cwd) {
        return Err(bad(
            "xolotl_cwd",
            format!("must be an absolute path (drive-letter or UNC): {cwd}"),
        ));
    }
    let cwd_path = PathBuf::from(cwd);
    if !cwd_path.is_dir() {
        return Err(bad(
            "xolotl_cwd",
            format!("is not an existing directory: {cwd}"),
        ));
    }
    // The engine binary and tokenizer the profile names (existence here so
    // the refusal names the yaml key; the walk re-checks the rendered copy).
    // The tested 256K config passes the tokenizer as a DIRECTORY
    // (vocab.json, merges.txt, chat_template.jinja), so either kind is valid
    // there; the executable must be a file.
    for key in ["xolotl_exe", "xolotl_tokenizer"] {
        let Some(value) = knob(model, key) else {
            return Err(missing(key));
        };
        if !super::strata::is_absolute_shape(value) {
            return Err(bad(
                key,
                format!("must be an absolute path (drive-letter or UNC): {value}"),
            ));
        }
        let path = Path::new(value);
        let exists = if key == "xolotl_tokenizer" {
            path.is_file() || path.is_dir()
        } else {
            path.is_file()
        };
        if !exists {
            return Err(missing_path(key, value));
        }
    }
    let Some(model_name) = model
        .serve_model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Err(missing("serve_model"));
    };
    // The 256K load streams for minutes: the global 120 s default would kill
    // a healthy load, so the row states its own number at or above the floor.
    match model.load_timeout_s {
        Some(seconds) if seconds >= XOLOTL_MIN_LOAD_TIMEOUT_S => {}
        Some(seconds) => {
            return Err(bad(
                "load_timeout",
                format!(
                "must be {XOLOTL_MIN_LOAD_TIMEOUT_S} seconds or more for a xolotl load: {seconds}"
            ),
            ))
        }
        None => return Err(missing("load_timeout")),
    }
    let mut args = match knob(model, "xolotl_args") {
        Some(line) => args_tokens(model, line, &cwd_path)?,
        None => Vec::new(),
    };
    let vision = match knob(model, "xolotl_vision") {
        Some(line) => Some(vision_block(model, line)?),
        None => None,
    };
    if vision.is_some() {
        if let Some(banned) = args.iter().find(|token| {
            // The ban reads the option NAME: the eq form is spawn truth too
            // (ruling 10[1]), so `--vision=off` must not ride alongside the
            // block the way `--host=` cannot ride the forced bind.
            let name = token
                .split_once('=')
                .map_or(token.as_str(), |(name, _)| name);
            "--vision".starts_with(name)
        }) {
            return Err(bad(
                "xolotl_vision",
                format!(
                    "the block already switches vision on; drop the `{banned}` \
                     token from `xolotl_args`"
                ),
            ));
        }
        args.push("--vision".to_string());
    }
    let env = match knob(model, "xolotl_env") {
        Some(line) => env_map(model, line)?,
        None => serde_json::Map::new(),
    };
    if let Some((key, path)) = missing_dspark_path(&env) {
        return Err(missing_path(key, &path));
    }
    let lib_dirs = match knob(model, "xolotl_lib_dirs") {
        Some(line) => path_list(model, "xolotl_lib_dirs", line)?,
        None => Vec::new(),
    };
    let gpu = match knob(model, "xolotl_gpu") {
        Some(value) => Some(unsigned(model, "xolotl_gpu", value)?),
        None => None,
    };
    let sampling = match knob(model, "xolotl_sampling") {
        Some(line) => Some(sampling_map(model, line)?),
        None => None,
    };
    let aliases = match knob(model, "xolotl_aliases") {
        Some(line) => {
            let mut names = Vec::new();
            for entry in line.split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    return Err(bad(
                        "xolotl_aliases",
                        format!("entries must be non-empty, comma-separated names: {line}"),
                    ));
                }
                names.push(serde_json::Value::String(entry.to_string()));
            }
            Some(names)
        }
        None => None,
    };
    let reasoning = match knob(model, "xolotl_reasoning_budget") {
        Some(value) => Some(unsigned(model, "xolotl_reasoning_budget", value)?),
        None => None,
    };
    let parallel = match knob(model, "xolotl_parallel") {
        Some(value) => {
            let n = unsigned(model, "xolotl_parallel", value)?;
            if !(PARALLEL_MIN..=PARALLEL_MAX).contains(&n) {
                return Err(bad(
                    "xolotl_parallel",
                    format!("must be a whole number {PARALLEL_MIN}..={PARALLEL_MAX}: {value}"),
                ));
            }
            Some(n)
        }
        None => None,
    };
    // The render is the launch (row 19 idiom): `host`, `port` and key fields
    // are never emitted — the bind and the port ride the forced argv.
    let mut profile = serde_json::Map::new();
    profile.insert(
        "exe".to_string(),
        serde_json::Value::String(knob(model, "xolotl_exe").unwrap_or_default().to_string()),
    );
    profile.insert(
        "args".to_string(),
        serde_json::Value::Array(
            args.into_iter()
                .map(serde_json::Value::String)
                .collect::<Vec<_>>(),
        ),
    );
    profile.insert(
        "cwd".to_string(),
        serde_json::Value::String(cwd_path.to_string_lossy().into_owned()),
    );
    profile.insert(
        "tokenizer".to_string(),
        serde_json::Value::String(
            knob(model, "xolotl_tokenizer")
                .unwrap_or_default()
                .to_string(),
        ),
    );
    profile.insert(
        "model_name".to_string(),
        serde_json::Value::String(model_name.to_string()),
    );
    profile.insert(
        "log".to_string(),
        serde_json::Value::String(
            crate::process::rendered_profiles_dir("xolotl")
                .join(format!(
                    "{}.log",
                    crate::process::sanitize_slug(&model.name)
                ))
                .to_string_lossy()
                .into_owned(),
        ),
    );
    if !lib_dirs.is_empty() {
        profile.insert(
            "lib_dirs".to_string(),
            serde_json::Value::Array(
                lib_dirs
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if !env.is_empty() {
        profile.insert("env".to_string(), serde_json::Value::Object(env));
    }
    if let Some(gpu) = gpu {
        profile.insert("gpu".to_string(), serde_json::json!(gpu));
    }
    if let Some(sampling) = sampling {
        profile.insert("sampling".to_string(), serde_json::Value::Object(sampling));
    }
    if let Some(aliases) = aliases {
        profile.insert("aliases".to_string(), serde_json::Value::Array(aliases));
    }
    if let Some(budget) = reasoning {
        profile.insert(
            "reasoning_budget_tokens".to_string(),
            serde_json::json!(budget),
        );
    }
    if let Some(vision) = vision {
        profile.insert("vision".to_string(), serde_json::Value::Object(vision));
    }
    if let Some(parallel) = parallel {
        profile.insert("parallel".to_string(), serde_json::json!(parallel));
    }
    // The passthrough door merges LAST: it can never overwrite a typed key
    // (every typed name is denied by name).
    for (key, value) in profile_extra(model)? {
        profile.insert(key, value);
    }
    // The content address is the version: one deterministic render per yaml
    // (serde_json's map order guarantees cross-build stability).
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(profile)).map_err(|e| {
        bad(
            "profile",
            format!("the rendered profile would not serialize: {e}"),
        )
    })?;
    // Re-runs the renderer-independent doors over the exact bytes (the
    // render walk + claim); this module's errors carry no model name yet,
    // so stamp it here before handing the refusal over.
    validate_rendered(&text).map_err(|problem| match problem {
        XolotlRowError::MissingPath { key, path, .. } => XolotlRowError::MissingPath {
            model: model.name.clone(),
            key,
            path,
        },
        XolotlRowError::Claim { claims, .. } => XolotlRowError::Claim {
            model: model.name.clone(),
            claims,
        },
        other => other,
    })?;
    Ok(XolotlRow {
        python: python_path,
        cwd: cwd_path,
        text,
    })
}

/// Xolotl adapter: `python -I <launcher>` plus the `xolotl` row's forced
/// tokens, `/health` 200 **and** `loaded != false` (the strata contract).
#[derive(Debug, Clone, Default)]
pub struct XolotlAdapter;

impl XolotlAdapter {
    /// The argv exactly as the ruling spells it: interpreter, `-I`, the
    /// launcher, then the forced tokens from the policy row (engine tag,
    /// loopback bind, rendered-profile position, harness-assigned port).
    /// `-I` isolates the interpreter from environment-import noise and lives
    /// here because a [`super::policy::ForcedField`] always carries a value.
    fn argv(
        &self,
        row: &super::policy::EnginePolicy,
        python: &Path,
        launcher: &str,
        config: &str,
        port: u16,
    ) -> Vec<String> {
        let mut argv = vec![
            python.to_string_lossy().into_owned(),
            "-I".to_string(),
            launcher.to_string(),
        ];
        argv.extend(super::policy::render_forced(row, port, Some(config)));
        argv
    }
}

impl EngineAdapter for XolotlAdapter {
    fn engine_id(&self) -> &str {
        "xolotl"
    }

    fn spec(&self, model: &ModelEntry, port: u16) -> Result<LaunchSpec, EngineError> {
        let row = super::policy::engine_policy("xolotl")
            .expect("the compiled-in table ships a xolotl row");
        let v = validate_row(model).map_err(XolotlRowError::into_engine_error)?;
        // Location custody (the row's executed-path roots, Fork C):
        // while the cells are open it admits everything (ruling 5), and it
        // will enforce unchanged once a later copy-in slice populates them
        // (the strata posture).
        super::policy::admit_custody_path(
            row,
            super::policy::CustodyClass::Executed,
            &model.name,
            "xolotl_python",
            &v.python.to_string_lossy(),
        )?;
        super::policy::admit_custody_path(
            row,
            super::policy::CustodyClass::Executed,
            &model.name,
            "xolotl_cwd",
            &v.cwd.to_string_lossy(),
        )?;
        // The runtime stages these bytes under `.simpleharn/xolotl-profiles`
        // and rewrites `--config` to exactly that staged path; no operator
        // file ever reaches the child.
        let staged_name = crate::process::staged_profile_name(&model.name, &v.text);
        let launcher = knob(model, "xolotl_launcher")
            .expect("validate_row made the launcher a present non-empty key");
        Ok(LaunchSpec {
            argv: self.argv(row, &v.python, launcher, &staged_name, port),
            cwd: v.cwd,
            env: BTreeMap::new(),
            log_name: model.name.clone(),
            readiness: ReadinessSpec::Then(
                Box::new(ReadinessSpec::Http {
                    path: "/health".to_string(),
                    expect_status: 200,
                }),
                Box::new(ReadinessSpec::HttpFlag {
                    path: "/health".to_string(),
                    expect_status: 200,
                    field: "loaded".to_string(),
                }),
            ),
            container: None,
            script: None,
            rendered_profile: Some(RenderedProfile {
                engine: "xolotl",
                text: v.text,
            }),
            // The xolotl launch rides the custody sidecar exactly as strata
            // does: the shim spawns the engine as a child, so a
            // crashed run's orphan tree is provably ours to reap at the next
            // boot purely from the harness's own record.
            custody: Some("xolotl".to_string()),
        })
    }
}

/// The context a xolotl row actually serves: the figure riding the engine's
/// own argv in the comma-separated `xolotl_args` list — the token after
/// `--ctx` or the `--ctx=262144` suffix (the tested 256K line spells
/// `--ctx 262144`). Either spelling must carry an all-ASCII-digits figure
/// (the strata `effective_context` rules: a flag in the value slot and a
/// malformed line answer `None`; the row `context:` key is llama's knob and
/// never a fallback).
///
/// # Arguments
///
/// * `raw` - the row's raw yaml keys (its `xolotl_args` line, if any).
///
/// # Returns
///
/// The spelled context figure, `None` when the argv names none the engine
/// would serve.
#[must_use]
pub fn effective_context(raw: &BTreeMap<String, String>) -> Option<String> {
    let line = raw
        .get("xolotl_args")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())?;
    let tokens = super::strata::well_formed_tokens(line)?;
    let mut tokens = tokens
        .iter()
        .flat_map(|entry| entry.split_whitespace())
        .peekable();
    while let Some(token) = tokens.next() {
        if let Some(value) = token.strip_prefix("--ctx=") {
            return figure(value).map(str::to_string);
        }
        if token == "--ctx" {
            return figure(tokens.next()?).map(str::to_string);
        }
    }
    None
}

/// The all-ASCII-digits shape of a context figure.
fn figure(text: &str) -> Option<&str> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())).then_some(text)
}

/// Reads the xolotl `GET /metrics` snapshot: the shim serves the strata
/// `engine` block contract (ruling: readiness and measurement ride the strata
/// path), so the parse IS the strata parse, named at the routing door so
/// [`crate::scheduler::occupant`] dispatches on the engine, not on a default.
pub(crate) fn measurement_from_metrics(
    body: &serde_json::Value,
) -> Option<crate::engine::EngineMeasurement> {
    super::strata::measurement_from_metrics(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CategoryId;

    fn raw(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn entry(pairs: &[(&str, &str)]) -> ModelEntry {
        ModelEntry {
            name: "xolotl-test".to_string(),
            engine: "xolotl".to_string(),
            category: CategoryId::Work,
            vram_gb: None,
            ram_gb: None,
            gguf_size_gb: None,
            cost: crate::model::CostClass::Greedy,
            vram_gate: crate::model::VramGate::Whole,
            load_timeout_s: Some(600),
            min_turn_s: None,
            measured_gb: None,
            measured_min_gb: None,
            measured_offload: None,
            measured_attribution: None,
            measured_profile: None,
            measured_ram_gb: None,
            measured_ram_min_gb: None,
            fit_gb: None,
            model_ref: Some("xolotl://test".to_string()),
            serve_model: Some("ds41-flash".to_string()),
            file: None,
            raw: raw(pairs),
        }
    }

    fn real_file(dir: &Path, tag: &str, extension: &str) -> PathBuf {
        let path = dir.join(format!("{tag}.{extension}"));
        std::fs::write(&path, b"{}").expect("temp file");
        path
    }

    fn path_text(path: &Path) -> &str {
        path.to_str().expect("utf-8 fixture path")
    }

    /// A host-absolute path the disk cannot hold, so the absolute-shape floor
    /// passes and the existence door refuses the same way on every host (the
    /// Windows spelling keeps the original `C:\xolotl\...` fixture shape).
    fn absent_path(parts: &[&str]) -> String {
        let (root, sep) = if cfg!(windows) {
            (r"C:\xolotl", '\\')
        } else {
            ("/xolotl-absent", '/')
        };
        let mut out = root.to_string();
        for part in parts {
            out.push(sep);
            out.push_str(part);
        }
        out
    }

    /// The minimal valid row: an existing interpreter, launcher, engine and
    /// tokenizer inside one temp tree root, and a relative `-m` that
    /// resolves against that root (the tested cache-workaround shape).
    fn fixture_pairs() -> Vec<(String, String)> {
        let dir = crate::testutil::temp_dir("xolotl");
        let python = real_file(&dir, "python", "exe");
        let launcher = dir.join("serve_ds41.py");
        std::fs::write(&launcher, b"").expect("launcher fixture");
        let exe = real_file(&dir, "ds41_generate", "exe");
        let tokenizer = real_file(&dir, "tokenizer", "bin");
        std::fs::create_dir_all(dir.join("model")).expect("model subdir");
        std::fs::write(dir.join("model").join("V41.gguf"), b"g").expect("weights fixture");
        vec![
            ("xolotl_python".to_string(), path_text(&python).to_string()),
            (
                "xolotl_launcher".to_string(),
                path_text(&launcher).to_string(),
            ),
            ("xolotl_cwd".to_string(), path_text(&dir).to_string()),
            ("xolotl_exe".to_string(), path_text(&exe).to_string()),
            (
                "xolotl_tokenizer".to_string(),
                path_text(&tokenizer).to_string(),
            ),
            (
                "xolotl_args".to_string(),
                "--backend cuda, --ctx 262144, --kv-q8, -m model/V41.gguf".to_string(),
            ),
        ]
    }

    fn spec_of(extra: &[(&str, &str)]) -> LaunchSpec {
        let base = fixture_pairs();
        let mut pairs: Vec<(&str, &str)> =
            base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        pairs.extend_from_slice(extra);
        XolotlAdapter
            .spec(&entry(&pairs), 18109)
            .expect("valid xolotl spec")
    }

    fn spec_err_of(extra: &[(&str, &str)]) -> EngineError {
        let base = fixture_pairs();
        let mut pairs: Vec<(&str, &str)> =
            base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        pairs.extend_from_slice(extra);
        XolotlAdapter
            .spec(&entry(&pairs), 18109)
            .expect_err("must refuse")
    }

    /// The shim's full vision contract renders, the block appends exactly one
    /// `--vision` (serve_ds41.py:299-327), and `gpu` defaults OFF on this line.
    /// Falsifier: drop the default insert and the encoder lands on-card.
    #[test]
    fn vision_block_renders_and_appends_exactly_one_vision_flag() {
        let dir = crate::testutil::temp_dir("xolotl-vision");
        let vex = real_file(&dir, "v-exe", "exe");
        let vpython = real_file(&dir, "v-python", "exe");
        let vmmproj = real_file(&dir, "v-mmproj", "gguf");
        let vmodel = real_file(&dir, "v-model", "gguf");
        let vref = dir.join("reference");
        std::fs::create_dir_all(&vref).expect("reference dir");
        let vweights = real_file(&dir, "v-weights", "safetensors");
        let spec = spec_of(&[(
            "xolotl_vision",
            &format!(
                "exe={} python={} mmproj={} model={} reference_dir={} weights={} \
                 dtype=BF16 device=CUDA offload=1 tf32=1 warmup_tokens=4096 \
                 threads=8 max_tokens=4096 min_tokens=1",
                path_text(&vex),
                path_text(&vpython),
                path_text(&vmmproj),
                path_text(&vmodel),
                path_text(&vref),
                path_text(&vweights)
            ),
        )]);
        let profile: serde_json::Value =
            serde_json::from_str(&spec.rendered_profile.as_ref().expect("rendered").text)
                .expect("json");
        assert_eq!(profile["vision"]["python"], path_text(&vpython));
        assert_eq!(profile["vision"]["dtype"], "bf16");
        assert_eq!(profile["vision"]["device"], "cuda");
        assert_eq!(profile["vision"]["offload"], serde_json::json!(true));
        assert_eq!(profile["vision"]["tf32"], serde_json::json!(true));
        assert_eq!(profile["vision"]["warmup_tokens"], serde_json::json!(4096));
        assert_eq!(profile["vision"]["threads"], serde_json::json!(8));
        assert_eq!(profile["vision"]["max_tokens"], serde_json::json!(4096));
        assert_eq!(profile["vision"]["min_tokens"], serde_json::json!(1));
        assert_eq!(
            profile["vision"]["gpu"],
            serde_json::json!(false),
            "the tested line keeps the encoder off-card unless the row opts in"
        );
        let args = profile["args"].as_array().expect("args");
        assert_eq!(
            args.iter()
                .filter(|token| token.as_str() == Some("--vision"))
                .count(),
            1,
            "exactly one --vision rides the args"
        );
        assert_eq!(args.last().and_then(|t| t.as_str()), Some("--vision"));
        let on_card = spec_of(&[(
            "xolotl_vision",
            &format!(
                "exe={} mmproj={} model={} gpu=1",
                path_text(&vex),
                path_text(&vmmproj),
                path_text(&vmodel)
            ),
        )]);
        let on_card: serde_json::Value =
            serde_json::from_str(&on_card.rendered_profile.as_ref().expect("rendered").text)
                .expect("json");
        assert_eq!(on_card["vision"]["gpu"], serde_json::json!(true));
    }

    /// The row spells the vision ONCE: a `--vision` token (bare or eq form) in
    /// `xolotl_args` alongside the block refuses naming the block.
    #[test]
    fn vision_block_refuses_a_second_vision_token_in_args() {
        let dir = crate::testutil::temp_dir("xolotl-vision-deny");
        let vex = real_file(&dir, "v-exe", "exe");
        let vmmproj = real_file(&dir, "v-mmproj", "gguf");
        let vmodel = real_file(&dir, "v-model", "gguf");
        for banned in ["--vision", "--vision=off"] {
            let err = spec_err_of(&[
                (
                    "xolotl_vision",
                    &format!(
                        "exe={} mmproj={} model={}",
                        path_text(&vex),
                        path_text(&vmmproj),
                        path_text(&vmodel)
                    ),
                ),
                (
                    "xolotl_args",
                    &format!("--backend cuda, {banned}, -m model/V41.gguf"),
                ),
            ]);
            let EngineError::Rejected { reason, .. } = &err else {
                panic!("expected Rejected, got {err:?}");
            };
            assert!(
                reason.contains("xolotl_vision") && reason.contains(banned),
                "`{banned}` refused blind: {reason}"
            );
        }
    }

    /// The shim caps this line's pool at 16 (`ds41_parallel_args`); a row
    /// outside the range refuses instead of rendering a figure the launch
    /// would shrink.
    #[test]
    fn xolotl_parallel_renders_within_the_shim_ceiling_and_refuses_beyond() {
        let spec = spec_of(&[("xolotl_parallel", "16")]);
        let profile: serde_json::Value =
            serde_json::from_str(&spec.rendered_profile.as_ref().expect("rendered").text)
                .expect("json");
        assert_eq!(profile["parallel"], serde_json::json!(16));
        for out_of_range in ["0", "17"] {
            let err = spec_err_of(&[("xolotl_parallel", out_of_range)]);
            let EngineError::Rejected { reason, .. } = &err else {
                panic!("expected Rejected, got {err:?}");
            };
            assert!(
                reason.contains("xolotl_parallel") && reason.contains("1..=16"),
                "`{out_of_range}` refused blind: {reason}"
            );
        }
    }

    /// The fork's unmodeled keys merge; harness-owned, admission-sized and
    /// hook keys refuse by name (the strata `profile_extra` posture).
    #[test]
    fn profile_extra_merges_fork_keys_and_refuses_owned_ones_by_name() {
        let spec = spec_of(&[(
            "xolotl_profile_extra",
            r#"{"thinking": true, "hide_tokens": false,
                "conversation_cache": {"slots": 4}}"#,
        )]);
        let profile: serde_json::Value =
            serde_json::from_str(&spec.rendered_profile.as_ref().expect("rendered").text)
                .expect("json");
        assert_eq!(profile["thinking"], serde_json::json!(true));
        assert_eq!(profile["hide_tokens"], serde_json::json!(false));
        assert_eq!(profile["conversation_cache"]["slots"], serde_json::json!(4));
        for (text, needle) in [
            (r#"{"host": "0.0.0.0"}"#, "host"),
            (r#"{"replicas": 2}"#, "replicas"),
            (r#"{"layer_split": "auto"}"#, "layer_split"),
            (r#"{"before_load": "evil.exe"}"#, "before_load"),
            (r#"{"slot_save_path": "C:/slot.bin"}"#, "slot_save_path"),
            (r#"{"vram_elastic": true}"#, "vram_elastic"),
            (r#"{"vram_segment_mib": 64}"#, "vram_segment_mib"),
            (r#"{"fit_max_tokens": 1024}"#, "fit_max_tokens"),
            (r#"{"lazy_load": true}"#, "lazy_load"),
            (r#"{"vision": {}}"#, "xolotl_vision"),
            ("nope", "xolotl_profile_extra"),
        ] {
            let err = spec_err_of(&[("xolotl_profile_extra", text)]);
            let EngineError::Rejected { reason, .. } = &err else {
                panic!("expected Rejected, got {err:?}");
            };
            assert!(reason.contains(needle), "`{text}` refused blind: {reason}");
        }
    }

    /// The exact forced argv shape (ruling: the launch IS
    /// `python -I <launcher> --engine strata --host 127.0.0.1 --config
    /// <staged> --port <slot>` and nothing else). Falsifier: drop `-I`,
    /// swap the engine tag or reorder the forced tail and this pins red.
    #[test]
    fn forced_argv_is_pinned_exactly() {
        let spec = spec_of(&[]);
        let staged = crate::process::staged_profile_name(
            "xolotl-test",
            spec.rendered_profile
                .as_ref()
                .expect("rendered")
                .text
                .as_str(),
        );
        assert_eq!(
            spec.argv,
            vec![
                spec.argv[0].clone(),
                "-I".to_string(),
                fixture_launcher(&spec),
                "--engine".to_string(),
                "strata".to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string(),
                "--config".to_string(),
                staged,
                "--port".to_string(),
                "18109".to_string(),
            ]
        );
        assert!(spec.argv[0].ends_with("python.exe"));
    }

    /// Pulls the launcher out of a rendered spec's argv (fixture echo, keeps
    /// the argv pin above readable).
    fn fixture_launcher(spec: &LaunchSpec) -> String {
        spec.argv[2].clone()
    }

    /// The render equals the row and carries no harness-forced field
    /// (22[1] clean by construction); `--config` names the content-addressed
    /// staged name the runtime rewrites to the staged bytes.
    #[test]
    fn rendered_profile_equals_the_row_and_carries_no_forced_fields() {
        let spec = spec_of(&[
            ("xolotl_aliases", "ds41, ds41-flash"),
            ("xolotl_reasoning_budget", "32768"),
            ("xolotl_gpu", "0"),
        ]);
        let profile: serde_json::Value =
            serde_json::from_str(&spec.rendered_profile.as_ref().expect("rendered").text)
                .expect("renders JSON");
        for banned in ["host", "port", "key", "api_key"] {
            assert!(
                profile.get(banned).is_none(),
                "the render must never emit `{banned}`"
            );
        }
        assert_eq!(profile["model_name"], "ds41-flash");
        assert_eq!(profile["gpu"], 0);
        assert_eq!(profile["reasoning_budget_tokens"], 32768);
        assert_eq!(profile["aliases"][0], "ds41");
        assert_eq!(profile["args"][0], "--backend");
        assert!(profile["log"]
            .as_str()
            .expect("log")
            .contains(".simpleharn"));
        let staged = crate::process::staged_profile_name(
            "xolotl-test",
            &spec.rendered_profile.as_ref().expect("rendered").text,
        );
        let config_at = spec
            .argv
            .iter()
            .position(|t| t == "--config")
            .expect("--config");
        assert_eq!(
            spec.argv[config_at + 1],
            staged,
            "the staged name rides --config"
        );
        assert!(
            staged.ends_with(".json") && staged.contains("xolotl-test"),
            "the staged name is slug-plus-content: {staged}"
        );
    }

    #[test]
    fn missing_required_keys_name_the_field() {
        for drop in [
            "xolotl_python",
            "xolotl_launcher",
            "xolotl_cwd",
            "xolotl_exe",
            "xolotl_tokenizer",
        ] {
            let base = fixture_pairs();
            let pairs: Vec<(&str, &str)> = base
                .iter()
                .filter(|(k, _)| k != drop)
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let err = XolotlAdapter
                .spec(&entry(&pairs), 18109)
                .expect_err("`{drop}` alone missing must refuse");
            let EngineError::MissingField { field, .. } = err else {
                panic!("a dropped required key is MissingField, got {err:?}");
            };
            assert_eq!(field, drop);
        }
        let mut no_serve = fixture_entry();
        no_serve.serve_model = None;
        assert_eq!(
            XolotlAdapter
                .spec(&no_serve, 18109)
                .expect_err("serve_model is required"),
            EngineError::MissingField {
                model: "xolotl-test".to_string(),
                field: "serve_model".to_string()
            }
        );
        let mut no_timeout = fixture_entry();
        no_timeout.load_timeout_s = None;
        assert_eq!(
            XolotlAdapter
                .spec(&no_timeout, 18109)
                .expect_err("timeout is required"),
            EngineError::MissingField {
                model: "xolotl-test".to_string(),
                field: "load_timeout".to_string()
            }
        );
    }

    /// The validated fixture row, as an entry (for tests that modify one
    /// typed field instead of a raw knob).
    fn fixture_entry() -> ModelEntry {
        let base = fixture_pairs();
        let pairs: Vec<(&str, &str)> = base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        entry(&pairs)
    }

    #[test]
    fn load_timeout_below_the_floor_is_refused_naming_the_floor() {
        let mut e = fixture_entry();
        e.load_timeout_s = Some(300);
        let err = XolotlAdapter
            .spec(&e, 18109)
            .expect_err("300 s is below the floor");
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a shaped timeout is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("load_timeout") && reason.contains("600"),
            "{reason}"
        );
    }

    #[test]
    fn paths_that_are_not_files_refuse_naming_key_and_path() {
        let err = spec_err_of(&[(
            "xolotl_exe",
            absent_path(&["ghost", "ds41_generate.exe"]).as_str(),
        )]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing engine exe is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("xolotl_exe") && reason.contains("ds41_generate.exe"),
            "{reason}"
        );
        let err = spec_err_of(&[("xolotl_python", &absent_path(&["nope", "python.exe"]))]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing interpreter is Rejected, got {err:?}");
        };
        assert!(reason.contains("xolotl_python"), "{reason}");
        let dir = crate::testutil::temp_dir("xolotl-dir-as-python");
        let err = spec_err_of(&[("xolotl_python", path_text(&dir))]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a directory interpreter is Rejected, got {err:?}");
        };
        assert!(reason.contains("xolotl_python"), "{reason}");
        // The `.exe` name rule is the Windows floor: on a unix host any
        // existing regular file is an acceptable interpreter, so there is
        // no non-.exe shim leg to refuse here.
        #[cfg(windows)]
        {
            let dir = crate::testutil::temp_dir("xolotl-bat-as-python");
            let shim = dir.join("shim.bat");
            std::fs::write(&shim, b"@echo off").expect("bat fixture");
            let err = spec_err_of(&[("xolotl_python", path_text(&shim))]);
            let EngineError::Rejected { reason, .. } = err else {
                panic!("a non-.exe interpreter is Rejected, got {err:?}");
            };
            assert!(
                reason.contains(".exe") && reason.contains("xolotl_python"),
                "{reason}"
            );
        }
        let err = spec_err_of(&[("xolotl_cwd", &absent_path(&["no-such-root"]))]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing cwd is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("xolotl_cwd") && reason.contains("directory"),
            "{reason}"
        );
    }

    #[test]
    fn relative_paths_must_be_absolute_naming_the_key() {
        for key in [
            "xolotl_python",
            "xolotl_launcher",
            "xolotl_cwd",
            "xolotl_exe",
        ] {
            let err = spec_err_of(&[(key, "relative\\thing")]);
            let EngineError::Rejected { reason, .. } = err else {
                panic!("a relative `{key}` is Rejected, got {err:?}");
            };
            assert!(
                reason.contains(key) && reason.contains("absolute"),
                "{reason}"
            );
        }
    }

    /// The relative `-m` resolves against `xolotl_cwd` (the cache
    /// workaround); a miss refuses naming the flag and the resolved path.
    #[test]
    fn a_relative_model_that_misses_refuses_by_path() {
        let err = spec_err_of(&[("xolotl_args", "--backend cuda, -m model\\gone.gguf")]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing model is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("-m") && reason.contains("gone.gguf"),
            "{reason}"
        );
    }

    #[test]
    fn engram_entries_and_rank_must_exist_naming_the_flag() {
        let dir = crate::testutil::temp_dir("xolotl-engram");
        std::fs::create_dir_all(dir.join("model")).expect("model dir");
        let l1 = dir.join("model").join("engram-layers.1");
        std::fs::write(&l1, b"1").expect("l1 fixture");
        // The first entry exists, the second does not: the refusal names the
        // `--engram` flag and the missing half.
        let err = spec_err_of(&[(
            "xolotl_args",
            &format!(
                "--engram {}{}{}",
                path_text(&l1),
                ENGRAM_SEPARATOR,
                path_text(&dir.join("model").join("engram-layers.14"))
            ),
        )]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing engram entry is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("--engram") && reason.contains("engram-layers.14"),
            "{reason}"
        );
        let err = spec_err_of(&[(
            "xolotl_args",
            &format!("--rank {}", absent_path(&["model", "rank.bin"])),
        )]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing rank sidecar is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("--rank") && reason.contains("rank.bin"),
            "{reason}"
        );
        // Both halves present: the row renders.
        let l14 = dir.join("model").join("engram-layers.14");
        std::fs::write(&l14, b"14").expect("l14 fixture");
        let rank = dir.join("model").join("rank.bin");
        std::fs::write(&rank, b"r").expect("rank fixture");
        let ok = spec_of(&[(
            "xolotl_args",
            &format!(
                "--engram {}{}{}, --rank {}",
                path_text(&l1),
                ENGRAM_SEPARATOR,
                path_text(&l14),
                path_text(&rank)
            ),
        )]);
        assert!(ok.argv.len() >= 10);
    }

    #[test]
    fn host_and_port_in_the_args_are_refused() {
        for line in ["--host 0.0.0.0, --native", "--port=9999, --native"] {
            let err = spec_err_of(&[("xolotl_args", line)]);
            let EngineError::Rejected { reason, .. } = err else {
                panic!("a forced flag spelled by the row is Rejected, got {err:?}");
            };
            assert!(
                reason.contains("harness-forced argv") && reason.contains("xolotl_args"),
                "{reason}"
            );
        }
    }

    /// Ruling 6: a DSpark sidecar the row names and the disk lacks refuses at
    /// the door, by path; absence means no DSpark; the keep file's `:64`
    /// suffix is stripped before the existence check.
    #[test]
    fn a_named_dspark_the_disk_lacks_refuses_by_path() {
        let err = spec_err_of(&[(
            "xolotl_env",
            &format!(
                "STRATA_SERVE_DSPARK={}",
                absent_path(&["dspark", "gone.Q2_K.gguf"])
            ),
        )]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("a missing DSpark sidecar is Rejected, got {err:?}");
        };
        assert!(
            reason.contains("STRATA_SERVE_DSPARK") && reason.contains("gone.Q2_K.gguf"),
            "{reason}"
        );
        let dir = crate::testutil::temp_dir("xolotl-dspark");
        let keep = dir.join("expert_counts.txt");
        std::fs::write(&keep, b"c").expect("keep fixture");
        // Keep present with its `:64` suffix, sidecar absent: the sidecar
        // refuses by name.
        let err = spec_err_of(&[(
            "xolotl_env",
            &format!(
                "STRATA_SERVE_DSPARK={} STRATA_DSPARK_KEEP={}:64",
                absent_path(&["gone.gguf"]),
                path_text(&keep)
            ),
        )]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("the missing sidecar still refuses, got {err:?}");
        };
        assert!(reason.contains("STRATA_SERVE_DSPARK"), "{reason}");
        // Sidecar present, keep missing: the keep refuses with the stripped
        // path (no `:64` in the refusal).
        let sidecar = dir.join("mtp.Q2_K.gguf");
        std::fs::write(&sidecar, b"m").expect("sidecar fixture");
        let err = spec_err_of(&[(
            "xolotl_env",
            &format!(
                "STRATA_SERVE_DSPARK={} STRATA_DSPARK_KEEP={}:64",
                path_text(&sidecar),
                path_text(&dir.join("gone.txt"))
            ),
        )]);
        let EngineError::Rejected { reason, .. } = err else {
            panic!("the missing keep refuses, got {err:?}");
        };
        assert!(
            reason.contains("STRATA_DSPARK_KEEP")
                && reason.contains("gone.txt")
                && !reason.contains("gone.txt:64"),
            "{reason}"
        );
    }

    #[test]
    fn env_outside_the_allow_list_is_refused_and_the_curated_var_passes() {
        for line in [
            "LD_PRELOAD=/tmp/evil.so",
            "STRATA_lower=1",
            "STRATA=1",
            "GGML_NO_BACKTRACE=nope, comma",
            "STRATA_TEXT_LOG=a b",
            "NOEQUALS",
        ] {
            let err = spec_err_of(&[("xolotl_env", line)]);
            let EngineError::Rejected { reason, .. } = err else {
                panic!("`{line}` is Rejected, got {err:?}");
            };
            assert!(reason.contains("xolotl_env"), "{reason}");
        }
        // The curated exception plus a STRATA_* path value render (the wider
        // value charset the tested config needs).
        let spec = spec_of(&[("xolotl_env", r"GGML_NO_BACKTRACE=1 STRATA_PRESTAGE_GIB=8")]);
        let profile: serde_json::Value =
            serde_json::from_str(&spec.rendered_profile.as_ref().expect("rendered").text)
                .expect("renders JSON");
        assert_eq!(profile["env"]["GGML_NO_BACKTRACE"], "1");
        assert_eq!(profile["env"]["STRATA_PRESTAGE_GIB"], "8");
    }

    /// The env path values only gate the two DSpark keys; every other
    /// STRATA_* path (e.g. a requant cache the server may create on first
    /// load) renders unchecked.
    #[test]
    fn non_dspark_env_paths_render_without_an_existence_check() {
        let spec = spec_of(&[(
            "xolotl_env",
            r"STRATA_DENSE_REQUANT_CACHE=D:\xolotl\cache-not-created-yet GGML_NO_BACKTRACE=1",
        )]);
        let profile: serde_json::Value =
            serde_json::from_str(&spec.rendered_profile.as_ref().expect("rendered").text)
                .expect("renders JSON");
        assert_eq!(
            profile["env"]["STRATA_DENSE_REQUANT_CACHE"],
            "D:\\xolotl\\cache-not-created-yet"
        );
    }

    #[test]
    fn readiness_chains_health_200_then_the_loaded_flag() {
        let spec = spec_of(&[]);
        assert_eq!(
            spec.readiness,
            ReadinessSpec::Then(
                Box::new(ReadinessSpec::Http {
                    path: "/health".to_string(),
                    expect_status: 200,
                }),
                Box::new(ReadinessSpec::HttpFlag {
                    path: "/health".to_string(),
                    expect_status: 200,
                    field: "loaded".to_string(),
                }),
            )
        );
    }

    /// The stub-server leg: `/health` 200 with `loaded: false` is NOT ready;
    /// `loaded: true` is. (The chain order itself is pinned above.)
    #[tokio::test]
    async fn loaded_false_is_not_ready_on_a_stub_server() {
        let port = serve_health(r#"{"loaded":false}"#).await;
        let spec = spec_of(&[]).readiness;
        assert!(matches!(
            crate::engine::http_probe(&client(), port, &spec).await,
            crate::engine::Probe::NotYet { .. }
        ));
        let port = serve_health(r#"{"loaded":true}"#).await;
        assert_eq!(
            crate::engine::http_probe(&client(), port, &spec).await,
            crate::engine::Probe::Ready
        );
    }

    async fn serve_health(body: &'static str) -> u16 {
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("addr").port();
        let app = axum::Router::new().route(
            "/health",
            axum::routing::get(move || async move {
                (
                    axum::http::StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
                    .into_response()
            }),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        port
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("client")
    }

    /// The measurement rides the strata `engine`-block parse: the captured
    /// component triple books its sum, and silence stays absence.
    #[test]
    fn metrics_book_the_strata_engine_block_sum() {
        let body = serde_json::json!({
            "engine": {
                "expert_cache_mib": 61440.0,
                "conversation_cache_mib": 0.0,
                "arena_mib": 0.0,
            }
        });
        let m = measurement_from_metrics(&body).expect("the triple books");
        assert_eq!(m.gpu_gib, 60.0);
        assert!(measurement_from_metrics(&serde_json::json!({"engine": "0.0.1"})).is_none());
        assert!(measurement_from_metrics(&serde_json::json!({})).is_none());
    }

    /// Ctx reads `--ctx` in both spellings (the row `context:` key is llama's
    /// knob and never a fallback).
    #[test]
    fn effective_context_reads_ctx_both_spellings() {
        assert_eq!(
            effective_context(&raw(&[(
                "xolotl_args",
                "--backend cuda, --ctx, 262144, --kv-q8"
            )]))
            .as_deref(),
            Some("262144")
        );
        assert_eq!(
            effective_context(&raw(&[("xolotl_args", "--ctx=262144, --kv-q8")])).as_deref(),
            Some("262144")
        );
        assert_eq!(
            effective_context(&raw(&[("xolotl_args", "--ctx, --kv-q8")])),
            None
        );
        assert_eq!(
            effective_context(&raw(&[("xolotl_args", "--ctx=auto")])),
            None
        );
        assert_eq!(effective_context(&raw(&[("context", "4096")])), None);
        assert_eq!(effective_context(&raw(&[])), None);
    }

    /// The policy row and the adapter stay one mechanism: the custody root,
    /// the offload cell and the knob surface are the compiled-in facts.
    #[test]
    fn the_policy_row_ships_the_xolotl_facts() {
        let row =
            super::super::policy::engine_policy("xolotl").expect("the table ships a xolotl row");
        assert_eq!(row.custody_data_roots, [".simpleharn/xolotl-profiles"]);
        assert!(row.expert_offload);
        assert!(!row.supports_fit);
        assert_eq!(row.knobs.len(), 15);
        assert!(row.config_knobs.is_empty());
        let spec = spec_of(&[]);
        assert_eq!(spec.custody.as_deref(), Some("xolotl"));
        assert_eq!(
            crate::process::rendered_profiles_dir("xolotl")
                .to_string_lossy()
                .replace('\\', "/")
                .rsplit(".simpleharn/")
                .next(),
            Some("xolotl-profiles")
        );
    }

    /// Defense in depth: a rendered profile that claims a harness-forced
    /// field is refused naming every claim (the renderer cannot emit one, so
    /// this pins the door itself over hand-written bytes).
    #[test]
    fn a_profile_claiming_a_forced_field_is_refused() {
        let exe = std::env::current_exe().expect("test exe is a real path");
        let json = format!(
            r#"{{"host": "0.0.0.0", "port": 1, "exe": "{}"}}"#,
            exe.to_string_lossy().replace('\\', "\\\\")
        );
        let err = validate_rendered(&json).expect_err("claims refuse");
        let text = err.clone().into_engine_error().to_string();
        let XolotlRowError::Claim { claims, .. } = err else {
            panic!("claims map to Claim, got {err:?}");
        };
        assert_eq!(claims.len(), 2);
        assert!(text.contains("`host`") && text.contains("`port`"), "{text}");
    }

    /// The malformed optional knobs keep naming their key (G6).
    #[test]
    fn malformed_optional_knobs_name_their_key() {
        for (key, value) in [
            ("xolotl_lib_dirs", "relative\\dir"),
            ("xolotl_gpu", "-1"),
            ("xolotl_reasoning_budget", "huge-but-not-a-number"),
            ("xolotl_sampling", "temperature=lukewarm"),
            ("xolotl_aliases", "one, ,two"),
        ] {
            let err = spec_err_of(&[(key, value)]);
            let EngineError::Rejected { reason, .. } = err else {
                panic!("`{key}`={value} is Rejected, got {err:?}");
            };
            assert!(reason.contains(key), "{reason}");
        }
    }
}
