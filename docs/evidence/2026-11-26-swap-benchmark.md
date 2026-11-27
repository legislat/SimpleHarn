---
title: Swap benchmark, one card, 2026-11-26
owner: agent
status: active
audience: [user, contributor]
source_of_truth_for: [measured swap figures per engine]
last_reviewed: 2026-11-26
review_trigger: release, behavior-change
---

# Swap benchmark, one card, 2026-11-26

This page records one measured swap benchmark. It is a measurement, not a
guarantee. Figures come from one machine and one card. Treat them as an
order of magnitude for planning, not as a specification.

## What was measured

Each engine leg ran through the harness control API, one resident model at a
time, on a single 71.7 GiB card with about 69 GiB free. The build under test
was the source tree at commit `cc52771` with the tray feature, started from a
test copy of the harness. No other harness or engine served during the run.

- **Cold load**: from the quiescent state to the row resident and ready.
- **Warm swap**: row A resident, then load row B in the same single-slot
  category. The figure includes eviction of A, confirmed release of A, and the
  readiness of B.
- **Release**: from resident to the quiescent state with the engine port
  closed.

## Figures

| Engine | Cold load (s) | Warm swap (s) | Release (s) |
|---|---|---|---|
| llama | 1.3 | 9.1 | 5.0 |
| basalt | 44.6 | 10.5 | 3.0 |
| strata | 85.0 | 53.5 | 3.0 |
| maya | 67.3 | 175.7 | 3.0 |
| xolotl | 633.5 | not run | 2.9 |
| ninfer | 18.7 | 212.3 | 5.5 |

Notes on the rows:

- **llama** used two small rows (a 0.6B cold row and a 9B swap row). The cold
  figure shows the floor, not the cost of a large GGUF.
- **basalt, strata, maya, ninfer** are 26-68 GiB class loads with their
  production rows. These are the figures that matter for capacity planning.
- **xolotl** has one row on this machine, so the swap did not run. Its cold
  load took 633.5 seconds because the weights and the engram are archived on a
  hard disk. Expect minutes, not seconds, for this engine on cold storage.
- Every release finished in 2.9-5.5 seconds. A confirmed release is the state the
  next admission decision may credit.

## How to reproduce

1. Free the card. Stop every harness and engine serve. Remove every listener on
   the harness test port set.
2. Start one harness from a test copy with a configuration that points at a
   scratch model catalog. Force the benchmark rows into one single-slot
   category in that scratch catalog, so loading row B must evict row A.
3. For each leg, use the control API: `POST /har/load` with the row name, poll
   `GET /har/status` until the slot shows the row, then `POST /har/unload` and
   poll until every slot is empty and every job is terminal.
4. Treat the unload reply as an acceptance, never as a release. A release is
   the quiescent status plus the engine port closed (for a release leg), or the
   evicted engine process gone (for a swap leg; the category port is reused by
   the incoming engine).

The harness starts no engine for its own requests during the benchmark. All
timing is wall clock on the operator's console.

## What this page is not

- It is not a release card. The release card runs its own suite with its own
  legs.
- It is not a comparison across hardware. One card, one machine.
- It is not a warm-cache claim. Each cold load read the weights from local
  storage except where a note says otherwise.
