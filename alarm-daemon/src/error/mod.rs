//! Error types and common result aliases for the daemon.

mod alarm_error;
pub use alarm_error::AlarmError;

/// Crate-local result type used across daemon modules.
pub type Result<T> = std::result::Result<T, AlarmError>;
