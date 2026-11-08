//! The Windows implementations of the platform traits: the hidden-suspended
//! spawn flags, the kill-on-close group, the Toolhelp descendant walk, the
//! per-pid working-set reader, the pid-identity token rules, and the named
//! mutex single-instance guard. This is the documented-unsafe cluster and it
//! moves as one unit so reviewers read one file for all Win32 invariants.
//!
//! This module holds the project's documented `unsafe` for Win32 interop: every
//! block states its invariant. The single-instance guard (A10, §0.2 step 3)
//! lives here too: a named mutex refuses a second harness.

use super::{ProcessIdentity, ProcessTree, SingleInstance};
use crate::custody::{CustodyOs, TerminateOutcome};
use async_trait::async_trait;

/// `CREATE_NO_WINDOW`: hidden children (Step 8).
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Hides a short-lived control child: the same flag the engine spawn uses,
/// without the suspended/resume discipline (control commands are not sealed
/// into a job).
pub fn hide_window(command: &mut tokio::process::Command) {
    // tokio's Command exposes creation_flags inherently on Windows.
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Owned Job Object handle. `Send` because the runtime owns it exclusively: it is
/// created on one task, stored once, and closed exactly once — by [`Drop`].
struct JobHandle(windows::Win32::Foundation::HANDLE);

// Safety: exclusive ownership as documented on the struct; Win32 job handles are
// usable from any thread and we never duplicate or share this one.
unsafe impl Send for JobHandle {}

impl Drop for JobHandle {
    fn drop(&mut self) {
        // Safety: sole owner per the struct invariant. Closing an un-terminated
        // job also kills its tree (KILL_ON_JOB_CLOSE) — the same kernel mechanism
        // `kill` relies on, so dropped entries can never outlive their children.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// The Win32 kill-on-close group: one Job Object per owned engine tree, closed
/// exactly once by the owner's `Drop` — which also reaps the tree.
pub struct KillGroup(JobHandle);

impl KillGroup {
    /// Terminates every process in the group.
    fn terminate(&self) {
        // Safety: the handle is valid (created in `create`) and closing or
        // terminating a job is safe from any thread; the group owns it.
        unsafe { terminate_job(self.0 .0) };
    }
}

/// ALL descendants of `root`, at every depth, from ONE Toolhelp32 process
/// snapshot:
/// both engines sit TWO levels below the harness child — python launcher ->
/// python server -> engine, plus the engine's own vision child — so a
/// one-generation sum misses the very process that holds the card. The
/// snapshot becomes a parent->children table and a breadth-first walk leaves
/// from the root with a visited set, so a parent cycle in the table cannot
/// spin the walk and the root itself never enters the answer. The walk reads
/// the snapshot only and touches no process. Every failure path — snapshot
/// refused, walk broken at any point — answers an empty Vec: absence is
/// reported, never a guess (doctrine 4).
///
/// FR-04 truncation contract: the answer is bounded at `DESCENDANT_CAP`, and
/// an answer whose length reaches that bound is the walk saying *I stopped* —
/// it is a partial list. A caller that must not treat a partial tree as
/// complete (working-set sums, process-tree measurements) checks
/// `pids.len() >= DESCENDANT_CAP` and refuses or logs; the bound is
/// conservative in the safe direction (a complete tree of exactly cap size is
/// declined too — pathological, and a refusal costs one sample, never a
/// mis-attribution).
#[must_use]
pub fn descendant_pids_of(root: u32) -> Vec<u32> {
    use crate::process::DESCENDANT_CAP;
    use std::collections::{HashMap, HashSet, VecDeque};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // Safety: a fresh process-list snapshot; `CreateToolhelp32Snapshot`
    // answers Err for the invalid handle, and every entry buffer passed to
    // the walk is owned here, zeroed, and carries its exact size in `dwSize`.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // Safety: `snapshot` is the live handle from above; `entry` is a writable,
    // correctly sized PROCESSENTRY32W. A failed `First` is an empty or broken
    // snapshot — the walk simply stops.
    let mut walking = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while walking {
        children_of
            .entry(entry.th32ParentProcessID)
            .or_default()
            .push(entry.th32ProcessID);
        // Safety: the same live handle and entry as the `First` call;
        // `Next` reports the end of the list (and any error) as a failed walk.
        walking = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    // Safety: sole owner of the snapshot handle from above; this single close
    // is the release on every path, mirroring the [`JobHandle`] discipline.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(snapshot) };
    let mut seen: HashSet<u32> = HashSet::new();
    seen.insert(root); // the root is asked-about, never an answer of its own
    let mut queue: VecDeque<u32> = VecDeque::new();
    queue.push_back(root);
    let mut descendants: Vec<u32> = Vec::new();
    while let Some(parent) = queue.pop_front() {
        let Some(children) = children_of.get(&parent) else {
            continue;
        };
        for &child in children {
            if descendants.len() >= DESCENDANT_CAP {
                return descendants; // pathological tree: bounded, never unbounded work
            }
            if seen.insert(child) {
                descendants.push(child);
                queue.push_back(child);
            }
        }
    }
    descendants
}

/// Working-set bytes of `pid`, read with `GetProcessMemoryInfo`
/// (`K32GetProcessMemoryInfo`): the RAM
/// twin of the Toolhelp walk above — unlike WDDM per-PID VRAM (box-wide
/// `[N/A]`), this query always answers for a live pid, so `None` means the
/// process is gone or unreadable and nothing else. Every failure path — open
/// refused, query refused — answers `None`: absence is reported, never a
/// guess (doctrine 4). The caller owns the bytes→GiB conversion, exactly once
/// at its own witness boundary.
#[must_use]
pub fn working_set_bytes_of(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // Safety: `OpenProcess` requests read-only query rights for the given pid;
    // every handle it returns is closed on every path below, and it is not
    // used after the close. `PROCESS_MEMORY_COUNTERS` is passed with its exact
    // size in `cb`, as the API contract requires, and its fields are read only
    // after the call reported success.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        let read = GetProcessMemoryInfo(handle, &mut counters, counters.cb);
        let _ = CloseHandle(handle);
        read.ok()?;
        Some(counters.WorkingSetSize as u64)
    }
}

/// Holds the named mutex for the process lifetime (A10 single-instance guard).
pub struct SingleInstanceGuard {
    handle: windows::Win32::Foundation::HANDLE,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        // Safety: sole owner per the struct invariant; the guard is process-lifetime.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

impl SingleInstance for SingleInstanceGuard {
    fn acquire() -> Result<Self, String> {
        // Safety: CreateMutexW with no security attributes and automatic ownership
        // reclaim; we only inspect the last error, never wait on the handle.
        unsafe {
            // The mutex name carries the product name: the pre-rename compatibility shim
            // is retired, exactly as the rename decision is complete — a
            // pre-rename binary no longer shares this lock, and none is
            // expected to run beside the current harness.
            let name = windows::core::w!("SimpleHarn-SingleInstance");
            let handle = windows::Win32::System::Threading::CreateMutexW(None, false, name)
                .map_err(|e| format!("cannot create mutex: {e}"))?;
            if windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
            {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
                return Err("another SimpleHarn instance is already running".to_string());
            }
            Ok(SingleInstanceGuard { handle })
        }
    }
}

/// Creates a Job Object whose tree dies when the handle closes (or is terminated).
///
/// # Safety
///
/// The caller must close the returned handle exactly once (`CloseHandle`) and must
/// not use it after. Valid as long as the kernel object exists.
unsafe fn create_kill_on_close_job() -> Result<windows::Win32::Foundation::HANDLE, String> {
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    let job = CreateJobObjectW(None, None).map_err(|e| format!("CreateJobObjectW: {e}"))?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let set = SetInformationJobObject(
        job,
        JobObjectExtendedLimitInformation,
        &limits as *const _ as *const core::ffi::c_void,
        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
    );
    if set.is_err() {
        let _ = windows::Win32::Foundation::CloseHandle(job);
        return Err("SetInformationJobObject failed".to_string());
    }
    Ok(job)
}

/// Assigns a child process to a job.
///
/// # Safety
///
/// `child` must be a live process handle owned by this harness; the job must not
/// already contain processes with conflicting limits.
unsafe fn assign_to_job(
    job: windows::Win32::Foundation::HANDLE,
    child: std::os::windows::io::RawHandle,
) -> Result<(), String> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;

    AssignProcessToJobObject(job, HANDLE(child as _))
        .map_err(|e| format!("AssignProcessToJobObject: {e}"))
}

/// Resumes the primary thread of a process created with `CREATE_SUSPENDED`:
/// the engine spawn seals the child into its kill job while no
/// instruction has run, then resumes it exactly once. A suspended process has
/// precisely one thread — its primary — so the Toolhelp entry whose owner pid
/// matches is that thread, and it cannot exit while suspended. This helper
/// needs the id because `std`'s `Command` closes the `CreateProcessW` primary
/// thread handle it is given and does not expose it.
///
/// # Errors
///
/// The Win32 reason the primary thread could not be found, opened, or resumed.
fn resume_primary_thread(pid: u32) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // Safety: a fresh thread-list snapshot; `CreateToolhelp32Snapshot` answers
    // Err for the invalid handle, and every entry buffer passed to the walk is
    // owned here, zeroed, and carries its exact size in `dwSize`. The thread
    // handle is opened by that snapshot's own id and closed on every path
    // below; resuming the one suspended thread of a child this harness just
    // created suspended affects nothing else.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
            .map_err(|e| format!("CreateToolhelp32Snapshot: {e}"))?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut primary: Option<u32> = None;
        let mut walking = Thread32First(snapshot, &mut entry).is_ok();
        while walking {
            if entry.th32OwnerProcessID == pid {
                primary = Some(entry.th32ThreadID);
                break;
            }
            walking = Thread32Next(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        let tid = primary.ok_or_else(|| format!("pid {pid} has no thread to resume"))?;
        let thread = OpenThread(THREAD_SUSPEND_RESUME, false, tid)
            .map_err(|e| format!("OpenThread({tid}): {e}"))?;
        let previous = ResumeThread(thread);
        let _ = CloseHandle(thread);
        if previous == u32::MAX {
            return Err(format!("ResumeThread({tid}) failed"));
        }
        if previous > 1 {
            // `CREATE_SUSPENDED` guarantees one suspend; more would mean the
            // child is still frozen after this resume. Fail closed: the seal
            // failure arm kills the child and mints no ownership evidence.
            return Err(format!(
                "ResumeThread({tid}) left the primary thread suspended (count {previous})"
            ));
        }
        Ok(())
    }
}

/// Terminates every process in the job.
///
/// # Safety
///
/// The handle must be valid and not yet closed.
unsafe fn terminate_job(job: windows::Win32::Foundation::HANDLE) {
    use windows::Win32::System::JobObjects::TerminateJobObject;

    let _ = TerminateJobObject(job, 1);
}

/// The live [`ProcessTree`]: the hidden-suspended spawn, the Job Object group,
/// the Toolhelp walk, and the working-set reader.
pub struct WinProcessTree;

impl ProcessTree for WinProcessTree {
    fn prepare_engine_command(&self, command: &mut tokio::process::Command) {
        // Hidden AND suspended. The primary thread runs no
        // instruction until the seal below has put this child in its kill
        // job and resumed it, so no engine thread can ever run — and no
        // grandchild ever be born — outside the job.
        command.creation_flags(
            CREATE_NO_WINDOW | windows::Win32::System::Threading::CREATE_SUSPENDED.0,
        );
    }

    fn create_kill_group(&self) -> Result<KillGroup, String> {
        // Safety: the handle is created here and handed to exactly one
        // `KillGroup`, whose owner's `Drop` closes it; job handles are usable
        // from any thread.
        unsafe { create_kill_on_close_job() }.map(|job| KillGroup(JobHandle(job)))
    }

    fn seal(&self, group: &KillGroup, child: &tokio::process::Child) -> Result<(), String> {
        let pid = child.id().unwrap_or(0);
        match (child.raw_handle(), pid) {
            // Safety: the job handle is valid (created for this launch, closed
            // by its owner's `Drop`), `raw_handle` is valid while the child is
            // alive, and both run here while the child is still suspended and
            // before it is stored.
            (Some(handle), pid) if pid != 0 => unsafe { assign_to_job(group.0 .0, handle) }
                .and_then(|()| resume_primary_thread(pid)),
            _ => Err("the spawned client reported no usable handle or pid".to_string()),
        }
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

/// `STILL_ACTIVE`: what `GetExitCodeProcess` reports for a running process.
const STILL_ACTIVE: u32 = 259;

/// The live [`ProcessIdentity`]: Win32 pid identity through `OpenProcess` +
/// `GetProcessTimes`, terminate through a re-verified handle. The start-time
/// token is the process creation FILETIME as `u64` — the same "same for one
/// process's lifetime, different for a reused pid" property the engine-side
/// probe keys on.
pub struct WinProcessIdentity;

/// FILETIME pair → the 64-bit 100 ns tick count since 1601.
#[must_use]
fn file_time_ticks(low: u32, high: u32) -> u64 {
    ((u64::from(high)) << 32) | u64::from(low)
}

/// The creation-time token of the process behind an open handle: the same
/// value [`ProcessIdentity::start_token`] answers for a live pid, read straight off
/// the launch handle (slice 10b: the spawn path records what the sweep will
/// later verify — one token rule, one spelling).
///
/// The token is a process creation FILETIME: same for one process's whole
/// lifetime, different for a later process that reuses the pid.
fn start_token_of(handle: windows::Win32::Foundation::HANDLE) -> Option<u64> {
    // Safety: `handle` is a valid process handle from `OpenProcess`; the four
    // out-params are plain 64-bit slots `GetProcessTimes` always writes for a
    // process handle.
    unsafe {
        let mut creation = windows::Win32::Foundation::FILETIME::default();
        let mut exit = windows::Win32::Foundation::FILETIME::default();
        let mut kernel = windows::Win32::Foundation::FILETIME::default();
        let mut user = windows::Win32::Foundation::FILETIME::default();
        windows::Win32::System::Threading::GetProcessTimes(
            handle,
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()
        .map(|()| file_time_ticks(creation.dwLowDateTime, creation.dwHighDateTime))
    }
}

/// The start token of a live pid; `None` for pid 0, a dead process, one that
/// cannot be opened, or whose times cannot be read — never a guess.
fn live_start_token(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return None;
    }
    // Safety: query-limited access only; the handle is closed on every path
    // below, and every API result is checked.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let outcome = (|| {
            let mut code = 0u32;
            GetExitCodeProcess(handle, &mut code).ok()?;
            if code != STILL_ACTIVE {
                return None;
            }
            start_token_of(handle)
        })();
        let _ = CloseHandle(handle);
        outcome
    }
}

impl ProcessIdentity for WinProcessIdentity {
    fn start_token(&self, pid: u32) -> Option<u64> {
        live_start_token(pid)
    }

    fn start_token_of_spawned(&self, child: &tokio::process::Child) -> Option<u64> {
        child
            .raw_handle()
            .and_then(|raw| start_token_of(windows::Win32::Foundation::HANDLE(raw)))
    }

    fn terminate_verified(&self, pid: u32, recorded_token: u64) -> TerminateOutcome {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_TERMINATE,
        };
        if pid == 0 {
            return TerminateOutcome::IdentityGone;
        }
        // Re-verify at the instant of the signal ON THE SAME HANDLE the signal
        // uses (review 10b): Windows cannot recycle a pid while any handle to
        // its process object is open, so checking the token and reading the
        // exit code through this one handle — then terminating through that
        // same handle — leaves no window in which the verified process dies,
        // the pid is handed out again, and a stranger receives the terminate.
        unsafe {
            let handle = match OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                false,
                pid,
            ) {
                Ok(handle) => handle,
                Err(_) => {
                    // The combined open failed: either the process is gone or
                    // a live process refuses terminate access. The query-only
                    // probe tells those apart — a live matching identity we
                    // may not signal keeps the claim for the next boot,
                    // anything else is nothing to signal.
                    return match live_start_token(pid) {
                        Some(token) if token == recorded_token => TerminateOutcome::Failed {
                            reason: "cannot open the process for termination".to_string(),
                        },
                        _ => TerminateOutcome::IdentityGone,
                    };
                }
            };
            let outcome = if start_token_of(handle) != Some(recorded_token) {
                // A different process owns this pid now (or the token is
                // unreadable): never signal it.
                TerminateOutcome::IdentityGone
            } else {
                let mut code = 0u32;
                // Identity proven; liveness unreadable or provably exited
                // leaves nothing to signal (same no-guess rule as
                // `live_start_token`).
                if GetExitCodeProcess(handle, &mut code).is_err() || code != STILL_ACTIVE {
                    TerminateOutcome::IdentityGone
                } else {
                    match TerminateProcess(handle, 1) {
                        Ok(()) => TerminateOutcome::Done,
                        Err(error) => TerminateOutcome::Failed {
                            reason: error.message().to_string(),
                        },
                    }
                }
            };
            let _ = CloseHandle(handle);
            outcome
        }
    }
}

