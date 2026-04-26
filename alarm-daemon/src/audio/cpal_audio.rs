use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, SizedSample, StreamConfig};
use tracing::{debug, error, info, warn};

use super::{gain_at, AlarmAudio, AudioError, DecodedSound, VolumeProfile};

/// Interval between output-stream rebuild attempts after a device loss
/// (FR-6.1: "attempting recovery every 5 s").
const DEVICE_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// How long [`CpalAudio`] will wait for the worker thread to ack a command
/// before reporting the worker as unavailable.
const COMMAND_ACK_TIMEOUT: Duration = Duration::from_secs(1);

/// Configuration for [`CpalAudio`]. Device selection will move into the
/// §6 config loader later; today [`CpalConfig::default`] picks the host
/// default output device.
#[derive(Debug, Clone, Default)]
pub struct CpalConfig {
    /// Preferred output device name. `None` uses the host default.
    pub device_name: Option<String>,
}

/// Production [`AlarmAudio`] backed by cpal + ALSA (FR-6.1).
///
/// `cpal::Stream` is `!Send` on ALSA, so all stream work runs on a dedicated
/// OS thread owned by this type. The public methods send a command to that
/// thread and block on a short ack.
#[derive(Debug, Clone)]
pub struct CpalAudio {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    tx: Sender<Command>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

enum Command {
    Play {
        sound: Arc<DecodedSound>,
        profile: VolumeProfile,
        ack: Sender<Result<(), AudioError>>,
    },
    Stop {
        ack: Sender<Result<(), AudioError>>,
    },
    Shutdown,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Play { profile, .. } => write!(f, "Play({profile:?})"),
            Self::Stop { .. } => f.write_str("Stop"),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

impl CpalAudio {
    /// Spawn the cpal worker thread.
    ///
    /// If the initial stream build fails (no device, permissions, etc.), the
    /// worker still starts and enters device-retry mode. Callers get a live
    /// handle in either case; `play()` returns [`AudioError::DeviceUnavailable`]
    /// while no stream is open.
    pub fn spawn(config: CpalConfig) -> Result<Self, AudioError> {
        let (tx, rx) = mpsc::channel::<Command>();
        let handle = thread::Builder::new()
            .name("alarm-audio".into())
            .spawn(move || worker_main(rx, config))
            .map_err(AudioError::Io)?;
        Ok(Self {
            inner: Arc::new(Inner {
                tx,
                handle: Mutex::new(Some(handle)),
            }),
        })
    }

    fn send_and_wait(
        &self,
        make_cmd: impl FnOnce(Sender<Result<(), AudioError>>) -> Command,
    ) -> Result<(), AudioError> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.inner
            .tx
            .send(make_cmd(ack_tx))
            .map_err(|_| AudioError::WorkerUnavailable)?;
        ack_rx
            .recv_timeout(COMMAND_ACK_TIMEOUT)
            .map_err(|_| AudioError::WorkerUnavailable)?
    }
}

impl AlarmAudio for CpalAudio {
    fn play(&self, sound: Arc<DecodedSound>, profile: VolumeProfile) -> Result<(), AudioError> {
        self.send_and_wait(|ack| Command::Play {
            sound,
            profile,
            ack,
        })
    }

