---
title: Decision ledger (tracked copy)
owner: operator
status: active
audience: [architect, agent]
source_of_truth_for: [settled and deferred decisions]
last_reviewed: 2026-11-27
review_trigger: new-decision, release
---

# DECISIONS - settled doctrine and rulings (binding; do not re-open without reason)

Single home for settled and deferred decisions. Former `handoff/DECISIONS.md` was a
pointer stub and was deleted 2026-11-03.

- **LOCKED**: active doctrine every brief must carry.
- **DEFERRED**: decided later; the tripwire re-opens it.
- **DELIVERED**: tripwire fired; delivery is in the dated rulings.
- **DATED RULINGS**: active and closed rulings with explicit status (decision, date,
  commit; detail lives in the commit body or the named file).

Unresolved questions live ONLY in `DECISIONS-PENDING.md` (a private handoff
repository file, absent from a public clone). This file only ever *gains*
lines, with three operator-ruled exceptions: the 2026-11-03 status-annotation pass may
change lifecycle labels, the section title, and reference qualifiers only; and the
2026-11-22 hygiene pass may compress prose to decision data while preserving every
status, date, commit, pointer, and binding clause; and the 2026-11-25 public/private
pass may replace private-repository evidence-path citations with plain labels while
preserving every status, date, commit, and binding clause; and the 2026-11-27
private-decisions sweep may move rulings that concern the operator or the work
process (not the app's architecture) to the private decision ledger, preserving
status, date, and commit there. Ruling text may not change and no ruling may be
deleted from its home.

## Contents

- [LOCKED (operator doctrine - binding on all agents)](#locked-operator-doctrine---binding-on-all-agents)
- [DEFERRED (decided later - tripwire in parentheses re-opens it)](#deferred-decided-later---tripwire-in-parentheses-re-opens-it)
- [DELIVERED (tripwire fired; the delivery line lives in the dated rulings)](#delivered-tripwire-fired-the-delivery-line-lives-in-the-dated-rulings)
- [DATED RULINGS (binding until superseded; status is explicit)](#dated-rulings-binding-until-superseded-status-is-explicit)

## LOCKED (operator doctrine - binding on all agents)

- **Scrapped by operator**: bonsai class, engine adoption, preload at boot, parallel
  soak, state restore across restarts. v3.3 `external: true` inert; **`sticky:`->AUX
  alias deleted from the parser** (0 uses in either catalog; a yaml with `sticky:` and
  no `category:` lands in WORK). `protected_containers` scrapped - reaping strictly
  ownership-scoped. **No hardcoded third-party software names in code/config
  defaults** (name-free rule) - scoped (operator 2026-10-28 audit): documented
  defaults of supported engines are exempt (strata/llama/ninfer paths, WSL distro
  `Ubuntu`); never names of software outside the harness's engine scope (other apps,
  other people's containers).
- **`bak:` yaml node = silent forensic, manual only** (2026-10-28): never read,
  warned, logged, surfaced, or written back (stripped from `raw` like `external`);
  one line; a second is a problem line; rewrite only inside the incident `.yaml.bak`
  copy, never append one.
- **Admission is the product**: refuse with named blockers, never overcommit;
  evidence-based kill/release (handle signaled + TCP-refused proof); witness 2 s TTL,
  no stale guesses. A job reaps **every** process it owns (victim + own spawn);
  engine kill/release require `(port, generation)`; a stale ticket is refused.
- **Config file is king** (read *and* write-back target `AppConfig.source_path`).
  Discovery reads only `simpleharn.json`; only `SIMPLEHARN_CONFIG`,
  `SIMPLEHARN_TOKEN`, `SIMPLEHARN_LOG` are read, and `SIMPLEHARN_TOKEN` beats the
  file. No other config name, no migration (2026-11-05: SimpleHar name retired).
  Optional `control_token`. Tray red = listener down, not per-job errors.
- **Guard order**: structural refusals (Host, cross-site `Origin` -> **403**,
  content-type -> 415) precede the token (401). Unauthenticated only `/`,
  `/index.html`, `/app.js`, `/style.css`, `/pico.min.css`, `/favicon.png`,
  `/favicon.ico`; `GET /health` **is** token-gated.
- **Client-facing `/v1/models` stays minimal** (no local paths); control surface owns
  detail. Cost verdicts are data (2026-10-28 audit): strata yamls declare
  `cost: greedy` with launch floors (60/65 in `ModelConf/`, not code); ninfer defaults
  to `fixed` at declared vram. The harness never registers/monitors what it did not
  spawn.
- **External engines obey only harness-owned configuration** (2026-10-27): the whole
  child config set (strata profile, ninfer autostart inputs, engine env) lives under
  harness control (like `ModelConf/`), validated at scan/spawn; policy fields (`host`,
  `port`, key) harness-forced, never obeyed from engine-side files. Enforcement build
  = chunk 10 (see the private PENDING_CHECKLIST register); `C:\Strata\*.json` migrate copy-in;
  HANDOFF §3 protects serving profiles until switched.
- **Ledger demoted** (2026-10-26): budget = `total - reserve - observed_used +
  same-slot victim credit - in-flight bookings`; the ledger sum left gate *and* UI,
  surviving only in `LedgerOnlyWitness` (no-GPU mode; dead witness on real HW fails
  closed). Measured `need` = witness total-diff per 60 s as `min-max` (invalidated by
  `cpu_offload`).
- **Scan accepts only `*.yaml`** (regular, non-symlink, case-insensitive; `.yml` and
  everything else silently inert; a problem line never publishes) - Fork C doctrine
  "better not publish anything blindly" (operator 2026-10-28, `e042b52`).
- **Config layers = L1/L2/L3** (2026-10-29; chunk 10 is the first build).
  **L1 Import**: engine-native artifacts (strata profile JSON, ninfer autostart
  script, llama invocation) are fossils of a manual bring-up, copied by an explicit
  operator verb then **forgotten** - after import the harness never reads or writes
  the original (what makes the area Section 3-safe). Re-import under the same model
  name rewrites the copy. Scan never pulls. **L2 Intent**: the small hand-authored
  per-model record. **L3 Launch spec**: generated from L2 + `EnginePolicy` + scrubbed
  L1; the harness launches only from L3. Binding: *L3 must be byte-reproducible from
  L2 + policy + L1*; needing a hand-edit of L3 means the design failed. Dangerous L1
  parameters (`--host 0.0.0.0`, port, foreign keys, `log` paths) are scrubbed **twice
  with different jobs**: at import for auditability (record what was dropped and why),
  at render for authority (never trust an artifact is clean). "Config is not a
  database" (`DESIGN.md` §0.4) stands: files per model and "a problem line never
  publishes" beat queryability. Full ruling and rejected alternatives in ARCHIVED.
- **Measured-footprint provenance** (2026-10-30; refines the Ledger-demoted measured
  clause). A measured figure carries provenance and books only if trusted.
  `MemorySample` gains `attribution: {PerProcess, Unavailable}` - witness-local,
  `PerProcess` iff the compute-process list is non-empty **and** >=1 entry reports
  `used>0`, else `Unavailable` (an idle card is `Unavailable` by design; idleness is
  `used_gib`'s job). A model's booked figure carries a strict yaml stamp
  `measured_attribution: {engine_report | per_process | total_diff}`, written by
  `on_measured` beside `measured_gb`/`measured_gb_min`/`measured_offload`, read by
  `read_measured`. **Booking trust**: `cost_source()` books measured only for
  `engine_report`/`per_process`; `total_diff` **never books** (falls to
  `Declared`/`Estimate`) but stays in the payload labelled untrusted; one accessor
  `bookable_measured()` feeds booking so gate and page share a source (2026-10-26
  duplication rule). Provenance derives from `own_gib(own_pids)` (`Some`->`per_process`,
  `None`->`total_diff`); ninfer startup `MemorySummary` log-parse yields
  `engine_report`. **Void on comparability change** (mirrors `measured_offload`): a
  `cpu_offload` flip, or a ninfer `ninfer_profile`/`--kv-capacity` change since the
  figure was taken, voids the pair; provenance is never upgraded post-hoc (only a
  fresh serve re-measures). **Ruling 11 = metadata-only**: R1 never touches the physical
  gate - the stopper stays observed `used_gib`. **Ruling 15 = (ii)**: an
  `engine_report` figure MAY book (for `fixed`-without-`vram_gb` and eviction
  ranking), bounded by declared `vram_gb` still winning when present, clamp
  `[0, total]`; `exact` engines only verify (`model/mod.rs:71-73`).
  `CardOccupancy`/"foreign-dominated" deferred to R3 (needs the own-vs-foreign
  `unreleasable_gib` split). Evidence: the private research note ninfer-telemetry-2026-10-30
  (local). **AMENDED 2026-11-01 ("Cost = max booking cost")**: the `vram_gb`-wins
  clause holds only as a FLOOR - a guarded `engine_report` figure ABOVE the declared
  value books instead (`engine_report_outranks_declared`); a declared value at-or-above
  the report stands; witness attributions never overshadow a declaration; `greedy`
  untouched. A mid-session raise is not retroactive; the next admit reads the yaml
  pair. R9 re-pinned: `a_declared_row_below_the_report_raises_the_next_admit`.
- **R10 - container-internal ports join the harness/manual separation** (2026-11-01:
  "8003-8005 manual only"; separation is NEXT after the booking/UI commit): the
  separation covers numbers, not only host binds. The container is a **third
  namespace**: `ninfer_internal_port` becomes optional, default **8808** (harness
  87xx +100; amended from 8080 the same day). A row naming a `protected_ports`,
  public-listener, or engine-plan number is refused at arm
  (`AdmissionError::PortClaimed`) and named by rescan (`apply_catalog` twin);
  `AppConfig::port_claim` is the single decision. The canonical template's
  `PUBLISH="${3:-$PORT}"` fallback (the last path that could host-publish a manual
  number) is replaced by `PUBLISH="${3:?...}"`: a missing publish argument fails the
  script, never borrows the host namespace. Live yamls drop the borrowed 8004/8005.

## DEFERRED (decided later - tripwire in parentheses re-opens it)

- **`name:` key** tolerate-iff-equal now -> hard-reject post-tag (frozen suites write
  matching pairs).
- **Maya typed `maya_vision` overlay; per-engine `import` config mechanism** ->
  operator ruling 2026-11-17, implementation deferred (tripwire: the Maya installer
  stops authoring its config JSON, or the operator asks for a full maya profile
  render; see the dated ruling and `TASK-MAYA-VISION-OVERLAY` /
  `TASK-CONFIG-IMPORT-RESEARCH`).

## DELIVERED (tripwire fired; the delivery line lives in the dated rulings)

- **Implemented** **13[10] typed `WritebackError`**: ~~lands with the first consumer
  that branches on the error class~~ - **TRIPPED + DELIVERED** 2026-10-28 (see
  ARCHIVED `b5bf0c1` line).

## DATED RULINGS (binding until superseded; status is explicit)

Former heading `ARCHIVED (tombstones: ruling | date | commit)` is recorded so dated
references still resolve. Every entry carries one status prefix:

| Status | Meaning |
|---|---|
| **Accepted** | operator-approved and still binding. |
| **Implemented** | built and shipped; binding unless a later entry supersedes it. |
| **Superseded** | replaced by a later entry; history only. |
| **Rejected** | refused by the operator; history only. |
| **Closed** | completed or tombstoned; no current constraint. |
| **Recorded** | captured evidence or observation; no new constraint. |

- **Accepted** Engine adapter completeness: every serving knob is exposed through the
  row or classified harness-forced; each adapter carries a knob manifest; an
  unclassified engine knob is a HIGH review finding | 2026-11-15 | `aba8d37`
- **Accepted** Adapter completeness clarified: a supported knob must be usable by
  editing row/config alone (typed or policy-filtered passthrough); hiding row fields
  is fine if adding them back works; requiring an adapter hotfix for a supported knob
  is a HIGH defect | 2026-11-15 | `0fec6b4`
- **Closed** 13[4] Fork C strict scan + persist-reconcile; `commit_verified` blanket
  kept | 2026-10-28 | `e042b52`
- **Closed** 13[5] Fork B restore temp+verify+rename + single-slot `.yaml.bak` +
  strict `.yaml` gate + symlink skip | 2026-10-28 | `e042b52`
- **Accepted** same-stem/temp-shape property pins ruled **vacuous** (single dir +
  full-name temps) - do not pin; re-open only if an arbitrary-path write-back caller
  appears | 2026-10-28 | `951023f`
- **Closed** 13[8] Fork B comment policy: identical-value write = byte-verbatim
  pass-through; a real change drops the inline comment | 2026-10-28 | `7fdf99c`
- **Closed** 13[9] parser charset = `[A-Za-z_][A-Za-z0-9_-]*` (hyphen legal) |
  2026-10-28 | `e042b52`
- **Closed** 13[11] gate pins: hammer + mixed-entry + residue on all paths +
  `write_synced` flush-before-publish (inspection-tripwire, declared) | 2026-10-28 |
  `951023f`
- **Closed** 13[12] `EDITABLE_FLAGS` deleted (dead code) | 2026-10-28 | `951023f`
- **Accepted** Editor race (harness patch vs operator save) = **acceptable doctrine**:
  edit yamls while harness idle / Rescan after; no CAS guard until doctrine fails |
  2026-10-28
- **Rejected** 19[5] duplicate-parked-load dedup sub-claim = **WONTFIX,
  supersede-is-by-design** (`on_swap` supersedes the queued twin deliberately) |
  2026-10-28 (narrowed 2026-11-10, later entry)
- **Implemented** 22[1] strata policy-field claim downgraded to "decided, enforcement
  owed to chunk 10"; 22[2] README witness-is-truth wording | 2026-10-28 | in tree
  (`DESIGN.md`, `README.md`)
- **Closed** DESIGN §20 item 7 yaml write-back (measured triple persists with pins) |
  2026-10-28 | `DESIGN.md:823`
- **Implemented** 14[7]+15[2] tri-state = "A + amber": `Pending` seeded at registry
  construction (absence unrepresentable); tray precedence red>amber>yellow>green>gray
  with 2 s boot grace; no-verdict-is-one-word (Pending and status-fail both amber);
  ready gate never calls no-verdict resolved; landed with 15[1]/[3]/[7]/[10] |
  2026-10-28 | `a149070`
- **Implemented** §20-9 engine schema = Fork C: compiled-in adapters + single-source
  `EnginePolicy` table (forced_fields / forbidden_words / custody_roots / stop_grace /
  cost_default / markers) with ONE generic validator - chunk 10 as "one mechanism,
  four instances". Sub-decisions 1+2: per-engine settings (strata python/cwd) move to
  config `engines:`; per-model yaml keeps references; knob single-source parity test
  (llama FLAG_TABLE <-> yaml <-> webui). Operator-declared `engines.toml` (Fork B)
  REJECTED on doctrine (harness-owned-config is the inverse of openness-posture);
  Fork A is what C prevents; Concept-set adoption deferred | 2026-10-28
- **Implemented** `cpu_offload` engine scoping = B+C+D (operator 2026-10-28, per
  the private research note cpu-offload-scoping-2026-10-28, local evidence not a repository
  document): llama-only via `OFFLOAD_ENGINES` (first `EnginePolicy` row; mock OUT);
  warn-only `ScanKind::InertKey` on engines that never read it; stamp/void engine-gated
  (strata/ninfer measured pairs no longer silently voided); page + API both refuse
  non-llama cpu edits. Sibling llama knobs (`ngl/fit/...`) unscoped until chunk 10 |
  `c6ee754`
- **Implemented** 15[9] token redaction = manual `Debug`: exhaustive-destructure guard
  (E0027 on new fields), `REDACTED(present/absent)`, secrecy declined; ring pin
  race-proofed after 28I | `ae71883`
- **Implemented** Reload-drift (operator 2026-10-29): Axis 1 = 1b queued-intent
  retirement by fingerprint on rescan (`57c5ff6`); Axis 2 = B + launch-time
  server-owned `stale_config` amber (spawn fingerprint; stats on install/reload turns
  only, double-layer zero-stat proof on read paths) + `launched_at` ISO-8601 rendered
  verbatim; C (auto-free) rejected; profile-layer variant -> chunk 10 (custody shrinks
  it) | `270fd16`
- **Superseded** `extra_args` = closed ALLOW-list (`EXTRA_ARGS_ALLOWED`, llama.rs; 11
  reviewed words: embedding/pooling/sampling/warmup family). The 10-name deny-list
  was a doctrine bypass (8/8 smuggles by review F2-0910; 13/13 blocked post-fix,
  28N). Full `EnginePolicy` custody enforcement owed to chunk 10 | 2026-10-28 |
  `7e99679`+engine round
  SUPERSEDED 2026-11-15 in behavior by the Accepted adapter-completeness rows: the
  shipped `extra_args` is an open, policy-filtered passthrough with a reviewed
  harness-owned word record (`LLAMA_FORBIDDEN_EXTRA_WORDS`).
- **Implemented** 15[4]/15[8]: tooltip error-first under truncation; `SIMPLEHAR_LOG`
  strict + reported fallback; shipped in the R2 batch | 2026-10-28 | `7e99679`
- **Implemented** 13[10] typed errors: 8 thiserror variants, String-site 1:1, Display
  byte-identical (review-verified against raw HEAD), api classes 400/409-naming-file/
  500; chunk 13 fully closed | 2026-10-28 | `b5bf0c1`
- **Implemented** Quoted-value seam = auto-quote + refuse-unquotable (operator
  2026-10-28, rec approved): `emit_form` defined solely through
  `split_value_comment` - as-is if faithful (live vocabulary byte-identical), one-pair
  quote-wrap if that round-trips, refuse naming key+value otherwise; quote-wrapped
  incoming words are literal data. Review 28G ACCEPT; LOW closed pre-commit |
  `47f4119`
- **Accepted** `ninfer_profile` default `"b"` = LEAVE AS-IS: autostart scripts already
  default to profile `b`, so the harness copy matches reality - neither warn nor
  refuse; FINDINGS-10 [10] closed without a code change | 2026-10-28
- **Implemented** Ninfer published-port = evidence-only diagnosis, option (b): the
  `$3` offer/contract stance (FINDINGS-10 [5]) stands; the harness does NOT enforce
  where the script published. On readiness failure for a container-backed engine it
  runs ownership-scoped `docker ps --filter publish=<assigned>` / `docker port <name>`
  and names a running-but-elsewhere container instead of a bare `load_timeout`.
  Report never gate; strictly the claimed name; failure-path only (zero
  happy-path WSL round-trip). Lands in chunk 10. Trigger: `ninfer-swift*` scripts
  echo `$PORT` (internal 8004, a protected port) while `--publish` honours `$3`, so
  the sole on-disk artifact of a launch misdirected a whole post-mortem |
  2026-10-29
- **Implemented** (empirical, manual test 2026-11-01) ninfer serve args ride the SPACE
  form - both shipped parsers reject `--flag=value`; `valid_serve_arg` admits bare
  value tokens, refuses single-dash options and every denylist name/prefix; an
  override naming a built-in profile (`ninfer_profile_b`) is refused at launch |
  2026-11-01
- **Implemented** (amends 15) the engine own word is read AT READY, not startup. The
  ninfer booking is max(startup MemorySummary reserve, post-ready `weights
  ready`+`runtime` from the same docker-logs scrape); both figures are the container
  own word; host-KV lines never join. Question A (swap credit) SUPERSEDED: with
  truthful bookings the same-slot credit at finish_admit is honest by construction -
  no witness-measured credit was built. Live yamls cleared of stale `measured_*`
  (estimate fallback until fresh folds); card is ECC-enabled (witness total 70, not
  72) - operator to flip after session. Upstream ask drafted:
  the private draft upstream-ninfer-post-ready-memory-summary | 2026-11-01 | `a7f1623`
- **Implemented** (supersedes "Question A SUPERSEDED"; operator: "(ii) - ok. but we
  will leave an option to revise it later") **same-slot swaps are
  release-then-resample**: when the gate is short and the planner cannot cover the
  deficit, a job with a same-slot victim sets `release_first`, destroys the resident
  it was replacing, and the SETTLE window (which already re-samples the card) acts as
  the re-gate; refusal happens against truth (victim gone: accepted cost, REVISABLE).
  Cross-slot victims are never queued on a paper decision; preflight (chunk 10 [2])
  still refuses before any kill. Evidence: round-2(A) credited 32, freed 44.1
  (refused a fitting swap); round-2(B) debited a stale 62.9 against 44 actual |
  2026-11-01
- **Implemented** BUILT 2026-11-01 R3: refusals name `unreleasable` (= physical used
  - same-slot victim - every evictable candidate), `used_gib`, and `booked_total` via
  `BudgetFacts` (boxed for clippy); zero/absent figures keep the one-sentence
  refusal; NO new error code (row 8). Ruling-10 trust grading = the FOLD gate:
  `grade_card_occupancy` grades the MOVEMENT of unaccounted memory between admit and
  sample against engine growth; static foreign holders are discounted by the baseline
  (a share-based grade would refuse every honest co-serve fold, pinned); a
  ForeignDominated window mints no `total_diff` pair (warn, labelled blank). Gate
  562/0/24 | 2026-11-01
- **Implemented** stale measurements are BLANKED, not annotated: the qwen-turbo
  post-ECC 23.8-59.3 `total_diff` quartet was stripped from both ModelConf dirs; the
  row books at the GGUF estimate (27.7) until a Wave C quiescent serve re-mints.
  Absence chain stands: Declared `vram_gb` -> bookable Measured -> Estimate
  (gguf x1.15 + 2) -> `exact` with nothing = Undeclared refusal. Under-booking vs the
  ~44-59 real footprint is protected only by the physical gate; re-measure is the fix
  | 2026-11-01
- **Implemented** RULED (R5, row 9; operator principle): **the harness operates only
  on resources available to it** - a foreign/unclaimed load's VRAM is never counted
  as the harness's own (never credited, never evictable, already occupied in the
  witness), and refusal on an OBSERVED insufficient card is correct. Foreign memory
  freeing/re-claiming mid-session is NORMAL - handled gracefully (settle-window
  re-samples, no permanent refusal on a foreign reading, nothing foreign killed). R5
  lands as **report-only** reconcile: boot/rescan names a running catalog
  `ninfer_container` with no claim in log + `/har/status` (+ webui); a refusal may
  NAME the unclaimed container beside its `unreleasable` figure. Never gate on a
  foreign footprint guess (per-PID is `[N/A]` box-wide); never kill (G4 unchanged) |
  2026-11-01
- **Implemented** RULED (row 10): **launch-floor achievability guard = refuse early,
  name it** - when `launch_only_with_at_least_vram_free` exceeds what NO release the
  harness can perform would reach, arm-time refusal: "No releases allow for the VRAM
  floor. Check foreign residents." Floor free = currently free + everything the
  harness may release (its own booked, evictable residents); foreign/unclaimed never
  counted (R5 principle, same arithmetic as R3 `unreleasable`). Values KEEP: 60/65 in
  the yamls; the guard NEVER auto-relaxes a floor | 2026-11-01
- **Accepted** RULED (row 17): **template denylist confirmed as-is; profile-override
  lines are THE capacity-shaping mechanism.** `ninfer_serve_args` keeps refusing the
  four harness-owned capacity flags (`--max-context`, `--kv-capacity`,
  `--max-concurrency`, `--kv-dtype`) plus every strict prefix; no serve-args
  allowlist (an escape hatch beside the denylist breaks booking-derivability -
  rejected). Capacity shaping rides `ninfer_profile_<name>` override lines only.
  One-time confirmation: capture `ninfer-serve --help` in Wave C and verify the four
  flag names verbatim | 2026-11-01
- **Implemented** RULED (row 18, option b): **legacy `ninfer_autostart` deprecated
  now, removed after a green Wave C.** Trigger: the Wave C window passes with template
  mode on the operator cards; until then the legacy path is the deliberate escape
  hatch (zero live yaml users). Interim: scan warning on any legacy row. EXECUTED
  2026-11-02 after green Wave C (`805c7d0`, TASK-DEBT-LEFTOVERS): scan
  (`model/scan.rs`) and adapter (`ninfer::legacy_removed_sentence`) refuse a declaring
  row with the migration sentence naming `ninfer_image`, `ninfer_model_name`,
  `ninfer_artifact`, `ninfer_model_roots`; the row never publishes. The legacy launch
  branch, internal-port consequence, R9 profile-gate warn, and stale display path are
  deleted. Template mode is the only ninfer launch path. Manual
  `installation-guide/*/autostart.sh` remain manual launches | 2026-11-01
- **Implemented** RULED (Search#40): **llama books from `llama-fit-params
  --fit-print on`** - a new **Fit** cost class computed PRE-SPAWN from the engine
  calculator (sibling of `llama_server_exe`); stdout `NAME model ctx compute` MiB;
  VRAM = non-Host rows + `mmproj_surcharge_gb` (vision) + `fit_margin_gb` per active
  device (Wave-C-calibrated constants). **Never persisted** - recomputed each admit
  (args-hash cache is the void identity), so the stale `measured_*` class cannot exist
  for Fit; version-locked. Max-truth ladder mirrors amended 15(ii): Fit above declared
  `vram_gb` books; `greedy` floor input reads Fit; `exact`-without-`vram_gb` verifies;
  webui amber "engine-projected" until a witness cross-check promotes green for the
  session. Live-proved 2026-11-01 (turbo: 31.0 projection vs 27.7 estimate vs 36
  observed). Server-log own word = display/audit only (post-spawn, trigger unverified
  `b11669`); hand-written GGUF formula REJECTED (upstream ships the calculator);
  quiescent `total_diff` demoted to the Wave-C calibration. Full design: the
  operator's research note `Search#40-llama-vram-booking-own-word` (kept off-repo;
  external operator-machine reference, not part of this repository). **AMENDED**:
  cache policy = warm **all** llama rows at boot/rescan (deferred serial queue, never
  near `first_bind`, cancelled+redone on rescan, ~0.66 s/row - cells patch in via the
  polling-quiet path), not only unmeasured ones; launch re-probes on any PROVEN change
  (tool/server version mismatch or row/model-file change; args_hash+size/mtime miss);
  no persisted numbers; in-process cache only | 2026-11-01
- **Implemented** RULED (row 19): **strata template mode SHIPS; external
  `strata_config` SUPERSEDED at once.** (a) L3 render: the row yaml owns the fields
  (`strata_exe`, cwd, gguf paths, quant/kv/context/spec knobs, vision block,
  `gpus_asked`); the harness renders the full profile JSON at load and launches ONLY
  its own copy. (b) Rendered-profile root `.simplehar/strata-profiles` beside the
  custody sidecar (content-addressed `<model>-<hash>.json`, keep-N prune, atomic
  write); spawn rewrites `--config` to exactly that path. (c) `simplehar-se import
  strata <profile.json>` (read profile -> emit yaml, delete nothing) replaces the
  held §20-9 copy-in slice; operator adopts live profiles afterwards. Bind is forced
  `--host 127.0.0.1`; port is harness `--port`; rendered profile carries NO
  `host`/`port`/`key` (22[1] clean by construction). A row declaring `strata_config`
  is refused at scan naming the import command; `C:\Strata` stays the manual home
  (import CLI is the one sanctioned reader). No compat path | 2026-11-02
- **Implemented** BUILT same day (`b9c4745` + `3e37c4d`, gate 651/0/24 + DOM 101).
  **Row 19b - faithfulness addendum**: the nvfp4 profile's top-level `sampling`
  defaults (`temperature 1.0, top_p 0.95, top_k 20`) and `uncensored` switch are
  consumed by `sampling_defaults_from_config`. `strata_sampling` (whitespace `k=v`
  number/bool, source types round-trip) and `strata_uncensored` (strict bool; explicit
  `false` distinct from absent) became render knobs and the import maps them. The
  import header reminds that `strata_python:` is a row key no profile carries (the
  nvfp4 row's alternate venv must be copied at adoption). No unmapped keys survive in
  the three live profiles | 2026-11-02
- **Implemented** **Row 19c - the import bridge, the ruling `src/import.rs` cites**:
  `simplehar-se import strata <profile.json>` is the ONE sanctioned reader of a manual
  profile and the only door off external mode. It EMITS, never applies: prints the
  knob block - scheduling keys and `strata_python:` named as row-side merges, every
  uncarried key reported, harness-forced fields reported as dropped - and touches
  neither row nor profile. Adoption is therefore an operator-owned paste (2026-11-02
  Q&A: "why still a paste" - because a profile cannot answer for row-only keys, and an
  auto-rewrite could silently drop the nvfp4 venv). An auto-merge `--apply` was
  offered and is NOT ruled (DECISIONS-PENDING, Needs GO) | 2026-11-02
- **Implemented** **Row 19d - vision faithfulness** (FINDINGS-25 [1], option 2):
  19b's claim held only for ROOT keys - the nvfp4 profile carries nested
  `vision.threads: 16`, and server.py consumes it (`--threads <n>`,
  `serve.server` at server.py:1941-1942). `strata_vision` mirrors the vision spawn's
  FULL cfg contract (server.py:1936-1946): unsigned `threads` and `min_tokens`
  (server's own hand-edited key, #767) join. The import maps both; a nested number it
  cannot carry reports `vision.<key> (present, wrong type)`; outside-contract keys
  report `# not mapped: vision.<key>` as since `d7e3858`. The golden import->render
  round-trip carries both keys every gate run | 2026-11-02
- **Implemented** RULED (B1): **Basalt option A - direct Python server,
  harness-rendered profile.** (a) Launch `basalt_python -m serve.server` in
  `basalt_cwd`, NOT the source-tree launcher; adapter supplies `--engine-bin`,
  `--alias`, `--gpu 0`, `--parallel`; row forces `--host 127.0.0.1` and slot
  `--port`. (b) Row-19 render shape: `{"server": ...}` profile (`alias`, `.basalt`
  path, `parallel` only); stage content-addressed under `.simplehar/basalt-profiles`;
  spawn rewrites `--config` to exactly the staged file, so the child can never point
  at another profile. (c) Cost: declared `vram_gb` with `cost: fixed`; NO Fit
  (engine reports no projection; Python child `per_process` is never "the model";
  grandchild `total_diff` never books). (d) Kill: job-object owns the Python server;
  Basalt kill-on-close engine job is the crash-path orphan guarantee; a card run must
  prove release and no orphan. Rejected: variant B (`basalt.exe serve` - launcher
  layer plus source-tree coupling) and variant C (strata profile reuse - different
  schema and flag ownership). The single-writer scheduler semantics are unchanged.
  Implemented in the adapter; operator card run is final acceptance | 2026-11-03 |
  `TASK-BASALT`
- **Accepted** CALIBRATED (TASK-R11-FIT, C1 capture at `805c7d0`,
  the private wave-verify logs): llama Fit constants stay margin 3 / surcharge 2 /
  session slack 2.0. C1 row books 31,717 MiB / 1024 (CUDA0 only; Host excluded) +
  surcharge 2.0 (mmproj) + margin 3.0 x 1 active device = 35.9736328125 GiB; NVML
  observed 35.61735153198242, so projection is 0.35628128051758 GiB above observation
  (conservative on one sample). A numeric reduction needs multi-sample card-window
  evidence. Pins: `src/engine/fit.rs` C1 arithmetic and the scheduler tolerance test.
  Deferred: `engines.llama.fit_validate_slack_gib` (owes the llama policy row's
  `config_knobs` claim and parity literal) | 2026-11-02 | `TASK-R11-FIT`
- **Accepted** (12-P) the park-path queue fingerprint stays one `fs::metadata` on the
  yaml the just-applied scan read. Scan is zero-fs by design; offloading costs a
  second read or couples queue registration to scan internals; a deferred stat races
  the KEEP-on-absence rule. Cost: one metadata call per parked load. Reopen only if a
  profile shows the park path as a hot spot | 2026-11-03 | `TASK-DEBT-LEFTOVERS`
- **Implemented** FIXED (R2 chunk 01/03): the post-release settle gate is
  reserve-aware. `freed_for(gate_target + vram_reserve_gb, witness_tolerance_gb)`
  matches the direct-admit guard band; the 2026-10-26 one-sided tolerance is
  unchanged. Intentionally stricter: a swap must reach `gate + reserve`, so the
  settle-timeout surface grows near the reserve. Same-slot holder credit counted
  once: ledger term `free_at_admit + (released_cost - holder_released_cost)`, observed
  keeps full release cost. The inflated-skip test is renamed and re-pinned at 55 < 56
  | 2026-11-03 | `TASK-DEBT-LEFTOVERS`
- **Implemented** RULED (R2-14-04/REV6-02): the legacy `simplehar.json` read-only
  rule scopes to auto-discovery and migration only. An explicit `SIMPLEHAR_CONFIG`
  path is an operator instruction and the write-back target even with the legacy name
  (config file is king). Boot honors it and warns once: every write-back rewrites the
  v3.3 file, and v3.3 reads `protected_ports` and may exit on its port-namespace
  check (rollback-strand hazard). Pins: `boot_config` warn, main.rs
  `explicit_legacy_named_config_warns_about_the_rollback_strand` and
  `legacy_named_file_is_a_write_target_only_when_explicitly_selected`, and
  `api::tests::config_write_path_follows_boot_and_never_the_legacy_file_on_fallback`
  (fallback never legacy; stamped explicit selection wins) | 2026-11-03 |
  `TASK-DEBT-LEFTOVERS`
- **Implemented** RULED (R2-02-02, option B): a job that terminalizes with an
  unconfirmed release and a spawned ticket gets one bounded late-release watcher:
  ticket- and generation-scoped, observes only, runs one additional
  `release_timeout_s`, warns once, never re-kills. `Ok(true)` clears the same
  generation; `StaleTicket` means a successor exists and the watch stops without
  touching it. Next load or boot reclaim remains recovery | 2026-11-03 |
  `TASK-DEBT-LEFTOVERS`
- **Implemented** RULED (N3): scheduler cohesion split approved. Phase 1 (free-cluster
  relocation) after the chunked R2 review and release 1.1.0, in a dedicated
  no-scheduler-edits window; Phase 2 (split `impl ActorState`) after a green
  wave-verify; Phase 3 (API split) deferred. Phase 1 and 2 complete. Single-writer
  semantics stay LOCKED; pure moves; cardless gate after each module | 2026-11-03 |
  `TASK-N3-SPLIT`
- **Accepted** RULED (N7): dependency hygiene approved. Remove the unused `reqwest`
  blocking feature, narrow `tokio` features, dedupe `base64`/`syn` where a compatible
  upgrade exists. No new dependency; report any duplicate that cannot be removed
  without an upgrade | 2026-11-03 | `TASK-N7-DEPS`
- **Implemented** RULED (import): GO for `simplehar-se import strata --apply
  <row.yaml>`: merge the emitted block into the live row, keep scheduling keys and
  `strata_python`, refuse on conflict, delete `strata_config` | 2026-11-03 |
  `TASK-IMPORT-APPLY`
- **Implemented** RULED (TASK-ENGINE-MEASUREMENT phases 1-4): a process engine's own
  status figure books as `engine_status` - `EngineReport`-class max truth above the
  declared floor, valid without a profile stamp (profile identity stays ninfer-only),
  voided by an `engine:` flip onto ninfer; `engine_report` stays ninfer-only. The
  witness process-tree sum (engine pid + direct children) books as `per_process`; a
  `total_diff` figure stays display-only. A measured fold re-books the slot occupant
  through one `booking_need`; an amber (`stale_config`) or greedy-launched resident is
  never re-booked. A rescan moving ONLY cost figures re-books the loaded resident
  without reload; a flag move keeps `stale_config` and the unload+reload gesture.
  Elastic residents poll their own status while serving and re-book under down-band
  hysteresis. Card capture stays owed to an operator-freed window | 2026-11-04 |
  `e22695b`..`11f76f4`
- **Accepted** F-EM-402 and F-EM-404 as LOWs (operator-delegated orchestrator ruling):
  a floor-clamped elastic resident's up-churn persists its measured pair while the
  floor-clamped booking stands; a mid-swap rescan re-book is conservative and
  plan-time ledger terms of the in-flight swap stay frozen | 2026-11-04 | `11f76f4`
- **Recorded** (2026-11-04 freed card window, TASK-ENGINE-MEASUREMENT) the real engine
  surfaces are `/metrics`
  `engine` blocks; `/status` is liveness only; `/v1/status` carries no `info.vram`;
  shipped parses corrected to the captured shapes. The booked figure is a LOWER BOUND
  (Strata components 59.9 GiB vs 69.75 hold vs 67.8 declared; Basalt 83.7 GiB exceeds
  the 72 GiB card due to RAM offload, so only `expert_cache_mib` books); the declared
  floor remains the booking. Process-tree NVML fallback unavailable on this driver
  (`attributed_gb: null`). Basalt is greedy with dynamic offload by ruling - the
  adapter then still refused the class: `TASK-BASALT-GREEDY`. Evidence:
  `docs/evidence/2026-11-04-engine-measurement.md` | 2026-11-04 | `bbbf74e`
- **Implemented** RULED (TASK-BASALT-GREEDY, "it's greedy and dynamically offloads to
  RAM"): Basalt accepts `cost: greedy`; floor 60; the operator sets
  `launch_only_with_at_least_vram_free: 60` after this build deploys (a greedy edit
  ahead of the new binary drops the row). The greedy nominal never refuses an admit
  (`need_deficit = 0`) and books at most the room at admit; the launch floor is the
  gate. `vram_gb` stays required for `cost: fixed` and may be absent on a greedy row
  (an unknown nominal is the legal greedy case, DOCTRINE 9/12); a present `vram_gb`
  on greedy is the last measured figure - never refuses, books only up to the room;
  the measured pair never books or re-books a greedy row. A component
  sum is never a footprint (83.7 GiB > 72 GiB under RAM offload). `cost: exact`
  refuses naming the two bookable classes | 2026-11-04 | `1fc2681`
- **Accepted** (R2-05-04) an over-limit body keeps `400 bad_request`, not `413`;
  request-shape refusals stay one class (same spirit as R2-04-03).
  `docs/reference/API.md` states it; the two limit pins name the ruling. R2-05-03
  closed beside it: unknown routes (`404 not_found`) and wrong methods
  (`405 method_not_allowed`) answer the project envelope on every listener; a proxy
  pass-through keeps the engine body byte-for-byte. The `har_log` query-rejection leg
  is unreachable on axum 0.8 (the form decoder is lossy and the map type accepts every
  pair); the handler keeps the typed-400 arm defensively and the test pins the lenient
  decode | 2026-11-04 | `ad19144..`
- **Accepted** (2026-11-04 debt sweep): R2-04-02 - the `snapshot` scan cost per
  occupied slot inside the 2 s publish stands (microseconds at this catalog size);
  reopen only on a measured hot spot (12-P precedent). N6 - api-owned byte-level
  source pins stay as deliberate documented tripwires; a rename forces a pin edit by
  design. R2-17-02 - the classifier FINDING branch is recorded INFO and lands with the
  first FINDING-emitting test | 2026-11-04 | `e5b4a0f..`
- **Superseded** Project renamed to SimpleHarn (2026-11-05; repo renamed 2026-11-04):
  product, binary, crate = `simpleharn`; primary config `simpleharn.json`;
  `simplehar-se.json` and `simplehar.json` read in place (write-back on the loaded
  file); `SIMPLEHARN_*` wins with `SIMPLEHAR_*` honored as deprecated fallbacks.
  Legacy-stable on purpose: wire `simplehar_error`, `owned_by: "simplehar"`,
  single-instance mutex, `.simplehar/` custody. Release `v1.2.0` = `8fcaa82`.
  SUPERSEDED 2026-11-05 by the entry below | 2026-11-05 | `01f135d..`
- **Implemented** **No SimpleHar compatibility; the name retires fully** (operator
  instruction 2026-11-05, TASK-NAMING-CLEANUP; supersedes "legacy-stable on purpose"
  above): the clash with the SimpleHAR HAR-file viewer retires every legacy spelling.
  Config discovery reads only `simpleharn.json`; an old `simplehar-se.json` or
  `simplehar.json` is ignored. Only `SIMPLEHARN_CONFIG`, `SIMPLEHARN_TOKEN`,
  `SIMPLEHARN_LOG` are read; `SIMPLEHAR_*` is inert. Wire values are `simpleharn_error` and
  `owned_by: "simpleharn"`; mutex `SimpleHarn-SingleInstance`; roots `.simpleharn/`.
  No fallback read, no migration | 2026-11-05 | `01f135d..`
- **Accepted** CPU-llama eviction rules: RAM is its own resource ladder - a CPU-only
  row bypasses VRAM legs, never blocks a `whole` request, and is never a VRAM victim;
  `ram_reserve_gb` defaults to 8 GiB; CPU rows are evicted only by CPU admissions and
  GPU jobs are never RAM victims | 2026-11-05 | `01f135d..`
- **Accepted** Module split waves: S1 (scheduler tests out of `scheduler/mod.rs`) and
  S2 (catalog/drift helpers into `catalog.rs`) approved. Move-only commits, both-shell
  gates per commit, no behavior change and no test-logic edits | 2026-11-05 |
  `01f135d..`
- **Accepted** README drops the front-matter banner: GitHub renders README on the
  landing page where YAML front matter shows as a metadata table. `README.md` starts
  at the H1; the exemption lives in `docs/DOCS_README.md` §5 and
  `scripts/check-docs.mjs`. Every other durable doc keeps front matter. Storefront
  screenshots only from operator-validated captures | 2026-11-05 | `011d8a5..`
- **Accepted** Category names become a config option: the four names are defaults,
  not fixed labels; `categories[].label` drives page and `/har/status`; ids
  `WORK/RP/AUX/EMB` stay the wire and eviction vocabulary (enum, yaml `category:`,
  `evictable_by`, engine-port order). Scheduled after `TASK-CPU-LLAMA-EVICTION`; see
  the private task record TASK-CATEGORY-LABELS | 2026-11-05 | `9c1bfda..`
- **Accepted** Unauthenticated asset list grows to seven: `/favicon.png` and
  `/favicon.ico` join `/`, `/index.html`, `/app.js`, `/style.css`, `/pico.min.css`;
  the control page serves its icon without a token. DOCTRINE item 7 and the
  guard-order entry state the list | 2026-11-05 | `8fe7d2c..`
- **Accepted** Project Maya engine adopted: `engine: maya` runs installed Maya
  `serve.server` directly (`maya_python -m serve.server --engine maya`), never
  `maya.py` (launcher opens a browser and may rebuild/calibrate). `maya_config` is
  L1: validate every path, strip `host`, `port`, `api_key`, `idle_unload_s`,
  `cors_origins`, force `model_management: false`, stage scrubbed JSON
  content-addressed under `.simpleharn/maya-profiles`, force `--host 127.0.0.1`, slot
  `--port`, `--idle-unload 0`. Readiness `/health` 200 with `/model` ==
  `serve_model`; `load_timeout` >= 300 s; `cost` fixed (with `vram_gb`) or greedy;
  `exact` refused. Card run 2026-11-06: both operator configs served and released
  (19.6 and 9.1 tok/s, 4 s unload), 60 GiB floor refused with a foreign Basalt
  resident, model-management doors answered 404. Residual: dashboard doors stay on
  the loopback slot port; do not use them on a managed instance (`MODELS.md`).
  Evidence the private evidence record 2026-11-06-maya | 2026-11-06 | `2de4725`
- **Accepted** Expert-offload RAM reclaim: strata, basalt and maya offload expert
  layers to host RAM then SSD. A `cost: greedy` admit on these drains every
  CPU-resident row whose booked host RAM exceeds `expert_offload_evict_cpu_above_gb`
  (default `4.0`, `0` disables), largest first, through the same confirmed release
  and RAM settle as the VRAM planner, before the single spawn. A row at the threshold
  stays; GPU residents are never victims; a fixed row or non-expert engine never
  triggers. A protected row or one inside `min_turn` is skipped and named; the load
  proceeds. An admission that queued a victim still fails if that victim cannot be
  released. Evidence the private evidence record 2026-11-06-ram-reclaim; card proof owed in
  the 1.3.2 window | 2026-11-06 | `938660b`
- **Accepted** Hidden manual launcher: the release ships `SimpleHarn.vbs`, which
  starts `simpleharn.exe` with window style 0 from its own folder. The binary stays a
  console build; direct exe is the diagnostic path; no console `.bat` wrapper ships |
  2026-11-06 | `6a371a0`
- **Accepted** Llama engine binary: optional global default and per-row fork override.
  No shipped `llama_server_exe` default. First start records a `llama-server.exe`
  found on absolute `PATH` entries into `simpleharn.json`. A llama row may name its
  own absolute `llama_server_exe`; the row value wins. If neither names a binary, the
  load refuses naming the key. The fit probe groups rows by effective binary and keys
  version and cache per tool | 2026-11-06 | `this change`
- **Superseded** LAN bind without a token stays allowed, with a page warning: a
  non-loopback `internal_host` with no `control_token` or `SIMPLEHARN_TOKEN` remains
  a valid boot. `/har/status` carries a `config_warnings` entry; the control page
  shows a persistent warning banner. Boot does not refuse and adds no acknowledgement
  key | 2026-11-06 | `this change`
  SUPERSEDED 2026-11-08 by the LAN opt-in row below (shipped `faaf9d6`).
- **Implemented** LAN bind needs the explicit opt-in: a non-loopback `internal_host`
  with no `control_token` or `SIMPLEHARN_TOKEN` is refused at boot unless
  `allow_unauthenticated_network_bind: true` (default `false`); the refusal names both
  remedies. With the opt-in set and still no token, the control page keeps a
  persistent, no-dismiss warning banner on every read. The opt-in is a deliberate boot
  gate, not an endorsement | 2026-11-08 | `faaf9d6`
- **Accepted** Model rows stay hand-written: the catalog scans only
  `ModelConf/*.yaml`, not model directories. Engine-specific keys, category and cost
  decisions, split GGUF shards and mmproj files, and conservative admission require
  an operator-authored row. `MODELS.md` documents the choice and a copy-paste row |
  2026-11-06 | `this change`
- **Accepted** Toolchain and dependency policy: build toolchain `1.98.1` in
  `rust-toolchain.toml`; MSRV `1.88`; CI tests `cargo +1.88.0`; Dependabot covers
  cargo and Actions weekly; cargo-deny enforces advisories, licenses, bans, and
  sources. A bump must keep the MSRV a deliberate choice | 2026-11-06 | `this change`
- **Accepted** Same-slot release projection: the admit credit for the occupant a
  replacement is about to release is the larger of its booked need and the VRAM
  footprint its own spawn folded; it is a projection, never a booking, and the settle
  re-gate stays physical truth. Figure `Occupant.live_measured_gb` is set only by a
  VRAM fold matching the spawn ticket, never seeded from disk; no this-spawn fold
  keeps the booked credit and the 2026-11-01 guard verdict. Booking itself is
  unchanged (DOCTRINE 12). Incident: a greedy Strata booked `vram_gb: 12` while
  holding 68.4 GiB; a Basalt replacement with a 60 GiB floor died on paper ("No
  releases allow for the VRAM floor") although the release physically fit - the
  release-first door of the 2026-11-01 ruling was unreachable on a booked credit.
  Logging gained `victim_need_gib` and `victim_release_gib`.
  Residual accepted: if the projection overstates a release (foreign traffic during
  serve), the kill can precede a settle re-gate that refuses | 2026-11-06 |
  `this change`
- **Accepted** Xolotl engine adapter (DeepSeek-V4.1-Flash GSQ; `ds41_generate.exe`
  behind its `serve_ds41.py` shim) joins as a compiled-in `engine: xolotl` row, not a
  strata mode or launcher knob. The harness renders/stages the serve profile from row
  knobs; the operator JSON is a value source, never launch truth. Forced launch:
  `python -I <launcher> --engine strata --host 127.0.0.1 --config <staged> --port
  <slot>`; readiness `/health` 200 with `loaded: true`; measured sum from the same
  `/metrics` engine block as strata. `aliases` and `reasoning_budget_tokens` join the
  render as optional row keys; env allow-list is `STRATA_*` plus curated
  `GGML_NO_BACKTRACE`. First cut is the 256K profile only (one row): `cost: greedy`
  with `launch_only_with_at_least_vram_free: whole`, host-RAM reclaim via
  `expert_offload`, operator card sequencing. A DSpark sidecar named by the row and
  missing on disk refuses by path. Deferred ruling with the seam kept open: hybrid
  booking (a non-CPU row claims host RAM `ram_gb` and admission gates both ledgers);
  not adopted now because the whole-card posture plus the reclaim leg cover the tested
  recipe, and the change reaches into the RAM gate's doctrine | 2026-11-08 |
  `this task`
- **Accepted** API surface additions deferred: the proxy keeps its closed route set
  (`/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`) plus harness routes;
  no endpoint added now. Deferred candidates ranked by cost: `GET /v1/models/{id}`
  (harness-only catalog lookup); `POST /v1/tokenize` and `/v1/detokenize`
  (engine-native, admitted only where the resident serves them); `POST /v1/responses`
  (translation and state task); engine introspection (`/props`, `/slots`) covered by
  harness status. Images, audio, files, batches, rerank stay out. Governance: never
  per-endpoint booleans; a route is admitted only where resident engine capability
  serves it; matrix in `COMPATIBILITY.md`. Escape hatch = single exact-path allowlist
  with empty default, never wildcard proxy | 2026-11-08 | `this task`
- **Accepted** Linux support: design authorized. The port is a platform boundary, not
  a second product: same categories, ports, YAML rows, admission rules, refusals; with
  `windows` target-gated and a `src/platform/` trait boundary for process identity,
  tree control, host RAM, single instance. Windows remains shipped until phases land.
  Tray becomes optional; Linux boots headless on the same ports. Adapters declare
  platforms: llama, strata, basalt, maya, xolotl document Linux (research 2026-11-08;
  xolotl Linux-primary and source-built); ninfer ships native Linux but this adapter
  is WSL-shaped, so Linux there is a separate adapter (that clause is superseded by
  the v1.5.0 shipped row below). Linux card evidence BLOCKED until a GPU Linux
  machine exists. Open: commitment beyond the boundary phase, tray posture,
  first-release engine set, artifact source | 2026-11-08 | `this task`
- **Accepted** Xolotl rows use a numeric floor on a GUI host:
  `launch_only_with_at_least_vram_free: whole` is exclusive - it requires the whole
  budget free and names every other resident blocker, so a three-monitor workstation
  holding 1-3 GiB of desktop VRAM can never satisfy it. The 256K row uses a numeric
  floor sized to the machine (66 GiB tested). The free witness already excludes
  foreign VRAM, so the floor accounts for the GUI without weakening the gate. Whole
  remains the posture for dedicated cards. First load: chat 200, unload 200; engine
  held 71.4 GiB with 1.1 GiB free | 2026-11-08 | `this task`
- **Accepted** Correction: whole already allows foreign occupancy up to the reserve.
  The xolotl entry above overstated the refusal. `whole` requires the credited
  witness to reach `total - reserve` (the error field is literally
  `total - 2 * reserve` on a figure that already nets the reserve), so up to
  `vram_reserve_gb` of foreign unreclaimable residency is allowed and only the excess
  is a blocker. A desktop holds ~2.6 GiB against a 2.0 GiB reserve, hence the 0.6 GiB
  shortfall. Levers: raise `vram_reserve_gb` to cover the GUI baseline, or use a
  numeric floor sized to need (66 GiB tested, 67 GiB recommended). The 256K row uses
  the floor; `whole` stays for dedicated cards | 2026-11-08 | `this task`
- **Accepted** Linux port committed; whole posture clarified: phases P0 boundary, unix
  implementations, engine set, release. Linux boots headless by default; tray is
  optional, non-default. First Linux release covers llama and strata (superseded by
  the v1.5.0 shipped row below). Artifacts are
  built by CI. WSL2 is the Linux playground; card legs run there where GPU passthrough
  allows, else BLOCKED. `whole` and `vram_reserve_gb` are documented as the exclusive
  posture with a foreign-residency allowance: free when the witness reaches
  `total - reserve`, so a desktop holding more than the reserve keeps a `whole` row
  refused forever. Levers: higher reserve (covers GUI baseline for every row) or a
  numeric floor sized to measured footprint plus margin. A floor below real need
  admits and lets the engine abort - an operator promise, not a measurement |
  2026-11-08 | `this task`
- **Implemented** Linux port shipped in v1.5.0 (D1-D5): headless default with an
  optional tray; one adapter covers ninfer on Linux (empty `wsl_prefix`, same
  canonical script, no `wsl.exe` hop); six engines serve on Linux in the same policy
  table; artifacts are built by CI. Supersedes the "separate adapter" and "first
  Linux release covers llama and strata" clauses in the two 2026-11-08 rows above |
  2026-11-09 | `v1.5.0` (`8c20550`)
- **Accepted** UI polish keeps both pinned accents: the bright serving dot
  (`--sh-dot-live: #22c55e`) and the matte olive selected-tab accent stay. The
  perceived loudness came from Pico's saturated `#0172ad` primary painting every idle
  tab and row button in dark mode, not from either pin. Dark mode routes
  `--pico-primary*` and form variables to a muted slate chrome plate (`#1e2631`,
  border `#2d3846`), keeps slate-blue `#8ab4d8` for outline controls and links, and
  leaves hue to state signals: olive selected tab, green serving dot, amber Unload
  family, cost provenance. Large surfaces and badges drop saturation; no pinned hex
  moves | 2026-11-09 | `this task`
- **Accepted** A parked load joins its identical twin: the earlier 19[5] WONTFIX
  ("supersede-is-by-design") is narrowed, not deleted. A repeat load for an already
  **parked** model attaches a second waiter to that queued row, so a double request
  yields one job, one outcome, one ledger line (observed `7 · Maya-M · Idle
  cancelled…` then `8 · Maya-M · Ready` from two POSTs). A load for a **different**
  model still supersedes the queued twin, by design. The duplicate caller waits
  through the same load as the first, consistent with the on-slot attach path |
  2026-11-10 | `this task`
- **Accepted** A load short only on in-flight bookings waits: admission debits
  another non-terminal load's whole booking on top of the physical memory it has
  taken, so a second load is refused until the first reports Serving even when both
  fit (Ninfer-nvfp4 booking 29.8 GiB; RP-1 refused three times at free 16.4/6.7,
  admitted when Ninfer terminalised). When `deficit <= in_flight`, the row has no
  victim, no releasable candidate, and no unreachable floor, the gate parks the row
  on its free slot (`pending[slot]`, state `Idle`, waiters attached) instead of
  refusing; the terminal funnel re-runs admission for parked rows whose slot is free.
  A re-gate that still cannot fit re-parks; a gate short on foreign memory takes
  `AdmitFail`. Paper bookings are never credited as physical bytes. Adds the
  `(Admit, Idle)` edge to the state graph; the supersede path may take a parked row's
  queue place before killing an in-flight load | 2026-11-10 | `this task`
- **Accepted** Maya vision joins the render rule through a typed overlay: every
  similar engine follows the strata and xolotl rule - the harness renders the serving
  profile itself. For maya the shape is a typed `maya_vision` overlay: the row spells
  the vision block, the renderer merges it into the staged L1 profile, and it refuses
  a conflict with an engine-side `vision` object. Every other installer-generated key
  still flows as L1, so a new upstream key needs no adapter change (DOCTRINE item 19).
  A full maya profile render is not chosen now: Maya's installer authors that JSON,
  and a closed typed set would turn each upstream key into an adapter hotfix.
  Implementation deferred to `TASK-MAYA-VISION-OVERLAY`.
  `TASK-CONFIG-IMPORT-RESEARCH` covers the per-engine `import` config mechanism,
  implementation deferred | 2026-11-17 | `this task`
- **Implemented** Strata 0.2.19 layout move (operator update 2026-11-22): the
  checkout stays `C:\Strata`, but the runtime moved under `C:\Strata\terminal`
  (venv, `engine\strata.exe`, `engine\strata-vision.exe`, `data`, `serve`, run
  configs). The adapter defaults are
  `C:\Strata\terminal\.venv\Scripts\python.exe` and `C:\Strata\terminal`
  (`-m serve.server` does not resolve from the old checkout root); a row on the
  main install that overrides `strata_exe`, `strata_lib_dirs`, `strata_vision`,
  or the `--expert-profile` argument names the terminal paths. Engine stays
  0.1.55; host/port/key forcing and the render rule are unchanged. The
  manual-launch home is `C:\Strata\terminal`; this supersedes row 19's
  manual-home clause for runtime paths. The source checkout root remains
  `C:\Strata`, and the separate `C:\Strata-nvfp4` fork keeps its own root. The
  updater's rewritten run configs are not a harness launch source; the row
  remains the source | 2026-11-22 | `this change`
- **Accepted** In-progress-release park (operator 2026-11-22): a card load
  whose deficit is covered by a resident release already in progress (a
  non-terminal `Unload`) and/or in-flight bookings parks. For a release wait,
  every named blocker must be a releasing resident and the floor must be
  reachable with the release; the in-flight booking park keeps its 2026-11-10
  rule (its named blocker is not the wait source). The release credit is a
  projection - `max(booking, live_measured)` per resident, clamped to occupied -
  never a booking; the planner treats a committed release as a blocker, never a
  victim. A partial evictable set does not join the wait coverage: park only
  when the release covers; a needed candidate refuses, and the retry after the
  release plans it. Same-slot release-first, `whole`, and the RAM ladder are
  unchanged; the RAM park matrix is deferred. | 2026-11-22 | `this task`
- **Accepted** Scan accepts `*.yml` like `*.yaml` (operator override of
  Doctrine 10's inertness clause; review 2026-11-25 item 1): a regular
  non-symlink `.yml` file publishes as an entry or a named problem exactly as
  `.yaml` does, case-insensitive; every other extension stays silently inert;
  identity stays the file stem, so an `m.yaml`/`m.yml` pair reports the
  existing `DuplicateName` refusal; write-back keeps the row's own extension |
  2026-11-25 | operator session 2026-11-25
- **Accepted** Llama booking ruling B: the AUX swift row declares `vram_gb: 23`
  and the Fit probe stays on, so `/har/models` books `fit_gb 24.0078125` at
  `cost_basis fit`; the card re-test on `73dc938` passed (basalt 67.4 and xolotl
  68.0 admitted via `Drain` with the swift released), `BOOKING-CREDIT-02` is
  verified for the live case, and a witnessed-footprint credit stays a
  code-risk item for any other under-booked row | 2026-11-22 | ruling B,
  transcribed from the handoff snapshot on 2026-11-25
- **Accepted** Same-slot pre-refuse matrix, option B (operator go 2026-11-26):
  "a same-slot re-admit refuses at the gate, before any release, only when its
  own settle gate could never pass on any witness reading of this card - settle
  target plus reserve minus the witness tolerance exceeds the most free GiB
  total minus what no harness release can reclaim - and the refusal names the
  need, the ceiling, and the never-releasable holders; every other shortfall
  keeps release-then-resample." Edge-case table and analysis in the private
  layer (record same-slot-pre-refuse) | 2026-11-26 | operator go on the
  2026-11-26 elaboration
- **Accepted** Sole-resident floor credit (operator go 2026-11-26): "the floor
  projection may credit a victim that is the only booked GPU resident at
  max(booked, card holds minus other installed bookings), bounded by the live
  witness and never counting foreign or unattributable bytes; it is a
  projection, never a booking, and the physical gate, the settle re-gate, the
  in-flight debit rule, and ledger-only mode stay unchanged." Edge-case table
  and analysis in the private layer (record booking-credit-02-residual) |
  2026-11-26 | operator go on the 2026-11-26 elaboration
