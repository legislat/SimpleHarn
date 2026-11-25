//! Admission arithmetic property tests (TASK-ADMISSION-PROPTEST, review #8).
//!
//! The conformance target `tests/admission_conformance.rs` states the five admission
//! invariants once each on fixed figures. This target drives the SAME seams — the
//! scripted witness, the real scheduler actor, and the in-process mock engine — over
//! seeded pseudo-random scenario families, so every leg is checked at points the
//! fixed tests never visit. Every property assertion computes its expected outcome
//! from a documented oracle, never from the code under test, so a mutated expression
//! turns the leg red the way `scripts/seed-admission-regression.ps1` proves for the
//! conformance target.
//!
//! Scope and limits:
//!
//! - Cardless: no port bind, no engine start, no model load, no GPU, no new
//!   dependency. Runs under plain `cargo test --features tray`.
//! - The scripted witness is a projection, never hardware truth. This target never
//!   replaces the operator-run card suite.
//! - Figures are integer quarter GiB (a multiple of 0.25). Every f64 stays exact in
//!   binary, so the oracle compares with equality, not epsilon, and the `{:.1}`
//!   figures quoted in refusal messages are pinned exactly.
//! - Open finding reported with this task (do NOT read the two negative-figure legs
//!   as "gap closed"): the gate refuses NaN and infinite readings (fail closed,
//!   DOCTRINE item 4), and a negative total refuses as a shortage, but a FINITE
//!   negative `used_gib` is not refused today — `finish_admit` subtracts the raw
//!   reading (`src/scheduler/admission.rs` `free = total - reserve - occupied + ...`),
//!   so a garbage negative column inflates the compared figure instead of refusing.
//!   The leg pins that the figure flows through the formula arithmetically (never a
//!   silent clamp); a follow-up gate guard is an orchestrator decision.
//!
//! Seeded PRNG (fixed, reproducible, no entropy source, no clock):
//!
//! - xorshift64* (Vigna, 2003): `x ^= x >> 12; x ^= x << 25; x ^= x >> 27;`
//!   output `x * 0x2545F4914F6CDD1D`. State must never be zero; the base seed is not.
//! - Base seed: `0x5EED_2026_1125_0007`. Each test takes one stream; a stream mixes
//!   the test index with the golden-ratio constant `0x9E37_79B9_7F4A_7C15`, so a
//!   whole run replays identically and one test's draws cannot shift another's.

mod mock_runtime;

