//! The launcher owns the pocket-ic server's lifecycle: pocket-ic is spawned into
//! its own process group and given a 30-day `--ttl`, so if the launcher goes away
//! without signalling it, nothing else will ever clean it up.
//!
//! These tests drive the real launcher binary against a fake pocket-ic server, so
//! they need no pocket-ic download and finish in seconds.
#![cfg(unix)]

use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, ExitStatus},
    thread::sleep,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tempfile::TempDir;

/// Stand-in for the pocket-ic server. It records the pids the launcher is
/// responsible for cleaning up, then writes a *malformed* port file, which fails
/// the launcher's startup sequence at a point where pocket-ic is already running.
///
/// The malformed port is just a cheap way to reach that error path; in the wild it
/// was reached by the HTTP gateway failing to bind an already-used port.
const FAKE_POCKETIC_SERVER: &str = r#"#!/bin/sh
port_file=
while [ $# -gt 0 ]; do
    case "$1" in
        --port-file) port_file="$2"; shift 2 ;;
        *) shift ;;
    esac
done
# Stands in for the canister sandboxes pocket-ic forks: in the server's process
# group, but not a direct child of the launcher.
sleep 600 &
echo "$!" > "$LAUNCHER_TEST_PID_DIR/sandbox.pid"
echo "$$" > "$LAUNCHER_TEST_PID_DIR/server.pid"
# Writing the port file is what unblocks the launcher, so it goes last.
printf 'not-a-port\n' > "$port_file"
wait
"#;

/// Generous enough to absorb a loaded CI runner, while still failing rather than
/// hanging the suite if the launcher deadlocks.
const LAUNCHER_TIMEOUT: Duration = Duration::from_secs(60);

/// The launcher signals pocket-ic before it exits, so by the time it is gone the
/// processes only have to be reaped. This just covers that last-moment race.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

/// When the launcher fails *after* spawning pocket-ic, it must still take the
/// server — and everything pocket-ic forked — down with it.
#[test]
fn error_after_spawn_leaves_no_pocketic_behind() {
    let dir = TempDir::new().expect("failed to create temporary directory");
    let pid_dir = dir.path().join("pids");
    fs::create_dir(&pid_dir).expect("failed to create pid directory");
    let fake_server = dir.path().join("fake-pocket-ic");
    fs::write(&fake_server, FAKE_POCKETIC_SERVER).expect("failed to write fake pocket-ic server");
    fs::set_permissions(&fake_server, Permissions::from_mode(0o755))
        .expect("failed to make fake pocket-ic server executable");

    let mut launcher = Command::new(env!("CARGO_BIN_EXE_icp-cli-network-launcher"))
        .arg("--pocketic-server-path")
        .arg(&fake_server)
        .env("LAUNCHER_TEST_PID_DIR", &pid_dir)
        .spawn()
        .expect("failed to spawn the launcher");
    let status = wait_with_timeout(&mut launcher, LAUNCHER_TIMEOUT);
    assert!(
        !status.success(),
        "expected the launcher to fail on the malformed port file, but it exited with {status}"
    );

    let server = read_pid(&pid_dir.join("server.pid"));
    let sandbox = read_pid(&pid_dir.join("sandbox.pid"));
    let leaked: Vec<&str> = [("pocket-ic server", server), ("sandbox child", sandbox)]
        .into_iter()
        .filter(|&(_, pid)| !wait_for_exit(pid, CLEANUP_TIMEOUT))
        .map(|(name, _)| name)
        .collect();
    // Don't leave the leak running for the rest of the suite (or the CI runner).
    for pid in [server, sandbox] {
        let _ = signal::kill(pid, Signal::SIGKILL);
    }
    assert!(
        leaked.is_empty(),
        "the launcher exited but left {} running",
        leaked.join(" and ")
    );
}

/// Waits for `child` to exit, killing it and failing the test on timeout.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("failed to wait for the launcher") {
            Some(status) => return status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("the launcher did not exit within {timeout:?}");
            }
            None => sleep(Duration::from_millis(50)),
        }
    }
}

fn read_pid(path: &Path) -> Pid {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let pid = contents
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("failed to parse pid from {}: {e}", path.display()));
    Pid::from_raw(pid)
}

/// Whether `pid` still exists, zombies included.
fn is_alive(pid: Pid) -> bool {
    !matches!(signal::kill(pid, None), Err(Errno::ESRCH))
}

fn wait_for_exit(pid: Pid, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while is_alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(50));
    }
    true
}
