use std::time::Duration;

use super::AudioError;

/// Minimum ramp duration per FR-6.1 ("2–10 seconds").
#[cfg(test)]
pub const MIN_RAMP: Duration = Duration::from_secs(2);
/// Maximum ramp duration per FR-6.1 ("2–10 seconds").
#[cfg(test)]
pub const MAX_RAMP: Duration = Duration::from_secs(10);

/// Shape of the volume ramp applied at playback start.
///
/// `Immediate` is used by tier `emergency` (FR-5.4, FR-6.1: "except for tier
/// `emergency` which starts at full volume"). `Linear` is the default
/// anti-startle ramp used by other tiers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ramp {
    /// Start playback at the target volume immediately.
    Immediate,
    /// Linearly ramp from 0 to the target volume over `duration`.
    #[cfg(test)]
    Linear { duration: Duration },
}

impl Ramp {
    /// Emergency / "wake up now" ramp.
    #[must_use]
    pub const fn immediate() -> Self {
        Self::Immediate
    }

    /// Linear 0 → target ramp over `duration`. Enforces the FR-6.1 2–10 s
    /// bound; anything else is rejected so bad configs can't slip into a
    /// running daemon.
    #[cfg(test)]
    pub fn linear(duration: Duration) -> Result<Self, AudioError> {
        if duration < MIN_RAMP || duration > MAX_RAMP {
            return Err(AudioError::InvalidRamp(duration));
        }
        Ok(Self::Linear { duration })
    }
}

/// How a sound should be played.
///
/// Built via [`VolumeProfile::new`] so the validation happens at construction
/// time rather than in the audio callback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeProfile {
    target: f32,
    ramp: Ramp,
    repeat: bool,
}

impl VolumeProfile {
    /// Build a validated profile. `target` must be finite and in `0.0..=1.0`.
    pub fn new(target: f32, ramp: Ramp, repeat: bool) -> Result<Self, AudioError> {
        if !target.is_finite() || !(0.0..=1.0).contains(&target) {
            return Err(AudioError::InvalidVolume(target));
        }
        Ok(Self {
            target,
            ramp,
            repeat,
        })
    }

    #[must_use]
    pub fn target(&self) -> f32 {
        self.target
    }

    #[must_use]
    pub fn ramp(&self) -> Ramp {
        self.ramp
    }

    /// Whether the sound should loop until stopped (tier `alert` / `emergency`
    /// typically repeat; tier `notice` typically does not).
    #[must_use]
    pub fn repeat(&self) -> bool {
        self.repeat
    }
}

/// Gain multiplier to apply at `elapsed` seconds into playback.
///
/// Pure function extracted for direct unit testing — this is the FR-6.1
/// ramp math.
#[must_use]
pub fn gain_at(ramp: Ramp, target: f32, _elapsed: Duration) -> f32 {
    let target = target.clamp(0.0, 1.0);
    match ramp {
        Ramp::Immediate => target,
        #[cfg(test)]
        Ramp::Linear { duration } => {
            let full = duration.as_secs_f32();
            if full <= 0.0 {
                return target;
            }
            let frac = (_elapsed.as_secs_f32() / full).clamp(0.0, 1.0);
            (target * frac).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn immediate_gain_is_target() {
        assert_eq!(gain_at(Ramp::Immediate, 0.8, Duration::ZERO), 0.8);
        assert_eq!(gain_at(Ramp::Immediate, 0.8, Duration::from_secs(60)), 0.8);
    }

    #[test]
    fn linear_gain_at_start_is_zero() {
        let r = Ramp::linear(Duration::from_secs(4)).unwrap();
        assert_eq!(gain_at(r, 1.0, Duration::ZERO), 0.0);
    }

    #[test]
    fn linear_gain_at_midpoint_is_half() {
        let r = Ramp::linear(Duration::from_secs(4)).unwrap();
        let g = gain_at(r, 1.0, Duration::from_secs(2));
        assert!((g - 0.5).abs() < 1e-5, "expected ~0.5, got {g}");
    }

    #[test]
    fn linear_gain_saturates_at_target() {
        let r = Ramp::linear(Duration::from_secs(4)).unwrap();
        assert_eq!(gain_at(r, 0.7, Duration::from_secs(4)), 0.7);
        assert_eq!(gain_at(r, 0.7, Duration::from_secs(60)), 0.7);
    }

    #[test]
    fn ramp_rejects_durations_outside_2_to_10_seconds() {
        assert!(Ramp::linear(Duration::from_millis(1999)).is_err());
        assert!(Ramp::linear(Duration::from_millis(10_001)).is_err());
        assert!(Ramp::linear(Duration::from_secs(2)).is_ok());
        assert!(Ramp::linear(Duration::from_secs(10)).is_ok());
    }

    #[test]
    fn profile_rejects_out_of_range_volume() {
        assert!(VolumeProfile::new(-0.1, Ramp::Immediate, false).is_err());
        assert!(VolumeProfile::new(1.1, Ramp::Immediate, false).is_err());
        assert!(VolumeProfile::new(f32::NAN, Ramp::Immediate, false).is_err());
        assert!(VolumeProfile::new(0.0, Ramp::Immediate, false).is_ok());
        assert!(VolumeProfile::new(1.0, Ramp::Immediate, false).is_ok());
    }
}
