---
title: Engine knob manifests
owner: operator
status: active
audience: [agent, reviewer]
source_of_truth_for: [engine adapter knob manifests]
last_reviewed: 2026-11-15
review_trigger: engine-adapter-change, release
---

# ENGINE_KNOBS.md

Up: `docs/DOCS_README.md`. The gate is `docs/reference/MODELS.md` "Engine
adapter gate: all serving knobs" and `docs/architecture/DOCTRINE.md` item 19.

Each engine has a manifest below. A row is one engine knob or one named group;
`Support` is `typed` (row key renders it), `passthrough` (an open, policy-filtered
operator channel carries it), or `forced` (the harness owns it, with the reason).
A `missing` or unclassified row fails `scripts/check-engine-knobs.mjs`, which
runs in the cardless gate. The full per-flag audit (261 llama flags, the Strata
config keys, the Xolotl shim keys) is in the handoff audit reports; this file is
the checked-in manifest the gate reads.

## llama

| Key/flag | Support | Row key or forced reason |
|---|---|---|
| `context` | typed | `context:` |
| `ngl` | typed | `ngl:` / `cpu_offload:` |
| `batch` | typed | `batch:` |
| `ubatch` | typed | `ubatch:` |
| `flash_attn` | typed | `flash_attn:` |
| `cache_type_k` | typed | `cache_type_k:` |
| `cache_type_v` | typed | `cache_type_v:` |
| `parallel` | typed | `parallel:` |
| `fit` | typed | `fit:` |
| `fit_target` | typed | `fit_target:` |
| `fit_ctx` | typed | `fit_ctx:` |
| `mmproj` | typed | `mmproj:` |
| `jinja` | typed | `jinja:` (true/false) |
| `all non-forced llama-server flags` | passthrough | `extra_args:` (reviewed denylist) |
| `host`, `port`, `reuse-port` | forced | Bind and port are harness-owned |
| `api-key`, `api-key-file`, `ssl-*` | forced | Key material and TLS are harness-owned |
| `path`, `slot-save-path`, `media-path`, `models-*` | forced | Static roots, custody, and the engine-side router are harness-owned |
| `model`, `hf-*`, `docker-repo`, `model-url`, `mmproj-url`, preset weights | forced | Which weights serve; download egress is harness-owned |
| `tools`, `mcp-*`, `agent`, `ui-mcp-proxy`, `connect` | forced | Tool execution and egress surfaces are refused |
| `cors-*`, `props`, `sleep-idle-seconds` | forced | CORS, mutation, and residency are harness-owned |
| `lora*`, `control-vector*`, `override-kv`, `device`, `main-gpu`, `split-mode`, `tensor-split`, `spec-draft-device` | forced | Adapter, KV, and multi-GPU placement decisions are harness-owned |
| `removed and utility flags` | forced | Removed in the pinned build or not serve knobs (`--help`, `--version`) |

## ninfer

| Key/flag | Support | Row key or forced reason |
|---|---|---|
| `autostart template`, `container`, `profile`, `image`, `model_name`, `artifact`, `internal_port`, `model_roots`, `gpu_device`, `model_id`, `key`, `key_file`, `serve_cmd` | typed | `ninfer_*:` row keys |
| `serve args` | passthrough | `ninfer_serve_args:` (reviewed denylist) |
| `env` | passthrough | `ninfer_env:` (multi-pair, `NINFER_*`) |
| `host`, `port`, `api-key`, `uds`, `ssl-*` | forced | Bind, key, socket, and TLS are harness-owned |
| `data-dir`, `data-dir-permissions`, `log-level`, `log-dir` | forced | Container lifecycle and log discipline are the canonical script's |
| `model`, `model-id`, `max-context`, `kv-capacity`, `max-concurrency`, `kv-dtype` | forced | Weights and booked capacity are harness-owned |

## strata

| Key/flag | Support | Row key or forced reason |
|---|---|---|
| `exe`, `args`, `cwd`, `tokenizer`, `lib_dirs`, `env`, `gpu`, `parallel` | typed | `strata_*:` row keys |
| `vision` | typed | `strata_vision:` (closed k=v set; includes elastic/mib/idle_s) |
| `sampling` | typed | `strata_sampling:` |
| `aliases` | typed | `strata_aliases:` |
| `reasoning_budget_tokens` | typed | `strata_reasoning_budget:` |
| `other non-forced profile keys` | passthrough | `strata_profile_extra:` (JSON; reviewed denylist) |
| `host`, `port`, `api_key` | forced | Bind and key are harness-owned |
| `idle_unload_s`, `CORS`, `static roots`, `vram floor` | forced | Residency, CORS, roots, and admission are harness-owned |
| `replicas`, `split_serve`, `layer_split`, `gpu_order`, `hip_ordinal`, `before_load*` | forced | Multi-GPU/lifecycle hooks need an admission ruling before exposure |

## xolotl

| Key/flag | Support | Row key or forced reason |
|---|---|---|
| `exe`, `args`, `cwd`, `tokenizer`, `lib_dirs`, `env`, `gpu`, `sampling`, `aliases`, `reasoning_budget_tokens` | typed | `xolotl_*:` row keys |
| `vision` | typed | `xolotl_vision:` (shim key set incl. `threads`, `max_tokens`, `min_tokens`; `gpu` default false; emits `--vision`) |
| `parallel` | typed | `xolotl_parallel:` (1..=16) |
| `other non-forced profile keys` | passthrough | `xolotl_profile_extra:` (JSON; reviewed denylist) |
| `host`, `port`, `api_key` | forced | Bind and key are harness-owned |
| `idle_unload_s`, `CORS`, `static roots`, `vram floor` | forced | Residency, CORS, roots, and admission are harness-owned |
| `replicas`, `split_serve`, `layer_split`, `gpu_order`, `hip_ordinal`, `before_load*` | forced | Multi-GPU/lifecycle hooks need an admission ruling before exposure |

## basalt

| Key/flag | Support | Row key or forced reason |
|---|---|---|
| `python`, `cwd`, `exe`, `parallel`, `vision` | typed | `basalt_*:` row keys |
| `engine args` | passthrough | `basalt_engine_args:` (reviewed denylist) |
| `sampling defaults and generation budgets` | typed | `basalt_sampling:` (JSON map) |
| `host`, `port`, `api-key`, `cors-*`, `browser`, telemetry | forced | Bind, key, CORS, browser, and telemetry are harness-owned |
| `gpu`, multi-GPU placement, min-free-VRAM, idle lifecycle | forced | Admission and residency are harness-owned |

## maya

| Key/flag | Support | Row key or forced reason |
|---|---|---|
| `python`, `cwd`, `config` | typed | `maya_python:` / `maya_cwd:` / `maya_config:` |
| `all config JSON keys except forced fields` | passthrough | `maya_config:` external JSON (scrubbed) |
| `host`, `port`, `api_key`, `idle_unload_s`, `cors_origins` | forced | Bind, key, residency, and CORS are harness-owned |
| `model_management` | forced | Always false; the harness is the single writer |
| `--mcp-config`, `--script`, `--parent-pipe`, `--open` | forced | Tool execution, lifecycle pipe, script, and browser are refused |

## Drift note

`strata_uncensored` and `strata_gpus_asked` are rendered but the installed
`C:\Strata\serve` reads neither. They are a cleanup candidate, not a supported
knob; remove them from the render and rows after confirming the install.