use async_trait::async_trait;
use mock_runtime::MockRuntime;
use simpleharn::budget::{
    free_gb, need_gb, AdmissionError, AttributionQuality, Gibibytes, MemorySample, VramWitness,
};
use simpleharn::config::{AppConfig, CategoryId};
use simpleharn::engine::mock::MockAdapter;
use simpleharn::engine::{EngineAdapter, EngineRuntime};
use simpleharn::scheduler::{
    JobStatus, Reply, Scheduler, SchedulerHandle, SchedulerStatus, SwapState,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Base seed for every stream in this file. Fixed by design (module doc).
const SEED: u64 = 0x5EED_2026_1125_0007;
/// Golden-ratio mixing constant for per-test streams.
const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// xorshift64* — see the module doc for the algorithm and the seeds. A stream is a
/// function of the fixed base seed and the fixed stream index: nothing reads the
/// clock, the pid, or an entropy source, so the whole file replays value for value.
struct Rng(u64);

impl Rng {
    fn new(stream: u64) -> Self {
        Rng(SEED ^ stream.wrapping_add(1).wrapping_mul(MIX))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// One f64 drawn uniformly from the quarter GiB grid, inclusive: the raw draw is
    /// an integer count of 0.25 GiB steps, so the quotient is exact in binary.
    fn quarters(&mut self, lo_q: u64, hi_q: u64) -> f64 {
        let span = hi_q - lo_q + 1;
        (lo_q + self.next_u64() % span) as f64 / 4.0
    }
}

/// Exact quarter/half grid helpers (never a float literal in an expected figure).
fn q(quarter_steps: i64) -> f64 {
    quarter_steps as f64 / 4.0
}

/// Scripted witness: one constant sample, replaced atomically by the test. The
/// override wins every read, so the fold loop, the admit sample, and the settle
/// window all observe the same scripted card. Same contract as the `FakeWitness`
/// override arm in `tests/admission_conformance.rs`.
#[derive(Clone)]
struct FakeWitness {
    current: Arc<Mutex<MemorySample>>,
}

impl FakeWitness {
    fn new(total: f64, used: f64) -> Self {
        FakeWitness {
            current: Arc::new(Mutex::new(sample(total, used))),
        }
    }

    fn set(&self, total: f64, used: f64) {
        *self.current.lock().expect("witness lock") = sample(total, used);
    }
}

#[async_trait]
impl VramWitness for FakeWitness {
    fn source(&self) -> String {
        "fake-prop".to_string()
    }

    async fn sample(&self) -> Result<MemorySample, String> {
        Ok(self.current.lock().expect("witness lock").clone())
    }
}

fn sample(total: f64, used: f64) -> MemorySample {
    MemorySample {
        total_gib: total,
        used_gib: used,
        processes: vec![],
        attribution: AttributionQuality::Unavailable,
    }
}

/// Starts the scheduler over the mock runtime with raw model yaml per model, so a
/// scenario can carry the `cost:` / `launch_only_with_at_least_vram_free:` keys
/// (`tests/admission_conformance.rs` `harness`). Returns the runtime handle too: the
/// kill/spawn counts falsify every "refused before any spawn" claim.
fn harness(
    cfg: AppConfig,
    specs: &[(&str, &str)],
    witness: FakeWitness,
) -> (SchedulerHandle, MockRuntime, PathBuf) {
    let runtime = MockRuntime::new();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert("mock".to_string(), Arc::new(MockAdapter));
    let dir = simpleharn::testutil::temp_dir("admission-prop");
    for (name, yaml) in specs {
        std::fs::write(dir.join(format!("{name}.yaml")), yaml).expect("model yaml");
    }
    let handle = Scheduler::start(
        cfg,
        dir.clone(),
        adapters,
        Arc::new(runtime.clone()) as Arc<dyn EngineRuntime>,
        Arc::new(witness),
    );
    (handle, runtime, dir)
}

fn row(name: &str, category: &str, vram_gb: f64, extra: &str) -> String {
    format!("name: {name}\nengine: mock\ncategory: {category}\nvram_gb: {vram_gb}\n{extra}")
}

async fn wait_until(handle: &SchedulerHandle, secs: u64, cond: impl Fn(&SchedulerStatus) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let status = handle.status().await.expect("actor alive");
        if cond(&status) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "condition not met within {secs}s; last status: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The newest ledger row of `model`.
fn job_row<'a>(status: &'a SchedulerStatus, model: &str) -> &'a JobStatus {
    status
        .jobs
        .iter()
        .rev()
        .find(|job| job.model == model)
        .unwrap_or_else(|| panic!("`{model}` must be in the job ledger: {:?}", status.jobs))
}

/// The measured pair currently on a catalog entry (the fold's persisted view).
async fn measured_of(handle: &SchedulerHandle, model: &str) -> (Option<f64>, Option<f64>) {
    let catalog = handle.catalog().await.expect("actor alive");
    catalog
        .models
        .iter()
        .find(|m| m.name == model)
        .map_or((None, None), |m| (m.measured_gb, m.measured_min_gb))
}

async fn await_measured(handle: &SchedulerHandle, model: &str, want: (Option<f64>, Option<f64>)) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = measured_of(handle, model).await;
        if got == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "range for {model} never reached {want:?}: got {got:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Oracle for the victim planner, stating the documented rule independently of the
/// implementation: exhaustive over subsets; a subset covers when its sum plus the
/// planner epsilon reaches the deficit; prefer the least sum, then the fewest
/// victims, then the most disposable set (the sum of `protection`, how many
/// categories may evict each victim); a full tie keeps the earlier candidate order.
/// Mirrors the ruling the planner doc carries (`plan_victims`).
fn cheapest_cover(candidates: &[(String, f64, usize)], deficit: f64) -> Option<Vec<usize>> {
    let n = candidates.len();
    let mut best: Option<(f64, usize, usize, u32)> = None;
    for mask in 1u32..(1u32 << n) {
        let (mut sum, mut count, mut disposability) = (0.0f64, 0usize, 0usize);
        for (bit, (_, cost, protection)) in candidates.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                sum += cost;
                count += 1;
                disposability += protection;
            }
        }
        if sum + 1e-9 < deficit {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_sum, best_count, best_pro, _)) => {
                sum < best_sum
                    || (sum <= best_sum + 1e-9
                        && (count < best_count
                            || (count == best_count && disposability > best_pro)))
            }
        };
        if better {
            best = Some((sum, count, disposability, mask));
        }
    }
    best.map(|(_, _, _, mask)| (0..n).filter(|bit| mask & (1 << bit) != 0).collect())
}

/// The gate's own compared figure, stated as the doctrine formula (DOCTRINE item 4):
/// `total - reserve - observed_used` with an empty card state (no victim, no debit).
fn plain_free(total: f64, reserve: f64, used: f64) -> f64 {
    total - reserve - used
}

// ---------- pure arithmetic legs (no scheduler; the budget module in isolation) --

/// Property: the budget subtraction is exactly `total - reserve - occupied` for
/// every seeded quarter-GiB triple — including the over-commit shapes where the
/// figure goes negative (documented: negative is information, the gate compares it,
/// it never errors). The witness clamp is `max(total - used, 0)` — used above total
/// reads zero free, never a fake negative — and `freed_for` reaches the target
/// exactly at `free`, exactly at `free + tolerance`, and never a half GiB past it.
///
/// Falsifier: flip any sign or operator in `free_gb`, drop the `max(0.0)` clamp, or
/// turn a `>=` in `freed_for` into `>` and a seeded leg goes red.
#[test]
fn the_free_gb_formula_and_the_clamp_hold_over_seeded_quarter_gib_triples() {
    let mut rng = Rng::new(0);
    for _ in 0..400 {
        let total = rng.quarters(1, 288);
        let reserve = rng.quarters(0, 16);
        let occupied = rng.quarters(0, 400);
        let expected = (total * 4.0 - reserve * 4.0 - occupied * 4.0) / 4.0;
        let got = free_gb(Gibibytes(total), Gibibytes(reserve), Gibibytes(occupied));
        assert_eq!(
            got.0, expected,
            "total {total} reserve {reserve} occupied {occupied}"
        );

        // The witness clamp and the settle band, same seeded pair shape.
        let used = rng.quarters(0, 320);
        let s = sample(total, used);
        let free = if total > used { total - used } else { 0.0 };
        assert_eq!(s.free_gib(), free, "clamp for {total}/{used}");
        let tol = rng.quarters(0, 16);
        // Reaching the target exactly passes; tolerance carries target == free + tol;
        // a half GiB past the tolerance band must not.
        assert!(
            s.freed_for(free, tol),
            "exact reach at free {free} tol {tol}"
        );
        assert!(
            s.freed_for(free + tol, tol),
            "the tolerance edge itself must clear at free {free} tol {tol}"
        );
        assert!(
            !s.freed_for(free + tol + q(2), tol),
            "beyond the one-sided band must refuse at free {free} tol {tol}"
        );
    }
}

