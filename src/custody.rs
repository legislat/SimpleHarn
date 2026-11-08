//! The harness-side custody sidecar for custodied engine launches (chunk 10
//! slice 10b; port of the engine-side ownership rule in `C:\Strata`'s
//! `strata_control/procs.py`, behavior not code).
//!
//! Doctrine: *the harness owns what it starts, and engine-side custody is
//! untrusted*. The strata server sweeps leftover engines at startup
//! (`serve/server.py::end_leftover_engines`) but its probe may itself fail —
//! R8's live line `[strata] could not check for engines left by an earlier
//! run: RuntimeError` — and a failed sweep says nothing at all about what is
//! still running. So the harness records what IT launched in its own sidecar
//! (next to the `ninfer-claims.json` ledger, same persistence idiom: atomic
//! temp + rename, a corrupt file warns and reads empty, never fatal) and the
//! boot reconcile acts purely on that record. Nothing here asks an engine, a
//! port, or the network who is running: the sweep is boot-stamp and
//! pid-identity only.
//!
//! The ownership-proof rule (ported from the engine's `procs.is_same`): a
//! recorded process is ours only while the host answers on the SAME boot the
//! entry was stamped on AND the pid answers with a start-time token equal to
//! the recorded token. A non-matching entry is dropped as stale —
//! the stranger at that pid is never touched. A matching orphan (a crashed
//! harness left it) is terminated once, re-verified at the instant of the
//! signal, and its entry survives a failed reap so the next boot retries.
//! NEVER kill anything the harness did not claim: absence of an entry is
//! absence of permission, and an entry naming an engine the compiled-in
//! [`crate::engine::policy`] table does not know is a forged claim, dropped
//! without action.

use crate::engine::policy;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One recorded launch in the custody sidecar. The identity triple
/// (`pid`, `start_token`) proves ownership later; `port` and `instance` are
/// the operator-facing forensics naming what the entry was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyEntry {
    /// OS process id of the launched engine.
    pub pid: u32,
    /// Start-time identity token: the process creation time as 100 ns ticks
    /// since 1601 (Windows FILETIME). Constant for one process's lifetime and
    /// different for a later process that reuses the pid — the port-reuse
    /// defense the engine-side probe applies and this harness must not skip.
    pub start_token: u64,
    /// The slot port the engine was assigned.
    pub port: u16,
    /// Instance name: the harness model name this launch served.
    pub instance: String,
    /// Engine id — must answer to a `policy::engine_policy` row, or the
    /// entry is not a claim this harness could have made.
    pub engine: String,
    /// Wall-clock ms since the epoch when the claim was written.
    pub claimed_at_ms: u64,
    /// The host boot id recorded at launch (mandatory on Linux, absent on
    /// Windows and on sidecars written before the boot stamp existed —
    /// `serde(default)` reads those old files, `skip_serializing_if` keeps the
    /// field out of the JSON when there is nothing to say). A pid and a
    /// start-time token can collide across a reboot; the boot id cannot: an
    /// entry stamped on an older boot describes a process that is definitionally
    /// gone, and acting on it is acting on a pid recycled by the NEW kernel —
    /// no permission, never a guess (doctrine 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
}

/// What the boot reconcile decided for one sidecar entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepDecision {
    /// The pid still answers as exactly the recorded process: reap it with
    /// the normal stop-grace.
    Reap,
    /// Dead, pid-reused, recorded on a stale/unknowable boot, or a forged
    /// engine id: drop the entry and leave whatever is (or is not) at that
    /// pid untouched.
    DropStale,
}

/// The pure ownership-proof rule. Re-verified at the instant of the signal,
/// exactly like the engine-side `_signal_verified` — the caller asks `os` for
/// the token again immediately before terminating.
///
/// # Arguments
///
/// * `engine_known` - whether the compiled-in policy table carries a row for
///   the entry's engine id (an entry naming a row-less engine is not a claim).
/// * `recorded` - the token written at launch.
/// * `observed` - the pid's current start token, `None` when no process (or
///   no readable one) answers.
#[must_use]
pub fn sweep_decision(engine_known: bool, recorded: u64, observed: Option<u64>) -> SweepDecision {
    if !engine_known {
        return SweepDecision::DropStale;
    }
    match observed {
        Some(token) if token == recorded => SweepDecision::Reap,
        _ => SweepDecision::DropStale,
    }
}

