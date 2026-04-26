//! Shared helpers for the integration test suite.
//!
//! The integration tests touch real user-scope state (`~/.local/bin`,
//! `~/.local/share/helm/sounds`, the user systemd unit) and a shared session
//! bus, so they must run one at a time. `cargo test` parallelises across both
//! tests *within* a binary and across test *binaries* — to cover both we use:
//!
//! - a per-process [`Mutex`] to serialise tests inside a single binary, and
//! - a `flock`-style file lock at a fixed temp path to serialise tests across
//!   different binaries / `cargo test` invocations.
//!
//! Cleanup is RAII: dropping [`TestEnv`] runs `force_clean` even if the test
//! body panicked, so a failing test never leaves the host in a state that
//! poisons subsequent runs.

#![allow(dead_code, clippy::panic)]

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use fs2::FileExt;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn run_bash(script: &str) -> bool {
    Command::new("bash")
        .current_dir(repo_root())
        .arg("-lc")
        .arg(script)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn require_tools() {
    assert!(have_cmd("bash"), "missing required command: bash");
    assert!(have_cmd("busctl"), "missing required command: busctl");
    assert!(
        have_cmd("dbus-run-session"),
        "missing required command: dbus-run-session"
    );
}

/// RAII guard that serialises an integration test against every other test
/// in the workspace and runs `force_clean` on entry/exit.
///
/// Acquire it as the very first line of each `#[test]`:
///
/// ```ignore
/// let _env = common::TestEnv::acquire();
/// ```
pub struct TestEnv {
    _file_lock: FileLock,
    _process_lock: MutexGuard<'static, ()>,
}

impl TestEnv {
    pub fn acquire() -> Self {
        let process_lock = lock_in_process();
        let file_lock = FileLock::acquire();
        force_clean();
        Self {
            _file_lock: file_lock,
            _process_lock: process_lock,
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        force_clean();
    }
}

fn force_clean() {
    let _ = run_bash(
        r#"
set +e
./setup.sh uninstall >/dev/null 2>&1
"#,
    );
}

/// File-backed exclusive lock spanning every `cargo test` process.
struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire() -> Self {
        let path = std::env::temp_dir().join("alarm-daemon-tests.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("failed to open test lock {}: {e}", path.display()));
        file.lock_exclusive()
            .unwrap_or_else(|e| panic!("failed to acquire test lock {}: {e}", path.display()));
        Self { file }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Best-effort: closing the file releases the flock automatically.
        let _ = FileExt::unlock(&self.file);
    }
}

fn lock_in_process() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Runs `body` inside an isolated `dbus-run-session`, with the daemon's
/// `start`/`stop` shell helpers pre-defined.
pub fn run_dbus_daemon_session(body: &str) -> bool {
    let script = format!(
        r#"
set -euo pipefail
dbus-run-session -- bash -lc '
  set -euo pipefail
  export HOME="{home}"
  DB_PATH="$(mktemp -u /tmp/alarm-daemon-test-XXXXXX.redb)"
  trap "rm -f \"${{DB_PATH}}\"" EXIT
  start() {{
    ALARM_DAEMON_BUS=session ALARM_DAEMON_DB_PATH="${{DB_PATH}}" \
    ALARM_DAEMON_SOUND_DIR="$HOME/.local/share/helm/sounds" \
    ALARM_DAEMON_CUSTOM_SOUND_DIR="$HOME/.local/share/helm/custom-sounds" \
    "$HOME/.local/bin/alarm-daemon" >/tmp/alarm-daemon-e2e.log 2>&1 &
    DAEMON_PID=$!
    for _ in $(seq 1 50); do
      busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control Status >/dev/null 2>&1 && return 0
      sleep 0.1
    done
    return 1
  }}
  stop() {{ kill "${{DAEMON_PID}}"; wait "${{DAEMON_PID}}" || true; }}

  {body}
'
"#,
        home = std::env::var("HOME").unwrap_or_else(|_| String::from(".")),
        body = body
    );
    run_bash(&script)
}

fn have_cmd(name: &str) -> bool {
    Command::new("bash")
        .arg("-lc")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