/// Property: the cost ladder books the declared `vram_gb` exactly when it exists;
/// with only a GGUF size it books `size * 1.15 + 2` (the oracle spells the ruled
/// constants once, so a silent constant drift turns it red); with neither it is
/// `UnderSpecified` — never a guess.
#[test]
fn need_gb_books_declared_or_the_gguf_ladder_over_seeded_sizes() {
    let mut rng = Rng::new(1);
    for _ in 0..300 {
        let declared = rng.quarters(1, 292);
        let other = rng.quarters(1, 292);
        let got = need_gb(Some(Gibibytes(declared)), Some(Gibibytes(other)));
        assert_eq!(got.expect("declared always books").0, declared);

        let size = rng.quarters(1, 292);
        let est = need_gb(None, Some(Gibibytes(size)));
        assert_eq!(
            est.expect("a size always estimates").0,
            size * 1.15 + 2.0,
            "size {size}"
        );
    }
    assert_eq!(need_gb(None, None), Err(AdmissionError::UnderSpecified));
}

// ---------- gate legs: garbage readings must fail closed (DOCTRINE item 4) ------

/// Property: every non-finite figure the witness can hand the gate — NaN, +inf,
/// -inf on either `total_gib` or `used_gib` — is a named refusal before any spawn.
/// A non-finite figure makes `free` non-finite, and the gate names it
/// `witness unavailable ... unreadable witness reading`; it never compares garbage
/// and never spawns. Order of shapes is PRNG-shuffled from the fixed stream.
///
/// Falsifier: drop the `is_finite` guard in `finish_admit` and a garbage comparison
/// reads "fits" — the spawn count and the refusal text both go red.
#[tokio::test]
async fn every_non_finite_witness_figure_refuses_at_the_gate_before_any_spawn() {
    let shapes: [(f64, f64); 6] = [
        (f64::NAN, q(30)),
        (q(288), f64::NAN),
        (f64::INFINITY, q(30)),
        (f64::NEG_INFINITY, q(30)),
        (q(288), f64::INFINITY),
        (q(288), f64::NEG_INFINITY),
    ];
    let mut order: Vec<usize> = (0..shapes.len()).collect();
    let mut rng = Rng::new(2);
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }

    let witness = FakeWitness::new(q(288), q(30));
    let cfg = AppConfig {
        settle_timeout_s: 2,
        ..AppConfig::default()
    };
    let (handle, runtime, _dir) = harness(
        cfg,
        &[(
            "garbage",
            &format!(
                "name: garbage\nengine: mock\ncategory: WORK\nvram_gb: {}\n",
                q(16)
            ),
        )],
        witness.clone(),
    );

    for index in order {
        let (total, used) = shapes[index];
        witness.set(total, used);
        let error = handle
            .swap("garbage", CategoryId::Work)
            .await
            .expect_err("a non-finite reading is not a figure");
        let message = error.to_string();
        assert!(
            message.contains("witness unavailable")
                && message.contains("unreadable witness reading"),
            "shape {index} must be the fail-closed witness refusal, was: {message}"
        );
        let status = handle.status().await.expect("actor alive");
        let job = job_row(&status, "garbage");
        assert!(job.terminal && job.state == SwapState::AdmitFail, "{job:?}");
        assert!(
            status.slots.iter().all(|slot| slot.model.is_none()),
            "refusal must leave every slot empty (shape {index})"
        );
        assert_eq!(runtime.spawn_count(), 0, "no garbage reading may spawn");
    }
}

/// Property: a finite negative `total_gib` is not a budget — `free` turns negative
/// (finite), so the load is refused as a shortage (`insufficient VRAM`) naming the
/// oracle-computed figure, and nothing spawns. The gate fails closed on a refusal,
/// so the product posture (a refusal that names its numbers) still holds.
#[tokio::test]
async fn a_negative_total_reading_refuses_as_a_shortage_not_a_pass() {
    let mut rng = Rng::new(3);
    let total = -rng.quarters(32, 160); // -8.0 ..= -40.0 GiB
    let reserve = 2.0; // the shipped default, left untouched by this leg
    let used = rng.quarters(2, 26);
    let need = 4.0;
    let free = plain_free(total, reserve, used);
    assert!(free < 0.0, "seeded shape must read negative: {free}");

    let witness = FakeWitness::new(total, used);
    let cfg = AppConfig {
        settle_timeout_s: 2,
        // The oracle below reads exactly this reserve; pin it, never inherit.
        vram_reserve_gb: reserve,
        ..AppConfig::default()
    };
    let (handle, runtime, _dir) = harness(
        cfg,
        &[(
            "shortrow",
            &format!("name: shortrow\nengine: mock\ncategory: WORK\nvram_gb: {need}\n"),
        )],
        witness,
    );

    let error = handle
        .swap("shortrow", CategoryId::Work)
        .await
        .expect_err("a negative card total is no budget");
    let message = error.to_string();
    assert!(
        message.contains(&format!(
            "need {need:.1} GiB, free {free:.1} GiB, reserve {reserve:.1} GiB"
        )),
        "the shortage must name the oracle-computed figures: {message}"
    );
    let status = handle.status().await.expect("actor alive");
    let job = job_row(&status, "shortrow");
    assert_eq!(job.free_at_admit_gib, Some(free), "{job:?}");
    assert_eq!(job.outcome.as_deref(), Some("admission refused"), "{job:?}");
    assert_eq!(runtime.spawn_count(), 0);
    assert!(status.slots.iter().all(|slot| slot.model.is_none()));
}