/// The boot-identity gate applied before the token rule, asymmetric by design:
///
/// - recorded `None` (Windows, or a sidecar written before the boot stamp
///   existed): today's token-only rule stands — the entry was never stamped,
///   so nothing about the boot can stale it.
/// - recorded `Some` and the host answers the SAME id: the entry lives on this
///   boot and the token rule below decides.
/// - recorded `Some` and the host answers a DIFFERENT id: the entry was
///   written before a reboot; whatever answers that pid now belongs to the new
///   kernel and is a stranger by definition — drop the entry, touch nothing.
/// - recorded `Some` and the host answers `None` (unreadable boot id): the
///   entry claims a boot this harness cannot confirm, so it has no more
///   permission to act than a mismatched one would — drop, never guess
///   (doctrine 4).
#[must_use]
pub fn boot_id_matches(recorded: Option<&str>, host: Option<&str>) -> bool {
    match recorded {
        None => true,
        Some(expected) => host == Some(expected),
    }
}

/// Outcome of a re-verified terminate attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminateOutcome {
    /// The pid was still the recorded process at the instant of the signal
    /// and termination was issued.
    Done,
    /// The pid is gone or belongs to a different process now: never signalled,
    /// the entry describes nothing live anymore.
    IdentityGone,
    /// The identity matched but the OS refused the terminate; the entry stays.
    Failed {
        /// OS-level explanation for the warn line.
        reason: String,
    },
}

/// The process-identity seam the boot reconcile drives through: pid
/// operations only, never engine endpoints. Tests substitute a mock; the
/// R8 falsifier depends on there being nothing else to drive — a failing
/// engine-side custody probe cannot reach this seam at all. The platform
/// implementation lives in `crate::platform`, where the identity legs
/// delegate to [`crate::platform::ProcessIdentity`].
#[async_trait]
pub trait CustodyOs: Send + Sync {
    /// The pid's current start token, `None` when dead or unreadable.
    fn start_token(&self, pid: u32) -> Option<u64>;

    /// Terminate `pid` only if it is still the recorded process at this
    /// instant (re-verifies the token inside).
    fn terminate(&self, pid: u32, recorded_token: u64) -> TerminateOutcome;

    /// The host's current boot id, `None` when this platform has no boot
    /// identity (Windows) or cannot read it. A recorded `Some` stamp meeting a
    /// `None` host acts on nothing — absence is never treated as a match
    /// ([`boot_id_matches`]).
    fn boot_id(&self) -> Option<String> {
        None
    }

    /// Whether `port` refuses TCP connections, polling until `grace` passes.
    /// The default is the pure loopback probe — identical on every platform,
    /// so the platform seam inherits it; a test mock may still script the
    /// verdict through its own implementation.
    async fn port_closed(&self, port: u16, grace: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(PORT_CLOSE_POLL).await;
        }
    }
}

/// Wall-clock ms since the epoch for the claim stamp.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The persisted custody sidecar: one entry per recorded launch, keyed by
/// pid. Written atomically (temp + rename) like the container claim ledger;
/// a missing file reads empty, a corrupt one warns and reads empty — the
/// harness never fails a boot or a launch on its own custody record, and
/// absence of evidence is absence of permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyLedger {
    entries: BTreeMap<u32, CustodyEntry>,
    path: PathBuf,
}

