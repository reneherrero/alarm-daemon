use thiserror::Error;

/// Top-level error type for daemon startup and IPC wiring.
#[derive(Debug, Error)]
pub enum AlarmError {
    /// Error returned by zbus transport or protocol operations.
    #[error("D-Bus error: {0}")]
    DBus(#[from] zbus::Error),

    /// Error constructing or validating D-Bus names.
    #[error("D-Bus name error: {0}")]
    DBusName(#[from] zbus::names::Error),

    /// Underlying OS I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid runtime configuration provided by environment or flags.
    #[error("invalid configuration: {0}")]
    Config(String),
}