/// Property (the review #8 "negative" leg — see the module doc for the open gap):
/// a FINITE negative `used_gib` is not refused at the VRAM gate today. The leg
/// pins the weaker contract that actually holds and that a fix must not weaken: the
/// garbage figure flows through the doctrine formula arithmetically — the compared
/// figure is exactly `total - reserve + |used|`, the observed figure exactly
/// `total + |used|` — and is never silently clamped into a flattering number. An
/// implementation that DOES grow the promised fail-closed refusal is also accepted
/// here (the named shapes below), so this leg stays green across the follow-up and
/// red only if garbage ever passes uncounted.
#[tokio::test]
async fn a_finite_negative_used_reading_never_quietly_clamps_at_the_gate() {
    let mut rng = Rng::new(4);
    let total = q(288); // 72.0
    let reserve = 2.0;
    let negative = rng.quarters(2, 48); // |used| in 0.5 ..= 12.0
    let used = -negative;
    let need = plain_free(total, reserve, 0.0) + negative / 2.0; // above the honest budget, inside the inflated one
    let inflated_free = plain_free(total, reserve, used);

    let witness = FakeWitness::new(total, used);
    let cfg = AppConfig {
        settle_timeout_s: 2,
        // The oracle below reads exactly this reserve; pin it, never inherit.
        vram_reserve_gb: reserve,
        ..AppConfig::default()
    };
    let (handle, runtime, _dir) = harness(
        cfg,
        &[(
            "negativeused",
            &format!("name: negativeused\nengine: mock\ncategory: WORK\nvram_gb: {need}\n"),
        )],
        witness,
    );

    match handle.swap("negativeused", CategoryId::Work).await {
        Ok(reply) => {
            let Reply::Loaded(receipt) = reply else {
                panic!("expected Loaded");
            };
            assert_eq!(receipt.model, "negativeused");
            let status = handle.status().await.expect("actor alive");
            let job = job_row(&status, "negativeused");
            assert_eq!(
                job.free_at_admit_gib,
                Some(inflated_free),
                "the compared figure must be the raw formula over the garbage reading, never a clamp: {job:?}"
            );
            assert_eq!(
                job.observed_at_admit_gib,
                Some(total + negative),
                "the observed figure is the clamped-only-if-negative accessor: {job:?}"
            );
            assert_eq!(runtime.spawn_count(), 1);
        }
        Err(error) => {
            // Accepted future shape: a named fail-closed refusal.
            let message = error.to_string();
            assert!(
                message.contains("insufficient VRAM")
                    || message.contains("witness unavailable")
                    || message.contains("launch floor not met"),
                "a refusal must be a named gate shape: {message}"
            );
            assert_eq!(runtime.spawn_count(), 0, "refused before spawn");
        }
    }
}

// ---------- gate legs: the reserve boundary ------------------------------------

/// Property: on the empty card the budget is exactly `total - reserve`. A load
/// needing exactly that figure is admitted (the deficit is `need - free` and only a
/// figure strictly above zero plans; zero fits); a load a half GiB above it is
/// refused naming the exact compared figures and spawns nothing; a load a half GiB
/// below it is admitted. Seeded card sizes; quarter figures throughout.
///
/// Falsifier: flip the deficit test `deficit > 0.0` to `>= 0.0` and the exact leg
/// flips to a refusal; drop the reserve subtraction and the above leg loads.
#[tokio::test]
async fn the_reserve_boundary_admits_at_exact_room_and_refuses_a_half_gib_above() {
    let mut rng = Rng::new(5);
    for stream in 0..3 {
        let total = rng.quarters(192, 330); // 48.0 ..= 82.5 GiB
        let reserve = rng.quarters(0, 20); // 0.0 ..= 5.0 GiB
        let free = plain_free(total, reserve, 0.0);
        let exact = free;
        let above = free + q(2);
        let below = free - q(2);
        assert!(
            below > 0.0,
            "seeded card must keep a legal below leg: {below}"
        );

        // Scheduler A: the over-room load refuses on the empty card, then the
        // under-room load still gets in (the refusal left nothing behind).
        let witness = FakeWitness::new(total, 0.0);
        let cfg = AppConfig {
            settle_timeout_s: 5,
            vram_reserve_gb: reserve,
            ..AppConfig::default()
        };
        let (handle, runtime, _dir) = harness(
            cfg.clone(),
            &[
                (
                    "above",
                    &format!("name: above\nengine: mock\ncategory: WORK\nvram_gb: {above}\n"),
                ),
                (
                    "below",
                    &format!("name: below\nengine: mock\ncategory: WORK\nvram_gb: {below}\n"),
                ),
            ],
            witness.clone(),
        );
        let error = handle
            .swap("above", CategoryId::Work)
            .await
            .expect_err("a half GiB over the budget must refuse");
        let message = error.to_string();
        assert!(
            message.contains(&format!(
                "insufficient VRAM: need {above:.1} GiB, free {free:.1} GiB, reserve {reserve:.1} GiB"
            )),
            "stream {stream}: the refusal must quote the compared figures: {message}"
        );
        let status = handle.status().await.expect("actor alive");
        assert_eq!(job_row(&status, "above").free_at_admit_gib, Some(free));
        assert_eq!(runtime.spawn_count(), 0, "a refusal never spawns");

        let Reply::Loaded(receipt) = handle
            .swap("below", CategoryId::Work)
            .await
            .expect("a half GiB under the budget loads after the refusal")
        else {
            panic!("expected Loaded");
        };
        assert_eq!(receipt.model, "below");
        assert_eq!(runtime.spawn_count(), 1);

        // Scheduler B: the exact-boundary load fits (deficit zero).
        let witness_b = FakeWitness::new(total, 0.0);
        let (handle_b, runtime_b, _dir_b) = harness(
            cfg,
            &[(
                "exact",
                &format!("name: exact\nengine: mock\ncategory: WORK\nvram_gb: {exact}\n"),
            )],
            witness_b,
        );
        let Reply::Loaded(receipt) = handle_b
            .swap("exact", CategoryId::Work)
            .await
            .expect("exactly the budget is not over the budget")
        else {
            panic!("expected Loaded");
        };
        assert_eq!(receipt.model, "exact");
        let status = handle_b.status().await.expect("actor alive");
        assert_eq!(job_row(&status, "exact").free_at_admit_gib, Some(free));
        assert_eq!(runtime_b.spawn_count(), 1);
    }
}

