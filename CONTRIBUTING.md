---
title: Contributor pointer
owner: operator
status: active
audience: [contributor]
source_of_truth_for: []
last_reviewed: 2026-11-06
review_trigger: release, quarterly
---

# CONTRIBUTING.md

This file is a pointer. It owns no rules. Read `README.md` for the product.

## Where the rules live

| Topic | File |
|---|---|
| Documentation map | `docs/DOCS_README.md` |
| Task lifecycle and templates | `docs/process/TASKS.md` |
| Coding standards | `docs/process/ENGINEERING.md` |
| Gate and evidence | `docs/process/VERIFICATION.md` |
| Git, commits, and release | `docs/process/GIT_WORKFLOW.md` |

## Private handoff state

The live queue, the handoff snapshot, open findings, traps, and evidence live in
the private repository `legislat/SimpleHarn_handoff`. The code repository reaches
that root through `SIMPLEHARN_HANDOFF` (see `docs/process/HANDOFF.md`). Ask the
operator if you need access.

## Cardless contributor path (fresh public clone)

This path runs from a clean clone of `legislat/SimpleHarn`. It needs no private
repository, no card, no model, and no engine.

1. Clone the repository. Install the toolchains CI uses: Rust 1.98.1 and
   Node.js 24. The crate minimum supported Rust version is `rust-version =
   "1.88"` in `Cargo.toml`. Install PowerShell 7 to run the gate script.
2. Run `pwsh scripts/gate.ps1`. It runs the build, the tests, clippy, the
   format check, the web UI DOM test, and the doc, instruction, comment, path,
   knob, and release-sum checks. It starts no engine and loads no model.
   `docs/process/VERIFICATION.md` owns the command list.
3. Open a pull request. CI runs the same gate on every pull request in the
   `gate` job. The `msrv`, `msrv-linux`, `linux`, and `deny` jobs run on every
   pull request too.
4. Follow `docs/process/REVIEW.md` for review method. Follow the
   "Commit style (D6)" section of `docs/process/GIT_WORKFLOW.md` for commit
   style.

## First task path (operator and agent sessions)

This path needs the private handoff layer in Private handoff state above.

1. Read `README.md` and `docs/process/TASKS.md`.
2. Create a task file under the handoff repo's `tasks/`.
3. Implement inside the task scope and run the gate
   (`docs/process/VERIFICATION.md`).
4. Request a review (`docs/process/REVIEW.md`).
5. Close the task file and the queue row.
6. Archive the task after the release that contains it.