    fn stop(&self) -> Result<(), AudioError> {
        self.send_and_wait(|ack| Command::Stop { ack })
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        let mut guard = match self.handle.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(h) = guard.take() {
            if h.join().is_err() {
                warn!("alarm-audio worker thread panicked during shutdown");
            }
        }
    }
}

struct Playback {
    sound: Arc<DecodedSound>,
    profile: VolumeProfile,
    frames_played: u64,
    cursor_frames: f64,
    finished: bool,
}

struct Shared {
    playback: Option<Playback>,
    device_lost: bool,
}

impl Shared {
    fn new() -> Self {
        Self {
            playback: None,
            device_lost: false,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn worker_main(rx: mpsc::Receiver<Command>, cfg: CpalConfig) {
    let shared = Arc::new(Mutex::new(Shared::new()));
    let host = cpal::default_host();
    let mut stream = match build_stream(&host, &cfg, Arc::clone(&shared)) {
        Ok(s) => Some(s),
        Err(e) => {
            error!(error = %e, "alarm-audio: initial stream build failed; entering retry mode");
            lock(&shared).device_lost = true;
            None
        }
    };
    let mut last_retry = Instant::now();

    loop {
        // If the error callback flagged the device as lost, drop the stream
        // so the next retry can rebuild it cleanly.
        if lock(&shared).device_lost && stream.is_some() {
            warn!("alarm-audio: dropping failed stream, will retry");
            stream = None;
        }

        match rx.recv_timeout(DEVICE_RETRY_INTERVAL) {
            Ok(Command::Play { sound, profile, ack }) => {
                let result = handle_play(&shared, sound, profile, stream.is_some());
                let _ = ack.send(result);
            }
            Ok(Command::Stop { ack }) => {
                let result = handle_stop(&shared);
                let _ = ack.send(result);
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        if stream.is_none() && last_retry.elapsed() >= DEVICE_RETRY_INTERVAL {
            match build_stream(&host, &cfg, Arc::clone(&shared)) {
                Ok(s) => {
                    info!("alarm-audio: output device recovered");
                    lock(&shared).device_lost = false;
                    stream = Some(s);
                }
                Err(e) => {
                    debug!(error = %e, "alarm-audio: device still unavailable");
                }
            }
            last_retry = Instant::now();
        }
    }

    drop(stream);
    debug!("alarm-audio worker exiting");
}

fn handle_play(
    shared: &Mutex<Shared>,
    sound: Arc<DecodedSound>,
    profile: VolumeProfile,
    stream_present: bool,
) -> Result<(), AudioError> {
    if !stream_present {
        return Err(AudioError::DeviceUnavailable);
    }
    let mut sh = lock(shared);
    sh.playback = Some(Playback {
        sound,
        profile,
        frames_played: 0,
        cursor_frames: 0.0,
        finished: false,
    });
    Ok(())
}

fn handle_stop(shared: &Mutex<Shared>) -> Result<(), AudioError> {
    lock(shared).playback = None;
    Ok(())
}

fn pick_device(host: &Host, requested: Option<&str>) -> Result<Device, AudioError> {
    if let Some(name) = requested {
        let devices = host
            .output_devices()
            .map_err(|e| AudioError::DeviceConfig(e.to_string()))?;
        for d in devices {
            if d.name().ok().as_deref() == Some(name) {
                return Ok(d);
            }
        }
        return Err(AudioError::DeviceConfig(format!(
            "output device not found: {name}"
        )));
    }
    host.default_output_device().ok_or(AudioError::NoDevice)
}

fn build_stream(
    host: &Host,
    cfg: &CpalConfig,
    shared: Arc<Mutex<Shared>>,
) -> Result<cpal::Stream, AudioError> {
    let device = pick_device(host, cfg.device_name.as_deref())?;
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::DeviceConfig(e.to_string()))?;
    let sample_format = supported.sample_format();
    let stream_config: StreamConfig = supported.into();

    let stream = match sample_format {
        SampleFormat::F32 => build_typed::<f32>(&device, &stream_config, shared)?,
        SampleFormat::I16 => build_typed::<i16>(&device, &stream_config, shared)?,
        SampleFormat::U16 => build_typed::<u16>(&device, &stream_config, shared)?,
        other => {
            return Err(AudioError::BuildStream(format!(
                "unsupported sample format: {other:?}"
            )));
        }
    };

    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))?;
    Ok(stream)
}

fn build_typed<T>(
    device: &Device,
    config: &StreamConfig,
    shared: Arc<Mutex<Shared>>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    let channels = config.channels;
    let sample_rate = config.sample_rate.0;
    let mut scratch: Vec<f32> = Vec::new();
    let err_shared = Arc::clone(&shared);
    let data_shared = Arc::clone(&shared);

    device
        .build_output_stream(
            config,
            move |data: &mut [T], _info| {
                if scratch.len() < data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                let buf = &mut scratch[..data.len()];
                fill_f32(&data_shared, buf, channels, sample_rate);
                for (out, src) in data.iter_mut().zip(buf.iter()) {
                    *out = T::from_sample(*src);
                }
            },
            move |err| {
                error!(error = %err, "alarm-audio: output stream error");
                lock(&err_shared).device_lost = true;
            },
            None,
        )
        .map_err(|e| AudioError::BuildStream(e.to_string()))
}

/// Fill `output` (interleaved f32 frames) with the current playback, applying
/// ramp gain, nearest-neighbor resampling, and mono-to-N-channel upmix.
///
/// Pure w.r.t. `Shared` mutation; exposed to the module for direct tests.
fn fill_f32(
    shared: &Mutex<Shared>,
    output: &mut [f32],
    device_channels: u16,
    device_rate: u32,
) {
    let mut sh = lock(shared);
    let device_channels = usize::from(device_channels);
    if device_channels == 0 {
        return;
    }
    let Some(pb) = sh.playback.as_mut() else {
        output.fill(0.0);
        return;
    };

    let src_channels = usize::from(pb.sound.channels());
    let src_rate = f64::from(pb.sound.sample_rate());
    let dst_rate = f64::from(device_rate);
    let step = if dst_rate > 0.0 { src_rate / dst_rate } else { 1.0 };
    let src_samples = pb.sound.samples();
    let total_frames = pb.sound.frames();

    for frame in output.chunks_mut(device_channels) {
        if pb.finished || total_frames == 0 || src_channels == 0 {
            frame.fill(0.0);
            continue;
        }

        let mut idx = pb.cursor_frames as usize;
        if idx >= total_frames {
            if pb.profile.repeat() {
                pb.cursor_frames %= total_frames as f64;
                idx = pb.cursor_frames as usize;
            } else {
                pb.finished = true;
                frame.fill(0.0);
                continue;
            }
        }

        let elapsed = Duration::from_secs_f64(pb.frames_played as f64 / dst_rate.max(1.0));
        let gain = gain_at(pb.profile.ramp(), pb.profile.target(), elapsed);

        let src_base = idx.saturating_mul(src_channels);
        for (ch_out, out_sample) in frame.iter_mut().enumerate() {
            let src_ch = ch_out % src_channels;
            let s = src_samples.get(src_base + src_ch).copied().unwrap_or(0.0);
            *out_sample = s * gain;
        }

        pb.cursor_frames += step;
        pb.frames_played = pb.frames_played.saturating_add(1);
    }

    if pb.finished && !pb.profile.repeat() {
        sh.playback = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::{Ramp, VolumeProfile};
    use super::*;

    fn silent_sound(channels: u16, sample_rate: u32, frames: usize) -> Arc<DecodedSound> {
        // Build a DecodedSound through the public WAV path so we don't need
        // to expose a test constructor.
        let mut buf = Vec::new();
        let data_size =
            u32::try_from(frames * usize::from(channels) * 2).unwrap_or(u32::MAX);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36u32 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * u32::from(channels) * 2).to_le_bytes());
        buf.extend_from_slice(&(channels * 2).to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for _ in 0..(frames * usize::from(channels)) {
            buf.extend_from_slice(&(i16::MAX / 2).to_le_bytes());
        }
        Arc::new(DecodedSound::from_bytes(buf, Some("wav")).unwrap())
    }

    fn shared_with_playback(pb: Playback) -> Arc<Mutex<Shared>> {
        Arc::new(Mutex::new(Shared {
            playback: Some(pb),
            device_lost: false,
        }))
    }

    #[test]
    fn silence_when_idle() {
        let shared = Arc::new(Mutex::new(Shared::new()));
        let mut out = vec![1.0f32; 8];
        fill_f32(&shared, &mut out, 2, 48_000);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn non_repeating_sound_clears_playback_when_done() {
        let sound = silent_sound(1, 48_000, 4);
        let profile = VolumeProfile::new(1.0, Ramp::immediate(), false).unwrap();
        let shared = shared_with_playback(Playback {
            sound,
            profile,
            frames_played: 0,
            cursor_frames: 0.0,
            finished: false,
        });
        // Ask for 16 device frames of mono @ matching rate; sound has 4.
        let mut out = vec![0.0f32; 16];
        fill_f32(&shared, &mut out, 1, 48_000);
        assert!(lock(&shared).playback.is_none());
    }

    #[test]
    fn repeating_sound_wraps_cursor() {
        let sound = silent_sound(1, 48_000, 4);
        let profile = VolumeProfile::new(1.0, Ramp::immediate(), true).unwrap();
        let shared = shared_with_playback(Playback {
            sound,
            profile,
            frames_played: 0,
            cursor_frames: 0.0,
            finished: false,
        });
        let mut out = vec![0.0f32; 20];
        fill_f32(&shared, &mut out, 1, 48_000);
        // Should still have playback: it loops.
        let sh = lock(&shared);
        let pb = sh.playback.as_ref().unwrap();
        assert!(!pb.finished);
        // Cursor stays within the 4-frame source (wrap is checked at iteration
        // start, so the final value can land at exactly `total_frames`).
        assert!(pb.cursor_frames <= 4.0);
        assert!(pb.frames_played >= 20);
    }

    #[test]
    fn mono_source_upmixes_to_stereo() {
        let sound = silent_sound(1, 48_000, 8);
        let profile = VolumeProfile::new(1.0, Ramp::immediate(), false).unwrap();
        let shared = shared_with_playback(Playback {
            sound,
            profile,
            frames_played: 0,
            cursor_frames: 0.0,
            finished: false,
        });
        let mut out = vec![0.0f32; 8]; // 4 frames × 2 channels
        fill_f32(&shared, &mut out, 2, 48_000);
        // L and R of each frame should be identical (mono duplicated).
        for frame in out.chunks(2) {
            assert_eq!(frame[0], frame[1]);
        }
    }

    #[test]
    fn linear_ramp_produces_increasing_gain() {
        // 1s of constant-amplitude mono at 48kHz, linearly ramped over 2s.
        // First sample near 0, last sample near (target * 0.5).
        let frames = 48_000;
        let sound = silent_sound(1, 48_000, frames);
        let profile = VolumeProfile::new(
            1.0,
            Ramp::linear(Duration::from_secs(2)).unwrap(),
            false,
        )
        .unwrap();
        let shared = shared_with_playback(Playback {
            sound,
            profile,
            frames_played: 0,
            cursor_frames: 0.0,
            finished: false,
        });
        let mut out = vec![0.0f32; frames];
        fill_f32(&shared, &mut out, 1, 48_000);

        let first = out[0].abs();
        let last = out[frames - 1].abs();
        assert!(first < 0.01, "expected near-silent start, got {first}");
        // After 1s of a 2s ramp, gain is ~0.5. Source amplitude is ~0.5 × i16::MAX
        // normalized to f32, so the product is ~0.125 ± slack.
        assert!(last > 0.1, "expected non-trivial end amplitude, got {last}");
        assert!(last > first * 10.0, "expected ramp to produce a clear increase");
    }
}
