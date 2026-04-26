use thiserror::Error;

/// Error returned by client operations.
#[derive(Debug, Error)]
pub enum ClientError {
    /// A D-Bus transport, serialization, or remote method error.
    #[error("d-bus call failed: {0}")]
    DBus(#[from] zbus::Error),
}