// ---------- gate legs: booking debit, victim credit, subset plan, dead floor ----

/// Property: a booking is a debit. While a swap holds the settle window, its need
/// rides `free` as a live debit, so a second load that would fit the physical card
/// is parked — not admitted over bytes already committed — with the compared figure
/// exactly `total - reserve - used - booking` (negative here: information, refused
/// as a wait). The park never strands: when the booking terminalizes the parked row
/// re-gates and loads. Seeded booking/occupancy figures; the oracle pre-derives the
/// park arithmetic (`deficit <= booking` is what parks).
///
/// Falsifier: delete the `- in_flight` debit and the parked load admits early — the
/// slot-empty / spawn-count pins go red.
#[tokio::test]
async fn an_in_flight_booking_debits_the_budget_so_the_second_load_parks_then_loads() {
    for stream in 0..2u64 {
        // (a0 nominal, a1 booking X, scripted occupancy U, b need) — all quarter
        // figures. Seeds: (10.0, 18.0, 58.0, 11.5) and (10.0, 24.0, 56.0, 13.5).
        let a0 = 10.0;
        let booking = if stream == 0 { 18.0 } else { 24.0 };
        let used = if stream == 0 { 58.0 } else { 56.0 };
        let need_b = if stream == 0 { 11.5 } else { 13.5 };
        let total = 72.0;
        let reserve = 2.0;

        // Oracle (the gate formula over the scripted state):
        // a1's admit reads the card credited for its 10.0 victim: must reach booking.
        let a1_free = plain_free(total, reserve, used) + a0;
        assert!(
            a1_free >= booking,
            "seed {stream}: a1 must admit: {a1_free}"
        );
        // b's admit: no credit, debit the live booking.
        let b_free = plain_free(total, reserve, used) - booking;
        let b_deficit = need_b - b_free;
        assert!(b_deficit > 0.0, "seed {stream}: b must be short");
        assert!(
            b_deficit <= booking,
            "seed {stream}: a deficit the live booking covers is a park, not a death"
        );

        let witness = FakeWitness::new(total, used);
        let cfg = AppConfig {
            settle_timeout_s: 30,
            // The oracle below reads exactly this reserve; pin it, never inherit.
            vram_reserve_gb: reserve,
            ..AppConfig::default()
        };
        let (handle, runtime, _dir) = harness(
            cfg,
            &[
                (
                    "a0",
                    &format!("name: a0\nengine: mock\ncategory: WORK\nvram_gb: {a0}\n"),
                ),
                (
                    "a1",
                    &format!("name: a1\nengine: mock\ncategory: WORK\nvram_gb: {booking}\n"),
                ),
                (
                    "b",
                    &format!("name: b\nengine: mock\ncategory: RP\nvram_gb: {need_b}\n"),
                ),
            ],
            witness.clone(),
        );

        handle.swap("a0", CategoryId::Work).await.expect("a0 loads");
        let swapping = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.swap("a1", CategoryId::Work).await })
        };
        wait_until(&handle, 10, |status| {
            status
                .jobs
                .iter()
                .any(|job| job.model == "a1" && job.state == SwapState::Dispose)
        })
        .await;

        let waiting = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.swap("b", CategoryId::Rp).await })
        };
        // The park: b's row carries the compared figure including the debit.
        wait_until(&handle, 10, |status| {
            status
                .jobs
                .iter()
                .rev()
                .any(|job| job.model == "b" && job.free_at_admit_gib == Some(b_free))
        })
        .await;
        for _ in 0..12 {
            assert!(
                !waiting.is_finished(),
                "seed {stream}: the debit must hold b out"
            );
            let status = handle.status().await.expect("actor alive");
            assert!(
                status
                    .slots
                    .iter()
                    .all(|slot| slot.model.as_deref() != Some("b")),
                "seed {stream}: b must never occupy a slot while the booking is live"
            );
            let job = job_row(&status, "b");
            assert_eq!(job.need_gib, Some(need_b));
            assert!(
                !job.terminal,
                "a covered deficit is a park, not a death: {job:?}"
            );
            assert_eq!(
                runtime.spawn_count(),
                1,
                "seed {stream}: neither a1 nor b may spawn over the live debit"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let status = handle.status().await.expect("actor alive");
        assert_eq!(job_row(&status, "a1").need_gib, Some(booking));

        // The card frees: a1 finishes its window, the unload terminalizes its row,
        // and the funnel re-gates the parked row onto the released card.
        witness.set(total, 40.0);
        let Reply::Loaded(receipt) = swapping
            .await
            .expect("task alive")
            .expect("a1 loads once the card frees")
        else {
            panic!("expected Loaded");
        };
        assert_eq!(receipt.model, "a1");
        handle.unload_model("a1").await.expect("unload accepted");
        let Reply::Loaded(receipt) = tokio::time::timeout(Duration::from_secs(15), waiting)
            .await
            .expect("b re-gates once a1's booking terminalizes")
            .expect("task alive")
            .expect("b loads after the release")
        else {
            panic!("expected Loaded");
        };
        assert_eq!(receipt.model, "b");
        assert_eq!(runtime.spawn_count(), 3, "a0, a1, b — nothing refused");
    }
}

