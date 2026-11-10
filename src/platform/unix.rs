//! The Linux implementations of the platform traits: the pid-identity token
//! from `/proc/<pid>/stat`, the session-group kill group, the descendant walk
//! over `/proc`, the resident-set reader, the `flock` single-instance guard,
//! and the host-RAM sample.
//!
//! The seam matches the Windows one but the guarantees differ by design, and
//! the difference is stated here rather than implied:
//!
//! - A spawned engine becomes a session and process-group leader at birth:
//!   `pre_exec` calls `setsid()`, so one `kill(-pgid)` reaches the whole tree
//!   that stays inside the session. No new crate is used for it — the C call
//!   is declared here, as are the others.
//! - There is no kernel job object on this target, and that difference is
//!   REAL, not papered over: `setsid` stops a signal aimed at the harness's
//!   group from reaching an engine, but it does not fence the tree — a
//!   descendant that itself calls `setsid()` or `prctl(PR_SET_PDEATHSIG)`-
//!   free reparenting escapes the group kill forever, exactly as a Win32
//!   grandchild born before the job assignment escaped `KILL_ON_JOB_CLOSE`.
//!   The PRIMARY reap on this target is therefore the custody sidecar's
//!   pid-identity sweep at the next boot (token + boot stamp), not the group
//!   kill; the group kill is the fast path for the tree that stayed.
//! - `flock` is advisory and per lock file: it refuses a second harness by the
//!   same lock file, not by a named kernel object. The file lives under
//!   `$XDG_RUNTIME_DIR` (falling back to a uid-suffixed name in the temp dir),
//!   where the kernel wipes it at logout/reboot, so a crashed harness cannot
//!   leave a lock no one can clear.
//!
//! No new crate is introduced for the C calls: the few needed functions are
//! declared here, and every call site is documented unsafe.

use super::{ProcessIdentity, ProcessTree, SingleInstance};
use crate::custody::{CustodyOs, TerminateOutcome};
use async_trait::async_trait;
use std::io::ErrorKind;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicI32, Ordering};

/// The only signal this module sends.
const SIGKILL: i32 = 9;

/// `flock` operations: exclusive, non-blocking.
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn flock(fd: i32, operation: i32) -> i32;
    fn setsid() -> i32;
    fn getuid() -> u32;
}

/// Splits a raw `/proc/<pid>/stat` line after the `comm` field. The `comm`
/// field is parenthesized and may itself contain spaces and parentheses, so
/// the walk starts after the LAST `)`: the next field is state (field 3).
/// Parsing the raw text is separated from reading the file so the rule is
/// assertable without a live process.
fn stat_tail_from(text: &str) -> Option<Vec<String>> {
    let close = text.rfind(')')?;
    let rest = text.get(close + 1..)?;
    Some(rest.split_whitespace().map(str::to_owned).collect())
}

fn stat_tail(pid: u32) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat_tail_from(&text)
}

/// The start token carried by a raw stat line: field 22 (starttime, clock
/// ticks since boot). Same for one process's whole lifetime, different for a
/// later process that reuses the pid. `None` for a zombie/dead state or an
/// unparseable field — never a guess.
fn start_token_from_stat(text: &str) -> Option<u64> {
    let fields = stat_tail_from(text)?;
    if matches!(fields.first().map(String::as_str), Some("Z" | "X" | "x")) {
        return None;
    }
    fields.get(19)?.parse().ok()
}

/// The start token of a live pid, read from `/proc`; see
/// [`start_token_from_stat`] for the rule. `None` for pid 0, a dead/zombie
/// process, or an unreadable stat.
fn live_start_token(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    start_token_from_stat(&text)
}

