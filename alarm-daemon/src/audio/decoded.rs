use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::AudioError;

/// Fully decoded PCM audio held in memory.
///
/// Alarm sounds are short (seconds), so eager decode keeps the realtime
/// playback callback allocation- and decode-free. The [`Arc`] buffer lets
/// the same decoded sound be cheaply reused across fires.
#[derive(Debug, Clone)]
pub struct DecodedSound {
    samples: Arc<[f32]>,
    channels: u16,
    sample_rate: u32,
}

impl DecodedSound {
    /// Load and decode a WAV or FLAC file from disk (FR-6.1).
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        Self::decode(Box::new(file), hint)
    }

    /// Decode from an in-memory byte buffer. `extension` hints the format
    /// (e.g., `"wav"` / `"flac"`); symphonia also sniffs via magic bytes.
    #[allow(dead_code)]
    pub fn from_bytes(bytes: Vec<u8>, extension: Option<&str>) -> Result<Self, AudioError> {
        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }
        Self::decode(Box::new(Cursor::new(bytes)), hint)
    }

    fn decode(source: Box<dyn MediaSource>, hint: Hint) -> Result<Self, AudioError> {
        let mss = MediaSourceStream::new(source, Default::default());
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| AudioError::Decode(e.to_string()))?;
        let mut format = probed.format;

        let track = format
            .default_track()
            .ok_or_else(|| AudioError::Decode("no default track".into()))?;
        let track_id = track.id;
        let codec_params = track.codec_params.clone();

        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| AudioError::Decode("unknown sample rate".into()))?;
        let channels = codec_params
            .channels
            .map(|c| c.count())
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| AudioError::Decode("unknown or invalid channel layout".into()))?;

        let mut decoder = symphonia::default::get_codecs()
            .make(&codec_params, &DecoderOptions::default())
            .map_err(|e| AudioError::Decode(e.to_string()))?;

        let mut samples: Vec<f32> = Vec::new();
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(SymphoniaError::ResetRequired) => break,
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            };
            if packet.track_id() != track_id {
                continue;
            }
            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let capacity = decoded.capacity() as u64;
                    let mut buf = SampleBuffer::<f32>::new(capacity, spec);
                    buf.copy_interleaved_ref(decoded);
                    samples.extend_from_slice(buf.samples());
                }
                Err(SymphoniaError::DecodeError(e)) => {
                    tracing::warn!(error = %e, "audio: skipping corrupt packet");
                    continue;
                }
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            }
        }

        if samples.is_empty() {
            return Err(AudioError::Decode("decoded zero samples".into()));
        }
        if samples.len() % (channels as usize) != 0 {
            return Err(AudioError::Decode(format!(
                "decoded {} samples, not a multiple of {} channels",
                samples.len(),
                channels
            )));
        }

        Ok(Self {
            samples: Arc::from(samples.into_boxed_slice()),
            channels,
            sample_rate,
        })
    }

    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Build a minimal PCM WAV file in memory: 16-bit, little-endian,
    /// interleaved. Written out by hand so tests don't pull in `hound`.
    fn synth_wav(
        sample_rate: u32,
        channels: u16,
        samples_per_channel: usize,
        freq: f32,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        let byte_rate = sample_rate * u32::from(channels) * 2;
        let block_align = channels * 2;
        let data_size =
            u32::try_from(samples_per_channel * usize::from(channels) * 2).unwrap_or(u32::MAX);
        // RIFF
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36u32.saturating_add(data_size)).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        // fmt
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        // data
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for i in 0..samples_per_channel {
            let t = i as f32 / sample_rate as f32;
            let s = (t * freq * std::f32::consts::TAU).sin();
            let pcm = (s * f32::from(i16::MAX)) as i16;
            for _ in 0..channels {
                buf.extend_from_slice(&pcm.to_le_bytes());
            }
        }
        buf
    }

    #[test]
    fn decodes_mono_wav() {
        let wav = synth_wav(44_100, 1, 4_410, 440.0); // 100 ms
        let sound = DecodedSound::from_bytes(wav, Some("wav")).unwrap();
        assert_eq!(sound.channels(), 1);
        assert_eq!(sound.sample_rate(), 44_100);
        assert_eq!(sound.frames(), 4_410);
        assert_eq!(sound.samples().len(), 4_410);
    }

    #[test]
    fn decodes_stereo_wav() {
        let wav = synth_wav(48_000, 2, 480, 220.0); // 10 ms stereo
        let sound = DecodedSound::from_bytes(wav, Some("wav")).unwrap();
        assert_eq!(sound.channels(), 2);
        assert_eq!(sound.sample_rate(), 48_000);
        assert_eq!(sound.frames(), 480);
        assert_eq!(sound.samples().len(), 960);
    }

    #[test]
    fn rejects_garbage() {
        let err = DecodedSound::from_bytes(b"not audio".to_vec(), Some("wav"));
        assert!(err.is_err());
    }
}