/// Property: the same-slot release credit is `max(booking, measured footprint)`,
/// clamped to the occupied figure (DOCTRINE 12: a projection is not a booking, and
/// a release frees what the resident physically holds). Seeded families pick a
/// greedy host that books `b` while its own fold witnesses `m > b` on the card, and
/// a replacement whose launch floor `F` sits between the two credit shapes:
/// `max(N, F) <= free_with_max` but `F - tolerance > free_with_min`. Crediting the
/// booking alone puts `F` past every release the guard may count and the row must
/// die at the no-release guard before any kill — so this test's Loaded expectation
/// is exactly the leg the `max -> min` mutation (the regression
/// `scripts/seed-admission-regression.ps1` plants) turns red.
#[tokio::test]
async fn a_same_slot_victim_is_credited_at_max_booking_measured_over_seeded_figures() {
    for stream in 0..2u64 {
        // (total, reserve, host booking b, card occupancy U = measured m, floor F,
        //  need N, post-release occupancy). Halves throughout.
        let (total, reserve, booking, used, floor, need, after) = if stream == 0 {
            (72.0, 2.0, 12.0, 64.5, 55.0, 66.0, 2.0)
        } else {
            (56.5, 2.5, 8.0, 48.5, 42.0, 44.5, 2.0)
        };
        // Oracle: free at max credit and at the (wrong) booking-only credit.
        let free_max = plain_free(total, reserve, used) + used; // credit = m = used
        let free_min = plain_free(total, reserve, used) + booking;
        assert!(
            floor <= free_max,
            "seed {stream}: the max credit must clear the floor"
        );
        assert!(
            floor - 2.0 > free_min,
            "seed {stream}: the booking-only credit must make the floor unreachable (tolerance 2.0)"
        );

        let witness = FakeWitness::new(total, 0.0);
        let cfg = AppConfig {
            settle_timeout_s: 10,
            measured_sample_s: 1,
            vram_reserve_gb: reserve,
            ..AppConfig::default()
        };
        let (handle, runtime, _dir) = harness(
            cfg,
            &[
                ("host", &row("host", "WORK", booking, "cost: greedy\n")),
                (
                    "repl",
                    &row(
                        "repl",
                        "WORK",
                        need,
                        &format!("cost: greedy\nlaunch_only_with_at_least_vram_free: {floor}\n"),
                    ),
                ),
            ],
            witness.clone(),
        );
        let Reply::Loaded(host) = handle
            .swap("host", CategoryId::Work)
            .await
            .expect("the host loads onto the empty card")
        else {
            panic!("expected Loaded");
        };
        // The host's own footprint: the fold witnesses the card filled to `used`.
        witness.set(total, used);
        await_measured(&handle, "host", (Some(used), Some(used))).await;

        let loading = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.swap("repl", CategoryId::Work).await })
        };
        wait_until(&handle, 15, |status| {
            status
                .jobs
                .iter()
                .any(|job| job.model == "repl" && job.state == SwapState::Dispose)
        })
        .await;
        witness.set(total, after);
        let Reply::Loaded(receipt) = loading.await.expect("task alive").unwrap_or_else(|_| {
            panic!("seed {stream}: the replacement loads on the measured credit, not the booking")
        }) else {
            panic!("expected Loaded");
        };
        assert_eq!(receipt.model, "repl");

        let status = handle.status().await.expect("actor alive");
        assert_eq!(
            status
                .slots
                .iter()
                .find(|slot| slot.category == CategoryId::Work)
                .and_then(|slot| slot.model.as_deref()),
            Some("repl")
        );
        let job = job_row(&status, "repl");
        assert_eq!(job.gate_target_gib, Some(floor), "the floor was the gate");
        assert!(
            job.victims_released.is_empty(),
            "a same-slot replacement is not an eviction: {:?}",
            job.victims_released
        );
        assert!(
            runtime.is_dead(host.port).await,
            "the credited release killed the host engine"
        );
        assert_eq!(
            runtime.server_count(),
            1,
            "only the replacement remains live"
        );
        assert_eq!(runtime.spawn_count(), 2);
    }
}