/// The custody sweep drives its seam through [`CustodyOs`]; the identity legs
/// delegate to [`ProcessIdentity`] and the port leg rides the trait's default:
/// a loopback connect is platform-neutral.
#[async_trait]
impl CustodyOs for WinProcessIdentity {
    fn start_token(&self, pid: u32) -> Option<u64> {
        ProcessIdentity::start_token(self, pid)
    }

    fn terminate(&self, pid: u32, recorded_token: u64) -> TerminateOutcome {
        ProcessIdentity::terminate_verified(self, pid, recorded_token)
    }
}

#[cfg(test)]
mod tests {
    /// Pin: the single-instance mutex name
    /// carries the current product spelling. The pre-rename compatibility shim
    /// is retired; a drift back to the
    /// pre-rename name would silently split the lock into two namespaces and
    /// break the single-instance rule for one of them. Falsifier: restore the
    /// pre-rename shim name — the first assert drops the new literal, the
    /// second catches the pre-rename spelling.
    #[test]
    fn the_single_instance_mutex_carries_the_current_product_name() {
        let source = include_str!("windows.rs");
        let body = source
            .split("fn acquire(")
            .nth(1)
            .expect("acquire must stay")
            .split("\n    }")
            .next()
            .expect("acquire must close");
        assert!(
            body.contains(r#"w!("SimpleHarn-SingleInstance")"#),
            "the mutex must carry the current product name: {body}"
        );
        let stripped = body.replace("SimpleHarn", "");
        let pre_rename = ["Simple", "Har"].concat();
        assert!(
            !stripped.contains(&pre_rename),
            "the pre-rename mutex spelling must not return: {stripped}"
        );
    }
}
