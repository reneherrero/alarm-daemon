//! Tracing subscriber initialisation.
//!
//! Picks one of two backends based on whether the daemon is running under a
//! systemd unit:
//!
//! - **Under systemd** (any unit type — detected via `$INVOCATION_ID`): emit
//!   events directly to journald via [`tracing_journald`]. Structured fields
//!   (`event.fields = …`) become first-class journal fields, so
//!   `journalctl -o json` and downstream shippers (Promtail, Vector,
//!   Datadog journald, …) get typed metadata instead of plain text.
//!
//! - **Outside systemd** (`cargo run`, integration tests, `dbus-run-session`):
//!   emit human-readable, ANSI-coloured records to stderr via the standard
//!   [`tracing_subscriber::fmt`] layer.
//!
//! Both layers honour `RUST_LOG` through [`EnvFilter`]; the default level is
//! `info` to match prior behaviour.

use tracing_subscriber::EnvFilter;

/// Identifies which backend was selected. Returned so the caller can mention
/// it in the first log line ("alarm-daemon ready bus=… logging=journald").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogBackend {
    /// Structured events sent over the journal socket.
    Journald,
    /// Human-readable text on stderr (default for dev runs).
    Stderr,
}

impl LogBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Journald => "journald",
            Self::Stderr => "stderr",
        }
    }
}

/// Initialises the global tracing subscriber for the lifetime of the process.
///
/// Falls back to stderr if journald was preferred but unreachable (the
/// failure is logged once via the stderr fallback so operators can spot it
/// in `journalctl -u alarm-daemon` boot logs even if the daemon is *not*
/// quite under systemd).
pub fn init() -> LogBackend {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_default();

    if running_under_systemd() {
        match tracing_journald::layer() {
            Ok(layer) => {
                use tracing_subscriber::layer::SubscriberExt as _;
                use tracing_subscriber::util::SubscriberInitExt as _;

                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer)
                    .init();
                return LogBackend::Journald;
            }
            Err(e) => {
                // Surface the journald failure on stderr so the fallback is
                // visible to operators inspecting `journalctl -u …` (systemd
                // captures stderr on its own when the journald socket is
                // genuinely broken — that path lands in the journal too).
                eprintln!(
                    "alarm-daemon: journald layer unavailable ({e}); \
                     falling back to stderr logging"
                );
            }
        }
    }

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_default();
    tracing_subscriber::fmt().with_env_filter(filter).init();
    LogBackend::Stderr
}

/// Returns `true` when the process appears to be running as a systemd unit
/// (any type — `notify`, `simple`, `dbus`, …).
///
/// `INVOCATION_ID` is set by systemd for every service invocation since v232
/// and is the canonical way for a child process to know it was spawned by
/// `systemd(1)`. We use it (rather than `NOTIFY_SOCKET`, which is only set
/// for `Type=notify`) so the dev-mode `Type=simple` user unit installed by
/// `setup.sh` also benefits from structured logging.
fn running_under_systemd() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_backend_as_str_is_stable() {
        assert_eq!(LogBackend::Journald.as_str(), "journald");
        assert_eq!(LogBackend::Stderr.as_str(), "stderr");
    }

    // We deliberately don't unit-test `running_under_systemd()` directly:
    // `std::env::set_var` is `unsafe` (#[forbid(unsafe_code)] applies) and
    // mutating shared process env from tests is racy under cargo's parallel
    // test runner. The function is one line and is exercised by every
    // integration test indirectly (the test harness runs without
    // INVOCATION_ID, so they all hit the stderr branch).
}