/// ALL descendants of `root`, at every depth, from one `/proc` walk, bounded
/// at [`crate::process::DESCENDANT_CAP`]. A parent cycle cannot spin the walk
/// (visited set), the root never enters the answer, and every failure answers
/// an empty Vec: absence is reported, never a guess.
#[must_use]
pub fn descendant_pids_of(root: u32) -> Vec<u32> {
    use crate::process::DESCENDANT_CAP;
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let Some(fields) = stat_tail(pid) else {
            continue;
        };
        let Some(parent) = fields.get(1).and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        children_of.entry(parent).or_default().push(pid);
    }
    children_of
        .values_mut()
        .for_each(|list| list.sort_unstable());

    let mut seen: HashSet<u32> = HashSet::new();
    seen.insert(root);
    let mut queue: VecDeque<u32> = VecDeque::new();
    queue.push_back(root);
    let mut descendants: Vec<u32> = Vec::new();
    while let Some(parent) = queue.pop_front() {
        let Some(children) = children_of.get(&parent) else {
            continue;
        };
        for &child in children {
            if descendants.len() >= DESCENDANT_CAP {
                return descendants;
            }
            if seen.insert(child) {
                descendants.push(child);
                queue.push_back(child);
            }
        }
    }
    descendants
}

/// Resident-set bytes of `pid` from `VmRSS` in `/proc/<pid>/status`; `None`
/// means the process is gone or unreadable and nothing else.
#[must_use]
pub fn working_set_bytes_of(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kib = text
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kib * 1024)
}

/// Host RAM in bytes from `/proc/meminfo`: `(total, available)`. `MemAvailable`
/// is the kernel's own estimate; an absent one (very old kernels) falls back to
/// `MemFree`, and a zero total answers `None` so the caller omits the bar.
#[must_use]
pub fn host_ram_bytes() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib = |key: &str| -> Option<u64> {
        text.lines()
            .find_map(|line| line.strip_prefix(key))?
            .trim()
            .strip_suffix("kB")?
            .trim()
            .parse::<u64>()
            .ok()
    };
    let total = kib("MemTotal:")?.checked_mul(1024)?;
    let available = kib("MemAvailable:")
        .or_else(|| kib("MemFree:"))?
        .checked_mul(1024)?;
    if total == 0 {
        return None;
    }
    Some((total, available))
}

/// Hides a short-lived control child. No console exists on this target, so the
/// seam is a no-op that keeps the call site portable.
pub fn hide_window(_command: &mut tokio::process::Command) {}

/// Whether this process owns a console window a person can actually see. A
/// Unix terminal has no hidden-console launcher (the Windows `.vbs` case does
/// not exist here), so the terminal test alone decides.
#[must_use]
pub fn console_window_visible() -> bool {
    true
}