/// Property: the eviction planner releases exactly the cheapest sufficient subset.
/// Oracle: the documented exhaustive rule (see `cheapest_cover`) over the seeded
/// candidate bookings. Three seeded random families of three AUX residents — two
/// with a covering deficit, one deliberately short of every subset so the refusal
/// names every considered candidate and kills nothing — plus one fixed tie case: RP
/// and AUX hold 10.0 each, either alone covers, the costs tie, and the more
/// disposable set (AUX: evicted by WORK and RP) must be the one released.
///
/// Falsifier: make the planner greedy-by-slot-order and a seeded subset or the
/// disposability tie diverges — the released-set or survivor assertion goes red.
#[tokio::test]
async fn the_planner_releases_exactly_the_cheapest_sufficient_victim_subset() {
    for stream in 0..4u64 {
        let mut rng = Rng::new(9 + stream);
        let total = 72.0;
        let reserve = 2.0;

        let sizes: Vec<f64>;
        let deficit;
        let rp_tie = stream == 3;
        if rp_tie {
            // Fixed tie case: RP 10.0 + AUX 10.0, deficit exactly 10.0.
            sizes = vec![10.0]; // the AUX resident; the RP resident is seeded below
            deficit = 10.0;
        } else {
            sizes = vec![
                rng.quarters(32, 64), // 8.0 ..= 16.0
                rng.quarters(32, 64),
                rng.quarters(32, 64),
            ];
            let sum: f64 = sizes.iter().sum();
            deficit = if stream == 0 {
                rng.quarters(1, (sum * 4.0) as u64) // guaranteed coverable
            } else if stream == 1 {
                sum + rng.quarters(4, 32) // guaranteed un-coverable
            } else {
                rng.quarters(1, (sum * 4.0) as u64 + 32) // either leg
            };
        }
        let used = if rp_tie {
            20.0
        } else {
            sizes.iter().sum::<f64>()
        };
        let need = plain_free(total, reserve, used) + deficit;
        assert!(need > 0.0, "seeded deficit must keep a legal need");

        // Oracle candidate list, in slot-table order (WORK, RP, AUX, EMB):
        // the tie case lists RP (protection 1) before AUX (protection 2).
        let candidates: Vec<(String, f64, usize)> = if rp_tie {
            vec![
                ("rp10".to_string(), 10.0, 1),
                ("aux10".to_string(), 10.0, 2),
            ]
        } else {
            sizes
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("aux{}", i + 1), *s, 2))
                .collect()
        };
        let plan = cheapest_cover(&candidates, deficit);

        let mut cfg = AppConfig {
            settle_timeout_s: 10,
            // The oracle below reads exactly this reserve; pin it, never inherit.
            vram_reserve_gb: reserve,
            ..AppConfig::default()
        };
        cfg.category_mut(CategoryId::Aux)
            .expect("default config has AUX")
            .max_models = 3;
        let mut specs: Vec<(String, String)> = Vec::new();
        if rp_tie {
            specs.push(("rp10".to_string(), row("rp10", "RP", 10.0, "")));
        }
        for (name, size, _) in candidates.iter().filter(|(n, _, _)| n.starts_with("aux")) {
            specs.push((name.clone(), row(name, "AUX", *size, "")));
        }
        specs.push(("work".to_string(), row("work", "WORK", need, "")));
        let spec_refs: Vec<(&str, &str)> = specs
            .iter()
            .map(|(name, yaml)| (name.as_str(), yaml.as_str()))
            .collect();

        let witness = FakeWitness::new(total, used);
        let (handle, runtime, _dir) = harness(cfg, &spec_refs, witness.clone());
        for (name, _) in &specs {
            if name == "work" {
                continue;
            }
            let category = if name.starts_with("rp") {
                CategoryId::Rp
            } else {
                CategoryId::Aux
            };
            handle.swap(name, category).await.unwrap_or_else(|e| {
                panic!("resident {name} must load onto the scripted card: {e}")
            });
        }

        match plan {
            None => {
                let error = handle
                    .swap("work", CategoryId::Work)
                    .await
                    .expect_err("no candidate set covers the deficit");
                let message = error.to_string();
                assert!(
                    message.contains(&format!(
                        "insufficient VRAM: need {need:.1} GiB, free {:.1} GiB, reserve {reserve:.1} GiB",
                        plain_free(total, reserve, used)
                    )),
                    "seed {stream}: the shortage must name the compared figures: {message}"
                );
                for (name, _, _) in &candidates {
                    assert!(
                        message.contains(name),
                        "the refusal must consider {name}: {message}"
                    );
                }
                let status = handle.status().await.expect("actor alive");
                let job = job_row(&status, "work");
                assert!(job.terminal && job.victims_released.is_empty(), "{job:?}");
                assert_eq!(
                    runtime.server_count(),
                    candidates.len(),
                    "an unplanable shortage kills nobody"
                );
                assert_eq!(runtime.spawn_count(), candidates.len() as u32);
            }
            Some(bits) => {
                let loaded = {
                    let handle = handle.clone();
                    tokio::spawn(async move { handle.swap("work", CategoryId::Work).await })
                };
                wait_until(&handle, 10, |status| {
                    status
                        .jobs
                        .iter()
                        .any(|job| job.model == "work" && job.state == SwapState::Dispose)
                })
                .await;
                let evicted: Vec<String> =
                    bits.iter().map(|bit| candidates[*bit].0.clone()).collect();
                let survivors: Vec<String> = (0..candidates.len())
                    .filter(|bit| !bits.contains(bit))
                    .map(|bit| candidates[bit].0.clone())
                    .collect();
                let survivors_used: f64 = survivors
                    .iter()
                    .map(|name| {
                        candidates
                            .iter()
                            .find(|(n, _, _)| n == name)
                            .expect("survivor is a candidate")
                            .1
                    })
                    .sum();
                // The scripted card after every planned release: only the survivors.
                witness.set(total, survivors_used);
                let Reply::Loaded(receipt) =
                    loaded.await.expect("task alive").unwrap_or_else(|_| {
                        panic!("seed {stream}: the planned victims release and the load lands")
                    })
                else {
                    panic!("expected Loaded");
                };
                assert_eq!(receipt.model, "work");

                let status = handle.status().await.expect("actor alive");
                let mut released = job_row(&status, "work").victims_released.clone();
                released.sort();
                let mut expected = evicted.clone();
                expected.sort();
                assert_eq!(
                    released, expected,
                    "seed {stream}: the ledger must carry exactly the oracle's set"
                );
                for name in &survivors {
                    assert!(
                        status
                            .slots
                            .iter()
                            .any(|slot| slot.model.as_deref() == Some(name.as_str())),
                        "seed {stream}: {name} is not in the released set and must survive"
                    );
                }
                assert_eq!(
                    runtime.server_count(),
                    survivors.len() + 1,
                    "seed {stream}: survivors plus the new load, nothing else"
                );
                assert_eq!(runtime.spawn_count(), candidates.len() as u32 + 1);
            }
        }
    }
}

