#[cfg(test)]
use std::time::Duration;

use thiserror::Error;

/// Errors raised by the audio output subsystem (FR-6.1).
#[derive(Debug, Error)]
pub enum AudioError {
    /// Ramp duration outside the FR-6.1 range of 2–10 s. Today only built by
    /// the test-only `Ramp::linear` constructor.
    #[cfg(test)]
    #[error("invalid ramp duration: must be 2..=10 seconds, got {0:?}")]
    InvalidRamp(Duration),

    /// Target volume outside `0.0..=1.0`, or non-finite.
    #[error("invalid volume target: must be in 0.0..=1.0, got {0}")]
    InvalidVolume(f32),

    /// No audio output device is available on the host.
    #[error("no audio output device available")]
    NoDevice,

    /// Failed to query or select an output device configuration.
    #[error("audio device configuration failed: {0}")]
    DeviceConfig(String),

    /// Failed to build the cpal output stream.
    #[error("failed to build audio output stream: {0}")]
    BuildStream(String),

    /// Failed to start an already-built stream.
    #[error("failed to start audio output stream: {0}")]
    PlayStream(String),

    /// Failed to decode the source WAV/FLAC file.
    #[error("failed to decode audio: {0}")]
    Decode(String),

    /// Unknown sound id or name.
    #[error("unknown sound id: {0}")]
    UnknownSound(String),

    /// Caller provided an invalid sound id.
    #[error("invalid sound id: {0}")]
    InvalidSoundId(String),

    /// Sound format is unsupported. Only constructed by the test-only sound
    /// registration path; promote to non-test once `RegisterSound` is exposed.
    #[cfg(test)]
    #[error("unsupported sound format: {0}")]
    UnsupportedFormat(String),

    /// Custom sound exceeds the configured max byte size. Test-only for the
    /// same reason as `UnsupportedFormat`.
    #[cfg(test)]
    #[error("sound is too large: {size} bytes (max {max} bytes)")]
    SoundTooLarge { size: usize, max: usize },

    /// Underlying I/O error (file open, thread spawn, etc.).
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// Worker thread has exited or its channel is closed.
    #[error("audio worker thread is unavailable")]
    WorkerUnavailable,

    /// Output device is currently lost; worker is attempting to rebuild the
    /// stream every 5 s (FR-6.1).
    #[error("audio device is currently unavailable; retry is in progress")]
    DeviceUnavailable,
}