/// The host boot id: the kernel's per-boot uuid, trimmed. `None` when `/proc`
/// cannot answer — the custody gate treats that as absence of permission for
/// a stamped entry, never as a match.
#[must_use]
pub fn host_boot_id() -> Option<String> {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The one-instance lock path: `$XDG_RUNTIME_DIR/simpleharn.lock` when the
/// runtime dir is set, else a uid-suffixed temp path so two different users
/// on one machine never collide on a world-writable temp dir, while a second
/// harness for the SAME user still hits the SAME file. Kept out of
/// [`SingleInstanceGuard::acquire`] so the naming rule is assertable without
/// touching the process environment.
fn lock_file_path(xdg_runtime_dir: Option<std::ffi::OsString>) -> std::path::PathBuf {
    if let Some(dir) = xdg_runtime_dir.filter(|dir| !dir.is_empty()) {
        return std::path::PathBuf::from(dir).join("simpleharn.lock");
    }
    // Safety: `getuid` is a pure syscall; it cannot fail and takes no state.
    let uid = unsafe { getuid() };
    std::env::temp_dir().join(format!("simpleharn-{uid}.lock"))
}

/// The flock-backed single-instance guard. Dropping the guard closes the file,
/// which releases the lock.
pub struct SingleInstanceGuard {
    _lock: std::fs::File,
}

impl SingleInstance for SingleInstanceGuard {
    fn acquire() -> Result<Self, String> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        let path = lock_file_path(std::env::var_os("XDG_RUNTIME_DIR"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        // Safety: `file` owns the descriptor and lives for the guard's lifetime.
        let locked = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if locked == 0 {
            return Ok(SingleInstanceGuard { _lock: file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == ErrorKind::WouldBlock {
            return Err("another SimpleHarn instance is already running".to_string());
        }
        Err(format!("cannot lock {}: {error}", path.display()))
    }
}

/// The session-group kill group: one group per owned engine tree. The group id
/// is the child's own pid, installed by `setsid()` in the spawn's `pre_exec`,
/// which makes the child a process-group (and session) leader at birth.
pub struct KillGroup {
    pgid: AtomicI32,
}

impl KillGroup {
    fn terminate(&self) {
        let pgid = self.pgid.load(Ordering::SeqCst);
        if pgid <= 0 {
            return;
        }
        // Safety: `pgid` is the group of a child this harness spawned and
        // sealed; killing it is the documented seam. A gone group answers an
        // error that the caller ignores, exactly like the Windows arm.
        unsafe {
            let _ = kill(-pgid, SIGKILL);
        }
    }
}

impl Drop for KillGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// The live [`ProcessTree`]: the own-group spawn, the group kill, the `/proc`
/// walk, and the resident-set reader.
pub struct UnixProcessTree;

impl ProcessTree for UnixProcessTree {
    fn prepare_engine_command(&self, command: &mut tokio::process::Command) {
        // Own session AND process group at birth: one `kill(-pid)` reaches the
        // tree that stayed, and no signal aimed at the harness group ever
        // reaches an engine. `pre_exec` runs between fork and exec, where the
        // child is guaranteed NOT to be a group leader (it carries the
        // harness's pgid and its pid is new), so `setsid()` always succeeds in
        // giving it a session with `pgid == pid` for [`Self::seal`] to store.
        // The escape reality is documented in the module header and accepted
        // by design: a descendant that itself calls `setsid()` leaves the
        // group for good — no kernel fence exists on this target — so the
        // PRIMARY reap is the custody sidecar's pid-identity sweep at the next
        // boot, and the group kill is the fast path for the tree that stayed.
        //
        // Safety: `pre_exec` requires the closure to be async-signal-safe:
        // `setsid()` is (POSIX-listed), takes no lock, allocates nothing, and
        // must not unwind — `last_os_error` on failure and an `Err` return
        // fail the spawn loudly instead, so the child is never exec'd left in
        // the harness's own group.
        unsafe {
            command.pre_exec(|| {
                // Safety: async-signal-safe per above; no shared state touched.
                if setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    fn create_kill_group(&self) -> Result<KillGroup, String> {
        Ok(KillGroup {
            pgid: AtomicI32::new(0),
        })
    }

    fn seal(&self, group: &KillGroup, child: &tokio::process::Child) -> Result<(), String> {
        let pid = child
            .id()
            .filter(|pid| *pid != 0)
            .ok_or_else(|| "the spawned client reported no usable pid".to_string())?;
        group.pgid.store(pid as i32, Ordering::SeqCst);
        Ok(())
    }

    fn terminate_group(&self, group: &KillGroup) {
        group.terminate();
    }

    fn descendants(&self, root: u32) -> Vec<u32> {
        descendant_pids_of(root)
    }

    fn working_set_bytes(&self, pid: u32) -> Option<u64> {
        working_set_bytes_of(pid)
    }
}

/// The live [`ProcessIdentity`]: the starttime of `/proc/<pid>/stat`, terminate
/// only after the token re-verifies.
pub struct UnixProcessIdentity;

impl ProcessIdentity for UnixProcessIdentity {
    fn start_token(&self, pid: u32) -> Option<u64> {
        live_start_token(pid)
    }

    fn start_token_of_spawned(&self, child: &tokio::process::Child) -> Option<u64> {
        child.id().and_then(live_start_token)
    }

    fn terminate_verified(&self, pid: u32, recorded_token: u64) -> TerminateOutcome {
        if pid == 0 {
            return TerminateOutcome::IdentityGone;
        }
        match live_start_token(pid) {
            Some(token) if token == recorded_token => {
                // Safety: identity re-verified immediately above with the same
                // token rule the sweep uses; pid 0 and dead processes never
                // reach here.
                let sent = unsafe { kill(pid as i32, SIGKILL) };
                if sent == 0 {
                    TerminateOutcome::Done
                } else {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() == Some(3) {
                        // ESRCH: the process exited between the check and the
                        // signal. Nothing to report, nothing was signalled.
                        TerminateOutcome::IdentityGone
                    } else {
                        TerminateOutcome::Failed {
                            reason: error.to_string(),
                        }
                    }
                }
            }
            _ => TerminateOutcome::IdentityGone,
        }
    }
}

/// The custody sweep drives its seam through [`CustodyOs`]; the port leg is the
/// trait's platform-neutral loopback default.
#[async_trait]
impl CustodyOs for UnixProcessIdentity {
    fn start_token(&self, pid: u32) -> Option<u64> {
        ProcessIdentity::start_token(self, pid)
    }

    fn terminate(&self, pid: u32, recorded_token: u64) -> TerminateOutcome {
        ProcessIdentity::terminate_verified(self, pid, recorded_token)
    }

    fn boot_id(&self) -> Option<String> {
        host_boot_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw stat line whose `comm` carries BOTH spaces and nested
    /// parentheses — the field `rfind(')')` exists to survive. The tail must
    /// split after the LAST `)` (state, ppid, ..., starttime), and a line
    /// without any `)` is no answer at all.
    #[test]
    fn a_stat_line_is_split_after_the_last_paren_even_with_spaces_inside_comm() {
        let tail_fields: Vec<String> = ["S", "4242"]
            .iter()
            .map(|v| (*v).to_string())
            .chain((1..=18).map(|i| i.to_string()))
            .collect();
        // index 19 of the tail is field 22 (starttime).
        let line = format!("1234 (engine (weird) proc) {}", tail_fields.join(" "));
        let tail = stat_tail_from(&line).expect("splits after the last paren");
        assert_eq!(tail[0], "S", "state follows the LAST close paren");
        assert_eq!(tail[1], "4242", "ppid parses from the shifted tail");
        assert_eq!(tail.len(), 20);
        assert_eq!(
            start_token_from_stat(&line),
            tail[19].parse::<u64>().ok(),
            "field 22 (starttime) is the token"
        );
        assert_eq!(
            start_token_from_stat("99 (p) Z 5 5 5 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 77"),
            None,
            "a zombie state answers nothing, whatever its starttime reads"
        );
        assert!(
            stat_tail_from("no paren here").is_none(),
            "a line without a close paren is no answer, never a guess"
        );
    }

    /// The port-reuse defense against a live stranger: the CURRENT pid answers
    /// its real token, so a recorded token that is not it must yield
    /// `IdentityGone` — and the test process being alive afterwards is the
    /// proof that nothing was signalled.
    #[test]
    fn a_live_pid_with_the_wrong_recorded_token_is_identity_gone_never_signalled() {
        let me = std::process::id();
        assert!(
            live_start_token(me).is_some(),
            "the test process has a token"
        );
        assert_eq!(
            UnixProcessIdentity.terminate_verified(me, u64::MAX),
            TerminateOutcome::IdentityGone,
            "a reused pid never receives the terminate: the token gate stands"
        );
        assert_eq!(
            UnixProcessIdentity.terminate_verified(0, 1),
            TerminateOutcome::IdentityGone,
            "pid 0 is the process-group shorthand, never a target"
        );
        assert!(
            std::fs::metadata(format!("/proc/{me}/stat")).is_ok(),
            "and the test process is still alive: the wrong token sent no signal"
        );
    }

    /// The kernel's per-boot uuid: present and trimmed on Linux; the custody
    /// gate relies on it never carrying trailing newline noise.
    #[test]
    fn the_host_boot_id_is_the_trimmed_kernel_uuid() {
        let boot = host_boot_id().expect("/proc/sys/kernel/random/boot_id is readable on Linux");
        assert!(!boot.is_empty());
        assert_eq!(boot.trim(), boot, "the stamp is written trimmed");
        assert!(
            std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .expect("readable")
                .trim()
                == boot
        );
    }

    /// The lock-path rule: `$XDG_RUNTIME_DIR/simpleharn.lock` when set, else
    /// the uid-suffixed temp name — asserted on the pure naming rule, with no
    /// environment mutation racing other tests.
    #[test]
    fn the_lock_path_is_xdg_runtime_dir_else_the_uid_suffixed_temp_name() {
        use std::ffi::OsString;
        assert_eq!(
            lock_file_path(Some(OsString::from("/run/user/1000"))),
            std::path::PathBuf::from("/run/user/1000/simpleharn.lock")
        );
        // Safety: pure syscall, cannot fail.
        let uid = unsafe { getuid() };
        for unset in [None, Some(OsString::new())] {
            let path = lock_file_path(unset);
            assert_eq!(
                path.file_name().expect("named").to_string_lossy(),
                format!("simpleharn-{uid}.lock"),
                "same-user harnesses collide on one file by design"
            );
            assert_eq!(path.parent().expect("in temp dir"), std::env::temp_dir());
        }
    }

    /// The flock refusal is per lock FILE and per holder: a second acquire is
    /// refused with the "already running" line, and a new acquire succeeds
    /// once the first guard drops (closing the descriptor releases the lock).
    #[test]
    fn the_flock_guard_refuses_a_second_holder_then_releases_on_drop() {
        let first = SingleInstanceGuard::acquire().expect("the first holder acquires");
        let second = SingleInstanceGuard::acquire();
        match second {
            Err(reason) => assert!(
                reason.contains("already running"),
                "a held lock must refuse with the second-harness line: {reason}"
            ),
            Ok(_) => panic!("flock must refuse a second exclusive holder"),
        }
        drop(first);
        let _reacquired =
            SingleInstanceGuard::acquire().expect("dropping the guard released the lock");
    }

    /// The walk over the live tree: a child this test spawned appears under
    /// the current process, the current process never appears in its own set,
    /// and an absent pid answers empty. The stand-in is owned by this test, so
    /// reaping it is this test's to do.
    #[test]
    fn the_descendant_walk_lists_my_child_and_never_the_root() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("/bin/sleep stands in for an engine child");
        let me = std::process::id();
        let found = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let listed = descendant_pids_of(me);
                assert!(
                    !listed.contains(&me),
                    "the root never enters its own descendant set: {listed:?}"
                );
                if listed.contains(&child.id()) {
                    break true;
                }
                if std::time::Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        };
        let _ = child.kill();
        let _ = child.wait();
        assert!(found, "the spawned child must appear under its own parent");
        assert!(
            descendant_pids_of(u32::MAX).is_empty(),
            "a pid with no descendants has none, never a guess"
        );
    }

    /// The resident-set reader against the current process: the test binary
    /// holds pages, so a real nonzero number must come back — and a pid that
    /// cannot exist answers `None`, never a zero measurement.
    #[test]
    fn the_working_set_of_the_current_process_is_a_real_number() {
        let me = std::process::id();
        let bytes = working_set_bytes_of(me).expect("the test process has a resident set");
        assert!(bytes > 0, "a live process never measures as zero bytes");
        assert_eq!(
            working_set_bytes_of(u32::MAX),
            None,
            "an absent process is absence, not a measurement"
        );
    }
}
