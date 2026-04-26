//! Audio output subsystem (FR-6.1).
//!
//! Plays alarm sounds via `cpal` with a configurable volume ramp. The public
//! entry point is the [`AlarmAudio`] trait (NFR-6.2); production uses
//! [`CpalAudio`], and [`FakeAudio`] is an in-memory replacement for tests.
//!
//! ## In scope here
//!
//! - WAV/FLAC decode via `symphonia` ([`DecodedSound`])
//! - cpal stream lifecycle on a dedicated OS thread (`cpal::Stream` is `!Send`
//!   on ALSA)
//! - Linear 2–10 s volume ramp, plus `Immediate` for tier `emergency`
//! - Best-effort hot-unplug recovery: log on stream error, retry stream build
//!   every 5 s until the device reappears
//! - Mono → N-channel upmix, nearest-neighbor resampling between file and
//!   device sample rates
//!
//! ## Deferred
//!
//! - Wiring into the daemon's firing/escalation path (waiting on tier model
//!   #15 and alarm records #6–#7)
//! - `OutputDegraded` D-Bus signal (#19) — today the subsystem logs

use std::sync::Arc;

mod cpal_audio;
mod decoded;
mod error;
#[cfg(test)]
mod fake;
mod profile;
mod sounds;

pub use cpal_audio::{CpalAudio, CpalConfig};
pub use decoded::DecodedSound;
pub use error::AudioError;
pub use profile::{gain_at, Ramp, VolumeProfile};
pub use sounds::SoundLibrary;

/// Alarm audio output (NFR-6.2).
///
/// Implementations own any background threads / streams. Methods are
/// synchronous and short; call from a `spawn_blocking` context if the caller
/// is on a tokio worker and cannot afford the ~1 s worst-case ack timeout.
pub trait AlarmAudio: Send + Sync + std::fmt::Debug {
    /// Start playing `sound` with `profile`. If another playback is already
    /// in flight it is replaced — only one alarm sound plays at a time per
    /// player.
    fn play(&self, sound: Arc<DecodedSound>, profile: VolumeProfile) -> Result<(), AudioError>;

    /// Stop any in-flight playback. No-op if nothing is playing.
    fn stop(&self) -> Result<(), AudioError>;
}