/// Property: when no release the harness may perform reaches the launch floor, the
/// gate refuses BEFORE any kill and names the guard. Seeded resident/foreign
/// shapes where the replacement's floor exceeds `free + every candidate + tolerance`
/// (the same reach the guard computes). Oracle pins: the refusal text with the exact
/// compared figures, the terminal "admission refused" row carrying the floor as its
/// gate target, an empty `victims_released`, zero extra spawns, and the resident
/// engine still alive.
///
/// Falsifier: move the guard after the release arm and the resident's engine is
/// dead before the refusal — `is_dead` and the spawn count go red.
#[tokio::test]
async fn an_unreachable_launch_floor_refuses_before_any_kill_over_seeded_figures() {
    for stream in 0..2u64 {
        // (total, reserve, holder booking, foreign occupancy, floor, need).
        let (total, reserve, holder_vram, foreign, floor, need) = if stream == 0 {
            (64.5, 1.5, 20.5, 26.0, 45.5, 30.0)
        } else {
            (56.5, 2.0, 18.0, 20.0, 40.5, 33.0)
        };
        // Oracle: at replacement time the card holds holder + foreign; the
        // same-slot credit gives the holder booking back.
        let free = plain_free(total, reserve, foreign + holder_vram) + holder_vram;
        assert_eq!(free, plain_free(total, reserve, foreign), "credit is exact");
        assert!(
            need <= free,
            "seed {stream}: the need itself must fit (the floor must bind)"
        );
        assert!(
            floor - 2.0 > free,
            "seed {stream}: the floor must be past every release the guard may count (tolerance 2.0)"
        );

        let witness = FakeWitness::new(total, foreign);
        let cfg = AppConfig {
            settle_timeout_s: 2,
            vram_reserve_gb: reserve,
            ..AppConfig::default()
        };
        let (handle, runtime, _dir) = harness(
            cfg,
            &[
                ("holder", &row("holder", "WORK", holder_vram, "")),
                (
                    "floored",
                    &row(
                        "floored",
                        "WORK",
                        need,
                        &format!("launch_only_with_at_least_vram_free: {floor}\n"),
                    ),
                ),
            ],
            witness.clone(),
        );
        let Reply::Loaded(holder) = handle
            .swap("holder", CategoryId::Work)
            .await
            .expect("the resident loads onto the card foreign-only")
        else {
            panic!("expected Loaded");
        };
        // The card after install: the holder's bytes plus foreign occupancy the
        // harness may never release.
        witness.set(total, foreign + holder_vram);

        let error = handle
            .swap("floored", CategoryId::Work)
            .await
            .expect_err("no release reaches the floor: the guard must refuse");
        let message = error.to_string();
        assert!(
            message.contains(&format!(
                "launch floor not met: at least {floor:.1} GiB free required, free {free:.1} GiB, reserve {reserve:.1} GiB"
            )),
            "seed {stream}: the refusal must quote the floor and the compared figure: {message}"
        );
        assert!(
            message.contains("No releases allow for the VRAM floor. Check foreign residents"),
            "seed {stream}: the guard sentence is the actionable fact: {message}"
        );

        let status = handle.status().await.expect("actor alive");
        assert_eq!(
            status
                .slots
                .iter()
                .find(|slot| slot.category == CategoryId::Work)
                .and_then(|slot| slot.model.as_deref()),
            Some("holder"),
            "the resident a release could not save stays installed"
        );
        let job = job_row(&status, "floored");
        assert!(job.terminal, "{job:?}");
        assert_eq!(job.outcome.as_deref(), Some("admission refused"), "{job:?}");
        assert_eq!(job.gate_target_gib, Some(floor), "{job:?}");
        assert_eq!(job.free_at_admit_gib, Some(free), "{job:?}");
        assert!(job.victims_released.is_empty(), "{job:?}");
        assert_eq!(runtime.spawn_count(), 1, "the refused load never spawned");
        assert!(
            !runtime.is_dead(holder.port).await,
            "no kill ever reached the resident's engine"
        );
    }
}