impl CustodyLedger {
    /// Loads the sidecar at `path`; unreadable or corrupt content becomes the
    /// empty set (warned, never fatal).
    #[must_use]
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let entries = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<BTreeMap<u32, CustodyEntry>>(&bytes) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        path = %path.display(),
                        "custody sidecar unreadable; treating every custodied launch as unrecorded"
                    );
                    BTreeMap::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    path = %path.display(),
                    "custody sidecar unreadable; treating every custodied launch as unrecorded"
                );
                BTreeMap::new()
            }
        };
        CustodyLedger { entries, path }
    }

    /// The sidecar file path this ledger persists to.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Recorded launch identities, ascending pid.
    pub fn entries(&self) -> impl Iterator<Item = &CustodyEntry> {
        self.entries.values()
    }

    /// Whether the sidecar holds no entries (the common clean-boot case).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records one started launch, persisting the sidecar. A re-claim of a
    /// reused pid replaces the old entry: the old process cannot be alive if
    /// its pid was handed out again.
    pub fn record(&mut self, entry: CustodyEntry) {
        self.entries.insert(entry.pid, entry);
        self.save();
    }

    /// Drops the entry for `pid` only when its token still matches — the
    /// engine's `drop_record(pid, token)` rule. Release clears, stale sweep
    /// drops, and the identity-gone reap all go through here, so nothing can
    /// ever erase a later launch's claim written under the same pid. Returns
    /// whether an entry was dropped.
    pub fn forget(&mut self, pid: u32, start_token: u64) -> bool {
        let matched = self
            .entries
            .get(&pid)
            .is_some_and(|entry| entry.start_token == start_token);
        if !matched {
            return false;
        }
        self.entries.remove(&pid);
        self.save();
        true
    }

    /// Writes through a temp file + rename so a crash mid-write can never
    /// truncate the ownership evidence. Best-effort, like the claim ledger.
    fn save(&self) {
        let result = (|| -> std::io::Result<()> {
            let bytes = serde_json::to_vec(&self.entries).map_err(std::io::Error::other)?;
            let temp = self.path.with_extension("json.tmp");
            std::fs::write(&temp, &bytes)?;
            std::fs::rename(&temp, &self.path)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(
                error = %error,
                path = %self.path.display(),
                "cannot persist the custody sidecar"
            );
        }
    }
}

/// What one boot reconcile did, for the single summary line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Pids whose orphan was terminated and whose port closed: entries gone.
    pub reaped: Vec<u32>,
    /// Pids dropped as dead, pid-reused, or forged: nothing was touched.
    pub stale: Vec<u32>,
    /// Pids terminated but not confirmed released within the stop-grace: the
    /// entries stay, so the next boot retries.
    pub stuck: Vec<u32>,
}

/// Boot reconcile + orphan sweep, ownership-scoped. For every sidecar entry:
/// stamped on a boot the host cannot confirm (rebooted, or boot id unreadable
/// while the entry carries a stamp) → drop the entry, never touch the pid;
/// the recorded process provably gone or pid-reused → drop the entry, never
/// touch the stranger; provably ours → terminate once (token re-verified at
/// the instant of the signal) and wait `grace` for the port to close; stuck →
/// keep the entry for the next boot. Every path warns or stays silent — this
/// never returns an error and never fails a boot (R8: even the engine's own
/// probe failing left the engine running; the harness sweeps from its OWN
/// sidecar alone, and this seam has no way to consult the engine anyway).
pub async fn reconcile(
    ledger: &Arc<Mutex<CustodyLedger>>,
    os: &dyn CustodyOs,
    grace: Duration,
) -> ReconcileReport {
    let mut report = ReconcileReport::default();
    let host_boot = os.boot_id();
    let snapshot: Vec<CustodyEntry> = ledger
        .lock()
        .expect("custody sidecar lock")
        .entries
        .values()
        .cloned()
        .collect();
    for entry in snapshot {
        if !boot_id_matches(entry.boot_id.as_deref(), host_boot.as_deref()) {
            tracing::warn!(
                pid = entry.pid,
                instance = %entry.instance,
                recorded_boot = entry.boot_id.as_deref().unwrap_or(""),
                host_boot = host_boot.as_deref().unwrap_or("unreadable"),
                "custody entry was stamped on a boot this host cannot confirm \
                 (rebooted, or the boot id cannot be read now): dropping it, \
                 touching nothing at that pid"
            );
            ledger
                .lock()
                .expect("custody sidecar lock")
                .forget(entry.pid, entry.start_token);
            report.stale.push(entry.pid);
            continue;
        }
        let known = policy::engine_policy(&entry.engine).is_some();
        match sweep_decision(known, entry.start_token, os.start_token(entry.pid)) {
            SweepDecision::DropStale => {
                if !known {
                    tracing::warn!(
                        pid = entry.pid,
                        engine = %entry.engine,
                        instance = %entry.instance,
                        "custody entry names an engine the policy table does not know: \
                         not a claim this harness could have made, dropping it"
                    );
                } else {
                    tracing::info!(
                        pid = entry.pid,
                        instance = %entry.instance,
                        "custody entry is stale (process gone or pid reused): dropping it, \
                         touching nothing at that pid"
                    );
                }
                ledger
                    .lock()
                    .expect("custody sidecar lock")
                    .forget(entry.pid, entry.start_token);
                report.stale.push(entry.pid);
            }
            SweepDecision::Reap => {
                tracing::warn!(
                    pid = entry.pid,
                    port = entry.port,
                    instance = %entry.instance,
                    "reclaiming a strata router left behind by an earlier run"
                );
                match os.terminate(entry.pid, entry.start_token) {
                    TerminateOutcome::IdentityGone => {
                        ledger
                            .lock()
                            .expect("custody sidecar lock")
                            .forget(entry.pid, entry.start_token);
                        report.stale.push(entry.pid);
                    }
                    TerminateOutcome::Failed { reason } => {
                        tracing::warn!(
                            pid = entry.pid,
                            error = %reason,
                            "cannot terminate the orphan: the claim stays for the next boot"
                        );
                        report.stuck.push(entry.pid);
                    }
                    TerminateOutcome::Done => {
                        if os.port_closed(entry.port, grace).await {
                            ledger
                                .lock()
                                .expect("custody sidecar lock")
                                .forget(entry.pid, entry.start_token);
                            report.reaped.push(entry.pid);
                        } else {
                            tracing::warn!(
                                pid = entry.pid,
                                port = entry.port,
                                grace_ms = grace.as_millis(),
                                "orphan's port still open after the stop-grace: the claim \
                                 stays for the next boot"
                            );
                            report.stuck.push(entry.pid);
                        }
                    }
                }
            }
        }
    }
    report
}

