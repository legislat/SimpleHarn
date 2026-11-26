//! The process-exit deadline that runs only after shutdown was requested.
//!
//! Once shutdown is requested, the honest exit is a cooperative unwind:
//! abort the listener supervisors, let the teardown settle, drop the runtime.
//! One part of that unwind cannot be cancelled: a `spawn_blocking` job parked
//! in kernel I/O (a dead NAS, a wedged driver) holds the blocking pool, and
//! dropping the runtime waits on it forever - the process never exits and the
//! children stay alive under the still-held job handle. The watchdog is the
//! honest bound for exactly that case: a detached thread sleeps past the
//! deadline and hard-exits the process, and the kill-on-close job object
//! ([`crate::platform::KillGroup`]) reaps the whole tree on process exit.

use std::time::Duration;

/// Arms the exit deadline for a shutdown that has already been requested.
///
/// The thread is detached on purpose. Nothing can cancel a job parked in
/// kernel I/O, so nothing can honestly disarm the watchdog either: a healthy
/// exit ends the process long before the deadline fires, and only a wedged
/// exit ever reaches it.
///
/// `deadline` is injectable so a test can arm-fire the whole mechanism in a
/// child process without waiting for the product deadline. Production arms
/// once - immediately after the shutdown select fires in `main.rs` - and never
/// during normal operation, when an armed deadline would end a serving
/// process.
pub fn arm_exit_deadline(deadline: Duration) {
    let armed = std::thread::Builder::new()
        // 11 bytes: inside Linux's 15-character pthread name limit, so the
        // watchdog keeps its name on a Linux thread dump.
        .name("sh-watchdog".to_string())
        .spawn(move || {
            std::thread::sleep(deadline);
            // stderr, not the tracing stack: the wedge this thread exists to
            // escape can BE the harness log volume (exe and `logs\` beside each
            // other on a dead network path), and the file sink writes
            // synchronously on the calling thread - a tracing line here could
            // park the watchdog on its own log write and lose the bound. The
            // arm-time line above already landed in the file during normal
            // operation; this line is best-effort console output only.
            eprintln!(
                "simpleharn::shutdown: shutdown deadline of {}ms passed with the process \
                 still alive; hard-exiting now (this is the deadline, not a crash: \
                 shutdown was already requested)",
                deadline.as_millis()
            );
            // Exit code 0: shutdown was requested and served, so the only
            // fault is that the cooperative exit never finished. Skipping
            // destructors is safe for the child tree because the kernel
            // closes the job-object handle as the process dies, and the
            // kill-on-close flag reaps every engine the harness owns.
            std::process::exit(0);
        });
    if let Err(error) = armed {
        // Without the watchdog a stuck exit is unbounded; the log line is the
        // only place that truth can still appear.
        tracing::error!(
            target: "simpleharn::shutdown",
            error = %error,
            "could not arm the shutdown exit deadline; a stuck exit will not be bounded"
        );
    }
}
