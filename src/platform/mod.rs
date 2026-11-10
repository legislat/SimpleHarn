//! The platform boundary for process supervision (Linux port design).
//!
//! Exactly three traits live here: [`ProcessIdentity`] (pid + start token),
//! [`ProcessTree`] (the kill-on-close group, killing the tree, and the
//! descendant walk), and [`SingleInstance`] (the one-instance lock). Host RAM
//! is deliberately NOT a trait of this module: the existing `RamWitness`
//! (`crate::witness`) stays where it is and gains platform implementations of
//! its own.
//!
//! Every `windows::` call of custody, supervision, and the single-instance
//! guard moves behind these traits, so a later platform only implements the
//! traits and touches no shared code. Phase P0 ships the Windows
//! implementations only; other targets arrive with their own phase.
//!
//! The three traits carry the OS seam exactly as the callers used it before the
//! move: same tokens, same error strings, same refusal rules. Nothing here
//! decides policy — the sweep rule, the seal-before-resume ordering, and the
//! ownership discipline stay in the modules that drive these seams.

use crate::custody::TerminateOutcome;
use std::sync::Arc;

/// The pid-identity seam: a start token is "same for one process's whole
/// lifetime, different for a later process that reuses the pid", and every
/// signal this harness sends re-verifies that identity first. `None` is
/// absence — a dead process, an unopenable one, an unreadable token — never a
/// guess (doctrine 4).
pub trait ProcessIdentity: Send + Sync {
    /// The live pid's current start token; `None` when no honest answer exists.
    fn start_token(&self, pid: u32) -> Option<u64>;

    /// The start token of a just-spawned child, read straight off its launch
    /// identity: the spawn path records what the boot sweep later verifies —
    /// one token rule, one spelling.
    fn start_token_of_spawned(&self, child: &tokio::process::Child) -> Option<u64>;

    /// Terminate `pid` only if it is still the recorded process at this
    /// instant (re-verifies the token inside); the caller never signals a
    /// process it has not identified.
    fn terminate_verified(&self, pid: u32, recorded_token: u64) -> TerminateOutcome;
}

/// The process-tree seam: one kill-on-close group per owned engine tree, the
/// tree kill, and the descendant walk that keeps a working-set or VRAM figure
/// honest about the whole family, not just the client.
pub trait ProcessTree: Send + Sync {
    /// Prepares an engine spawn command so the child is hidden and frozen at
    /// birth: no engine instruction runs before [`Self::seal`] has claimed the
    /// child into its kill group, so no descendant can be born outside it.
    fn prepare_engine_command(&self, command: &mut tokio::process::Command);

    /// Creates a group whose tree dies when the group handle closes or is
    /// terminated — the crash-proof orphan guarantee.
    ///
    /// # Errors
    ///
    /// The OS reason the group could not be created.
    fn create_kill_group(&self) -> Result<KillGroup, String>;

    /// Claims a still-frozen child into `group` and lets it run exactly once:
    /// the assignment happens before the first instruction, so the escape
    /// window between create and assign stays shut.
    ///
    /// # Errors
    ///
    /// The OS reason the child could not be claimed or resumed.
    fn seal(&self, group: &KillGroup, child: &tokio::process::Child) -> Result<(), String>;

    /// Terminates every process in the group.
    fn terminate_group(&self, group: &KillGroup);

    /// ALL descendants of `root`, at every depth, bounded at the walk cap:
    /// an answer at exactly the cap is the walk saying *I stopped* — a partial
    /// list the caller must never book as complete. Every failure answers an
    /// empty Vec: absence is reported, never a guess (doctrine 4).
    fn descendants(&self, root: u32) -> Vec<u32>;

    /// Working-set bytes of `pid`; `None` means the process is gone or
    /// unreadable and nothing else. The caller owns the unit conversion.
    fn working_set_bytes(&self, pid: u32) -> Option<u64>;
}

/// The one-instance lock: a second harness refuses to boot. The guard type is
/// the lock — dropping it releases, and every guard lives for the process
/// lifetime.
pub trait SingleInstance: Sized {
    /// Acquires the one-instance lock.
    ///
    /// # Errors
    ///
    /// [`String`] naming the error; "already running" means a second harness
    /// exists.
    fn acquire() -> Result<Self, String>;
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{
    console_window_visible, descendant_pids_of, host_ram_bytes, working_set_bytes_of, KillGroup,
    SingleInstanceGuard, UnixProcessIdentity, UnixProcessTree,
};
#[cfg(windows)]
pub use windows::{
    console_window_visible, descendant_pids_of, working_set_bytes_of, KillGroup,
    SingleInstanceGuard, WinProcessIdentity, WinProcessTree, CREATE_NO_WINDOW,
};

/// Hides a short-lived control child: no window on this target.
pub fn hide_window(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    windows::hide_window(command);
    #[cfg(unix)]
    unix::hide_window(command);
    #[cfg(not(any(windows, unix)))]
    let _ = command;
}

/// The live process-tree implementation for this target.
#[cfg(windows)]
#[must_use]
pub fn process_tree() -> Arc<dyn ProcessTree> {
    Arc::new(WinProcessTree)
}

/// The live process-tree implementation for this target.
#[cfg(unix)]
#[must_use]
pub fn process_tree() -> Arc<dyn ProcessTree> {
    Arc::new(UnixProcessTree)
}

/// The live process-identity implementation for this target.
#[cfg(windows)]
#[must_use]
pub fn process_identity() -> Arc<dyn ProcessIdentity> {
    Arc::new(WinProcessIdentity)
}

/// The live process-identity implementation for this target.
#[cfg(unix)]
#[must_use]
pub fn process_identity() -> Arc<dyn ProcessIdentity> {
    Arc::new(UnixProcessIdentity)
}

/// The process-identity seam the custody boot sweep drives.
#[cfg(windows)]
#[must_use]
pub fn custody_os() -> Arc<dyn crate::custody::CustodyOs> {
    Arc::new(WinProcessIdentity)
}

/// The process-identity seam the custody boot sweep drives.
#[cfg(unix)]
#[must_use]
pub fn custody_os() -> Arc<dyn crate::custody::CustodyOs> {
    Arc::new(UnixProcessIdentity)
}

/// The host boot id recorded on every custody entry at launch and compared
/// by the boot sweep (see [`crate::custody::boot_id_matches`]). Windows has
/// no boot identity this harness reads (the FILETIME token already spans a
/// reboot because a process object cannot outlive one), so it answers `None`
/// and entries stay unstamped. Linux reads the kernel's per-boot uuid; an
/// unreadable `/proc` answers `None`, which the asymmetric gate treats as
/// "no permission to act on a stamped entry", never as a match.
#[cfg(windows)]
#[must_use]
pub fn host_boot_id() -> Option<String> {
    None
}

/// The host boot id (see the [`crate::custody::boot_id_matches`] rule).
#[cfg(unix)]
#[must_use]
pub fn host_boot_id() -> Option<String> {
    unix::host_boot_id()
}

/// The host boot id: no boot identity exists to read on this target.
#[cfg(not(any(windows, unix)))]
#[must_use]
pub fn host_boot_id() -> Option<String> {
    None
}
