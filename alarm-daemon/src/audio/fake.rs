use std::sync::{Arc, Mutex};

use super::{AlarmAudio, AudioError, DecodedSound, VolumeProfile};

/// Event recorded by [`FakeAudio`] for test assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeEvent {
    Play {
        frames: usize,
        channels: u16,
        sample_rate: u32,
        profile: VolumeProfile,
    },
    Stop,
}

/// In-memory [`AlarmAudio`] for tests (NFR-6.2). Records every call as a
/// [`FakeEvent`]; does not produce audio.
#[derive(Debug, Default, Clone)]
pub struct FakeAudio {
    events: Arc<Mutex<Vec<FakeEvent>>>,
}

impl FakeAudio {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of all recorded events so far.
    #[must_use]
    pub fn events(&self) -> Vec<FakeEvent> {
        let guard = match self.events.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }

    fn push(&self, ev: FakeEvent) {
        let mut guard = match self.events.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.push(ev);
    }
}

impl AlarmAudio for FakeAudio {
    fn play(&self, sound: Arc<DecodedSound>, profile: VolumeProfile) -> Result<(), AudioError> {
        self.push(FakeEvent::Play {
            frames: sound.frames(),
            channels: sound.channels(),
            sample_rate: sound.sample_rate(),
            profile,
        });
        Ok(())
    }

    fn stop(&self) -> Result<(), AudioError> {
        self.push(FakeEvent::Stop);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::super::{Ramp, VolumeProfile};
    use super::*;

    fn tiny_sound() -> Arc<DecodedSound> {
        // 1 frame, 1 channel, 8 kHz; enough to populate the event fields.
        let wav = {
            let mut b = Vec::new();
            let sr: u32 = 8_000;
            let ch: u16 = 1;
            let data_size: u32 = 2;
            b.extend_from_slice(b"RIFF");
            b.extend_from_slice(&(36u32 + data_size).to_le_bytes());
            b.extend_from_slice(b"WAVE");
            b.extend_from_slice(b"fmt ");
            b.extend_from_slice(&16u32.to_le_bytes());
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&ch.to_le_bytes());
            b.extend_from_slice(&sr.to_le_bytes());
            b.extend_from_slice(&(sr * u32::from(ch) * 2).to_le_bytes());
            b.extend_from_slice(&(ch * 2).to_le_bytes());
            b.extend_from_slice(&16u16.to_le_bytes());
            b.extend_from_slice(b"data");
            b.extend_from_slice(&data_size.to_le_bytes());
            b.extend_from_slice(&0i16.to_le_bytes());
            b
        };
        Arc::new(DecodedSound::from_bytes(wav, Some("wav")).unwrap())
    }

    #[test]
    fn records_play_and_stop() {
        let fake = FakeAudio::new();
        let sound = tiny_sound();
        let profile = VolumeProfile::new(
            0.8,
            Ramp::linear(Duration::from_secs(3)).unwrap(),
            true,
        )
        .unwrap();

        fake.play(Arc::clone(&sound), profile).unwrap();
        fake.stop().unwrap();

        let events = fake.events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], FakeEvent::Play { frames: 1, channels: 1, sample_rate: 8000, .. }));
        assert_eq!(events[1], FakeEvent::Stop);
    }
}
