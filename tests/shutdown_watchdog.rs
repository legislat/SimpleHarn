//! Falsifier for the shutdown exit deadline: a parked child process that
//! armed the real watchdog with a short deadline must be ended by it with
//! exit code 0. Pre-fix there is no watchdog to arm (no such function
//! exists), so this target does not even compile against the pre-fix crate -
//! the red leg - and with the arming wired but the fire removed the child
//! parks past the bounded wait and the parent fails.

use std::io::Read;
use std::time::{Duration, Instant};

/// The marker the helper prints after arming and before parking; seeing it
/// proves the child reached the park (and that the exact-match filter hit),
/// so a later exit 0 can only be the watchdog.
const PARK_MARKER: &str = "SHUTDOWN-WATCHDOG-PARKED";

#[test]
fn a_parked_child_with_an_armed_deadline_is_exited_with_zero() {
    let exe = std::env::current_exe().expect("test binary path");
    let mut child = std::process::Command::new(exe)
        .args([
            "--exact",
            "watchdog_helper_arms_then_parks",
            "--ignored",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the parked helper child");
    let mut stdout = child.stdout.take().expect("piped stdout");
    // Drain the child's pipe on a separate thread: the read must not hold
    // the bounded wait hostage, and it must not let the child fill the pipe
    // and stall before arming.
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    // Generously above the armed 500 ms deadline: reaching it means the
    // deadline never fired and the child parked forever - the pre-fix shape.
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait().expect("try_wait the helper child") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the parked child outlived the bounded wait: an armed deadline must end it");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let log = reader
        .join()
        .expect("reader thread ends when the child dies");
    assert!(
        log.contains(PARK_MARKER),
        "the helper never reached its park (filter missed or the child died early): {log}"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "the watchdog exit must report shutdown, not a crash: {log}"
    );
}

/// Spawned by the test above (`--exact <this name> --ignored --nocapture`).
/// It arms the production mechanism with a short deadline, proves it parked,
/// and then blocks the way a `spawn_blocking` fs call parked in kernel I/O
/// blocks an exit. Reaching the panic line means the watchdog never fired.
/// `#[ignore]`d so the mechanism is never armed inside a normal suite run -
/// it hard-exits the whole process.
#[test]
#[ignore = "spawned by the parent test as a re-exec of this binary; arming the watchdog in-process would end the test binary"]
fn watchdog_helper_arms_then_parks() {
    simpleharn::shutdown::arm_exit_deadline(Duration::from_millis(500));
    println!("{PARK_MARKER}");
    std::io::Write::flush(&mut std::io::stdout()).expect("flush the park marker");
    std::thread::sleep(Duration::from_secs(3600));
    panic!("unreachable while the mechanism works: the armed deadline must end this process first");
}