/// Poll pause while `port_closed` waits for a released port to stop accepting a
/// loopback connect within its grace. A short fixed sleep: the check is a cheap
/// connect attempt, and the deadline (not the cadence) bounds the wait.
const PORT_CLOSE_POLL: Duration = Duration::from_millis(100);

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, token: u64) -> CustodyEntry {
        CustodyEntry {
            pid,
            start_token: token,
            port: 18_109,
            instance: "strata-model".to_string(),
            engine: "strata".to_string(),
            claimed_at_ms: 1,
            boot_id: None,
        }
    }

    /// The ownership-proof rule, pinned as a matrix. The R8 core: the ONLY
    /// way an entry authorizes a kill is pid-alive AND start-time match; every
    /// other observation drops the entry and touches nothing.
    #[test]
    fn only_a_token_match_authorizes_a_reap() {
        assert_eq!(
            sweep_decision(true, 77, Some(77)),
            SweepDecision::Reap,
            "alive and provably the recorded process: ours to reap"
        );
        assert_eq!(
            sweep_decision(true, 77, Some(78)),
            SweepDecision::DropStale,
            "the pid answers with a different start time: a stranger, never killed blind"
        );
        assert_eq!(
            sweep_decision(true, 77, None),
            SweepDecision::DropStale,
            "nothing lives at the pid: drop the stale record"
        );
        assert_eq!(
            sweep_decision(false, 77, Some(77)),
            SweepDecision::DropStale,
            "an entry naming an engine the table does not know is a forged claim: even a \
             token match must not authorize a kill"
        );
    }

    /// R8 falsifier harness: a mock *process* runtime — pid ops only, no
    /// engine endpoint exists to consult — so a failing engine-side custody
    /// probe (its `RuntimeError` path) has no channel into this sweep.
    #[derive(Default)]
    struct MockOs {
        live: std::sync::Mutex<BTreeMap<u32, u64>>,
        terminated: std::sync::Mutex<Vec<u32>>,
        port_open: std::sync::Mutex<bool>,
        boot: std::sync::Mutex<Option<String>>,
    }

    impl MockOs {
        fn new(live: &[u32]) -> Self {
            let mut map = BTreeMap::new();
            for pid in live {
                // Mock token convention: pid * 10 (entries below use it).
                map.insert(*pid, u64::from(*pid) * 10);
            }
            MockOs {
                live: std::sync::Mutex::new(map),
                terminated: std::sync::Mutex::new(Vec::new()),
                port_open: std::sync::Mutex::new(false),
                boot: std::sync::Mutex::new(None),
            }
        }

        /// Scripts the host boot id the reconcile sees (default `None`: a
        /// platform with no readable boot stamp).
        fn with_boot(self, boot: Option<&str>) -> Self {
            *self.boot.lock().expect("mock boot lock") = boot.map(str::to_string);
            self
        }
    }

    #[async_trait]
    impl CustodyOs for MockOs {
        fn start_token(&self, pid: u32) -> Option<u64> {
            self.live.lock().expect("mock live lock").get(&pid).copied()
        }

        fn boot_id(&self) -> Option<String> {
            self.boot.lock().expect("mock boot lock").clone()
        }

        fn terminate(&self, pid: u32, recorded_token: u64) -> TerminateOutcome {
            let mut live = self.live.lock().expect("mock live lock");
            match live.get(&pid).copied() {
                Some(token) if token == recorded_token => {
                    live.remove(&pid);
                    self.terminated.lock().expect("mock term lock").push(pid);
                    TerminateOutcome::Done
                }
                Some(_) | None => TerminateOutcome::IdentityGone,
            }
        }

        async fn port_closed(&self, _port: u16, _grace: Duration) -> bool {
            !*self.port_open.lock().expect("mock port lock")
        }
    }

    fn seeded(path: &std::path::Path, entries: &[CustodyEntry]) -> Arc<Mutex<CustodyLedger>> {
        let mut ledger = CustodyLedger::load(path);
        for entry in entries {
            ledger.record(entry.clone());
        }
        Arc::new(Mutex::new(ledger))
    }

    /// The R8 falsifier proper: boot-reconcile driven with sidecar entries
    /// and a mock process runtime sweeps purely from the harness's OWN
    /// record — the live orphan is reaped, the pid-reused stranger and the
    /// dead entry are dropped untouched, a forged engine id is dropped even
    /// with a live matching pid, and the persisted sidecar reflects it.
    #[tokio::test]
    async fn boot_reconcile_sweeps_purely_from_the_harness_sidecar() {
        let dir = crate::testutil::temp_dir("custody-r8");
        let path = dir.join("strata-custody.json");
        let ours = entry(4001, 40_010);
        let stranger = entry(4002, 123); // token != pid*10: a stranger lives now
        let gone = entry(4003, 40_030); // pid absent from the mock live set
        let forged = entry(4004, 40_040); // live and matching, but the engine id is bogus
        let mut forged = forged;
        forged.engine = "not-an-engine".to_string();
        let ledger = seeded(&path, &[ours.clone(), stranger, gone, forged]);
        let os = MockOs::new(&[4001, 4002, 4004]);

        let report = reconcile(&ledger, &os, Duration::from_millis(50)).await;

        assert_eq!(report.reaped, vec![4001], "only the provably-ours orphan");
        assert_eq!(
            report.stale,
            vec![4002, 4003, 4004],
            "stranger, dead, and forged entries drop without any action at the pid"
        );
        assert!(report.stuck.is_empty());
        assert_eq!(
            *os.terminated.lock().expect("mock term lock"),
            vec![4001],
            "the harness terminated exactly the process it claimed — never a stranger"
        );
        let reloaded = CustodyLedger::load(&path);
        assert!(reloaded.is_empty(), "every swept entry left the sidecar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stale/garbage tolerance pin: a corrupt sidecar warns and reads
    /// empty; reconcile against it is a no-op — never a boot failure.
    #[tokio::test]
    async fn a_garbage_sidecar_warns_and_drops_but_never_fails_boot() {
        let dir = crate::testutil::temp_dir("custody-garbage");
        let path = dir.join("strata-custody.json");
        std::fs::write(&path, b"{ not json at all").expect("write garbage");
        let ledger = Arc::new(Mutex::new(CustodyLedger::load(&path)));
        assert!(ledger.lock().expect("lock").is_empty());
        let os = MockOs::new(&[4001]);
        let report = reconcile(&ledger, &os, Duration::from_millis(50)).await;
        assert_eq!(report, ReconcileReport::default());
        assert!(
            os.terminated.lock().expect("mock term lock").is_empty(),
            "a garbage file authorizes no kill"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stuck reap (port still open after the stop-grace) keeps the claim
    /// for the next boot instead of silently orphaning.
    #[tokio::test]
    async fn a_stuck_reap_keeps_the_claim_for_the_next_boot() {
        let dir = crate::testutil::temp_dir("custody-stuck");
        let path = dir.join("strata-custody.json");
        let ours = entry(4101, 41_010);
        let ledger = seeded(&path, std::slice::from_ref(&ours));
        let os = MockOs::new(&[4101]);
        *os.port_open.lock().expect("mock port lock") = true;

        let report = reconcile(&ledger, &os, Duration::from_millis(10)).await;

        assert_eq!(report.stuck, vec![4101]);
        assert!(report.reaped.is_empty());
        assert!(
            CustodyLedger::load(&path).entries().any(|e| e.pid == 4101),
            "an unconfirmed reap stays reclaimable"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ledger's own rules: a release can only forget the entry it wrote
    /// (token-checked), and the round-trip survives reload exactly — the boot
    /// path sees precisely what the launch path wrote.
    #[test]
    fn custody_ledger_round_trips_and_token_guards_its_forgets() {
        let dir = crate::testutil::temp_dir("custody-ledger");
        let path = dir.join("strata-custody.json");
        let ours = entry(4201, 42_010);
        let mut ledger = CustodyLedger::load(&path);
        ledger.record(ours.clone());
        assert!(
            !ledger.forget(4201, 999),
            "forgetting under a foreign token is refused"
        );
        assert!(ledger.entries().any(|e| e.pid == 4201));
        drop(ledger);
        let mut reloaded = CustodyLedger::load(&path);
        assert_eq!(
            reloaded.entries().cloned().collect::<Vec<_>>(),
            vec![ours.clone()],
            "reload sees exactly what was written"
        );
        assert!(reloaded.forget(4201, 42_010));
        assert!(CustodyLedger::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The boot gate as a pinned matrix: a recorded stamp needs the host to
    /// answer the SAME id; a missing stamp (Windows, old sidecar) never gains
    /// a boot obligation; a missing host answer never matches a present stamp.
    #[test]
    fn the_boot_stamp_gate_matches_only_an_equal_stamp_and_never_guesses() {
        assert!(
            boot_id_matches(None, None),
            "an unstamped entry keeps today's token-only rule"
        );
        assert!(
            boot_id_matches(None, Some("host")),
            "a host boot answer cannot stale an entry that was never stamped"
        );
        assert!(
            boot_id_matches(Some("boot-a"), Some("boot-a")),
            "same boot: the entry lives and the token rule decides"
        );
        assert!(
            !boot_id_matches(Some("boot-a"), Some("boot-b")),
            "another boot: the pid belongs to the new kernel, never ours to touch"
        );
        assert!(
            !boot_id_matches(Some("boot-a"), None),
            "an unreadable host boot id gives no permission to act on a stamped entry"
        );
    }

    /// The sidecar JSON contract: an absent stamp is omitted from the bytes
    /// (a Linux write and the unchanged Windows shape both hold), and an old
    /// file without the field still deserializes (`serde(default)`).
    #[test]
    fn the_sidecar_omits_an_absent_boot_id_and_reads_an_old_file() {
        let unstamped = serde_json::to_string(&entry(5001, 50_010)).expect("serializes");
        assert!(
            !unstamped.contains("boot_id"),
            "an absent stamp must never enter the JSON: {unstamped}"
        );
        let stamped = CustodyEntry {
            boot_id: Some("boot-9".to_string()),
            ..entry(5002, 50_020)
        };
        let bytes = serde_json::to_string(&stamped).expect("serializes");
        assert!(bytes.contains(r#""boot_id":"boot-9""#), "{bytes}");
        let pre_stamp_file = r#"{"5003":{"pid":5003,"start_token":50030,"port":18109,
            "instance":"old","engine":"strata","claimed_at_ms":7}}"#;
        let loaded: BTreeMap<u32, CustodyEntry> =
            serde_json::from_str(pre_stamp_file).expect("an old sidecar without boot_id loads");
        assert_eq!(
            loaded.get(&5003).expect("entry").boot_id,
            None,
            "the old shape reads as unstamped, not as a corrupt file"
        );
    }

    /// A sidecar written before the boot stamp existed still drives the sweep
    /// through the token rule even though the host now answers a boot id:
    /// recorded None + host Some is "no boot obligation", not a mismatch.
    #[tokio::test]
    async fn an_unstamped_sidecar_keeps_the_token_rule_on_a_boot_aware_host() {
        let dir = crate::testutil::temp_dir("custody-boot-old");
        let path = dir.join("strata-custody.json");
        let ours = entry(4301, 43_010);
        assert!(ours.boot_id.is_none());
        let ledger = seeded(&path, std::slice::from_ref(&ours));
        let os = MockOs::new(&[4301]).with_boot(Some("host-boot-1"));

        let report = reconcile(&ledger, &os, Duration::from_millis(50)).await;

        assert_eq!(
            report.reaped,
            vec![4301],
            "the pre-stamp entry is judged by the token rule alone"
        );
        assert!(CustodyLedger::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stamp the host no longer answers is a reboot: every entry it covers
    /// drops as stale and NOTHING at those pids is touched — the token would
    /// otherwise match a pid the new kernel just handed out.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_recorded_boot_id_the_host_no_longer_answers_drops_everything() {
        let dir = crate::testutil::temp_dir("custody-boot-stale");
        let path = dir.join("strata-custody.json");
        let ours = CustodyEntry {
            boot_id: Some("old-boot".to_string()),
            ..entry(4311, 43_110)
        };
        let ledger = seeded(&path, std::slice::from_ref(&ours));
        let os = MockOs::new(&[4311]).with_boot(Some("new-boot"));

        let report = reconcile(&ledger, &os, Duration::from_millis(50)).await;

        assert_eq!(report.stale, vec![4311]);
        assert!(report.reaped.is_empty());
        assert!(
            os.terminated.lock().expect("mock term lock").is_empty(),
            "a reboot-stamped entry never authorizes a signal"
        );
        assert!(CustodyLedger::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recorded Some + host None: the entry claims a boot the harness cannot
    /// confirm, so it acts on NOTHING — while an unstamped entry in the same
    /// reconcile keeps its full token-rule authority (the asymmetry, pinned).
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_host_boot_id_lets_a_stamped_entry_act_on_nothing() {
        let dir = crate::testutil::temp_dir("custody-boot-unknown");
        let path = dir.join("strata-custody.json");
        let stamped = CustodyEntry {
            boot_id: Some("boot-at-launch".to_string()),
            ..entry(4321, 43_210)
        };
        let unstamped = entry(4322, 43_220);
        let ledger = seeded(&path, &[stamped, unstamped.clone()]);
        let os = MockOs::new(&[4321, 4322]); // boot seam answers None

        let report = reconcile(&ledger, &os, Duration::from_millis(50)).await;

        assert_eq!(
            report.stale,
            vec![4321],
            "the stamped entry drops, never reaps"
        );
        assert_eq!(
            report.reaped,
            vec![4322],
            "the unstamped entry is still judged by the token rule"
        );
        assert_eq!(
            *os.terminated.lock().expect("mock term lock"),
            vec![4322],
            "the stamped pid was never signalled"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The matching case on the seam: same boot id, live token — the orphan is
    /// reaped exactly as the unstamped rule always did.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_matching_host_boot_id_reaps_the_orphan_as_before() {
        let dir = crate::testutil::temp_dir("custody-boot-match");
        let path = dir.join("strata-custody.json");
        let ours = CustodyEntry {
            boot_id: Some("boot-now".to_string()),
            ..entry(4331, 43_310)
        };
        let ledger = seeded(&path, std::slice::from_ref(&ours));
        let os = MockOs::new(&[4331]).with_boot(Some("boot-now"));

        let report = reconcile(&ledger, &os, Duration::from_millis(50)).await;

        assert_eq!(report.reaped, vec![4331]);
        assert!(CustodyLedger::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
