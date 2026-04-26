#![forbid(unsafe_code)]

//! Typed async client for `org.helm.AlarmDaemon` over D-Bus.
//!
//! # Example
//!
//! ```no_run
//! use alarm_daemon_client::AlarmDaemonClient;
//!
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let client = AlarmDaemonClient::connect_session().await?;
//! let sounds = client.list_sounds().await?;
//! if let Some(sound_id) = sounds.first() {
//!     client.arm(sound_id).await?;
//! }
//! # Ok(())
//! # }
//! ```

mod client;
mod constants;
mod error;

pub use client::AlarmDaemonClient;
pub use error::ClientError;
