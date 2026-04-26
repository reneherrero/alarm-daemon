//! Daemon-wide runtime configuration parsed from the environment.
//!
//! Today this is just the bus selection (session vs system) and the FHS-aware
//! defaults that follow from it. The struct lives here rather than in
//! `main.rs` so other modules (and unit tests) can reason about the defaults
//! without dragging in the binary entry point.
//!
//! Public APIs read process env at the top of the call; the underlying
//! parsers ([`BusKind::parse`], [`BusKind::default_db_path_with_home`]) are
//! pure functions over their inputs so unit tests don't need to mutate the
//! environment (which is now `unsafe` in edition 2024).

use std::path::PathBuf;

use crate::error::AlarmError;

/// Which D-Bus the daemon attaches to.
///
/// `Session` is the dev/CI default — it works without root, plays nicely with
/// `dbus-run-session`, and matches `systemd --user`. `System` is the
/// production posture used by Yocto images, and is what the bundled system
/// unit (`data/systemd/alarm-daemon.service`) sets via `Environment=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BusKind {
    /// `org.freedesktop.DBus` session bus (per-user).
    #[default]
    Session,
    /// `org.freedesktop.DBus` system bus (per-host).
    System,
}

impl BusKind {
    /// Reads `ALARM_DAEMON_BUS` and returns the matching variant. Defaults to
    /// [`BusKind::Session`] when unset so `cargo run` works out of the box.
    pub fn from_env() -> Result<Self, AlarmError> {
        Self::parse(std::env::var("ALARM_DAEMON_BUS").ok().as_deref())
    }

    /// Pure parser used by [`BusKind::from_env`] and by unit tests.
    pub fn parse(raw: Option<&str>) -> Result<Self, AlarmError> {
        match raw {
            None | Some("") | Some("session") => Ok(Self::Session),
            Some("system") => Ok(Self::System),
            Some(other) => Err(AlarmError::Config(format!(
                "ALARM_DAEMON_BUS must be 'session' or 'system' (got {other:?})"
            ))),
        }
    }

    /// Stable string used in logs and error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::System => "system",
        }
    }

    /// FHS-aligned default redb path per bus mode. Reads `$HOME` for the
    /// session case; overridden by `ALARM_DAEMON_DB_PATH` at the call site.
    #[must_use]
    pub fn default_db_path(self) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        self.default_db_path_with_home(&home)
    }

    /// Pure variant of [`BusKind::default_db_path`]; tests call this directly
    /// to avoid touching the process environment.
    #[must_use]
    pub fn default_db_path_with_home(self, home: &str) -> PathBuf {
        match self {
            Self::System => PathBuf::from("/var/lib/helm/alarm-daemon.redb"),
            Self::Session => {
                PathBuf::from(format!("{home}/.local/state/helm/alarm-daemon.redb"))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_session_when_unset() {
        assert_eq!(BusKind::parse(None).ok(), Some(BusKind::Session));
    }

    #[test]
    fn empty_value_is_treated_as_unset() {
        assert_eq!(BusKind::parse(Some("")).ok(), Some(BusKind::Session));
    }

    #[test]
    fn parses_session_value() {
        assert_eq!(
            BusKind::parse(Some("session")).ok(),
            Some(BusKind::Session)
        );
    }

    #[test]
    fn parses_system_value() {
        assert_eq!(BusKind::parse(Some("system")).ok(), Some(BusKind::System));
    }

    #[test]
    fn rejects_unknown_value() {
        let Err(err) = BusKind::parse(Some("tcp")) else {
            panic!("expected parse error for 'tcp'");
        };
        let msg = err.to_string();
        assert!(msg.contains("ALARM_DAEMON_BUS"), "{msg}");
        assert!(msg.contains("tcp"), "{msg}");
    }

    #[test]
    fn as_str_is_stable() {
        assert_eq!(BusKind::Session.as_str(), "session");
        assert_eq!(BusKind::System.as_str(), "system");
    }

    #[test]
    fn system_default_db_path_is_under_var_lib_helm() {
        assert_eq!(
            BusKind::System.default_db_path_with_home("/anything"),
            PathBuf::from("/var/lib/helm/alarm-daemon.redb")
        );
    }

    #[test]
    fn session_default_db_path_is_under_home() {
        assert_eq!(
            BusKind::Session.default_db_path_with_home("/tmp/some-fake-home"),
            PathBuf::from("/tmp/some-fake-home/.local/state/helm/alarm-daemon.redb")
        );
    }
}
