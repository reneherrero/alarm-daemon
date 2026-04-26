use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::AudioError;

pub const DEFAULT_BUILTIN_SOUND_DIR: &str = "/usr/share/helm/sounds";
pub const DEFAULT_CUSTOM_SOUND_DIR: &str = "/var/lib/helm/sounds";

/// Supported sound file formats. Used both to filter directory listings and
/// (in tests) to write/remove custom sounds with the right extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoundFormat {
    Wav,
    Flac,
}

impl SoundFormat {
    fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("wav") => Some(Self::Wav),
            Some("flac") => Some(Self::Flac),
            _ => None,
        }
    }

    #[cfg(test)]
    fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
        }
    }
}

/// Origin of a sound asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundSource {
    /// Bundled with the daemon (read-only, lives in `builtin_dir`).
    Builtin,
    /// User-supplied (lives in `custom_dir`).
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundSummary {
    pub id: String,
    pub name: String,
    pub source: SoundSource,
    pub bytes: u64,
}

/// Read-mostly view over the daemon's installed sound assets.
///
/// Two on-disk locations:
/// - `builtin_dir`: shipped with the daemon image (read-only at runtime).
/// - `custom_dir`: user-managed sounds dropped in by an out-of-band tool.
///
/// The library exposes the read paths used by the D-Bus surface (`list_sounds`,
/// `resolve_path`); programmatic registration is intentionally not part of the
/// production API yet (no polkit, no D-Bus method) and lives behind
/// `#[cfg(test)]`.
#[derive(Debug, Clone)]
pub struct SoundLibrary {
    builtin_dir: PathBuf,
    custom_dir: PathBuf,
}

impl SoundLibrary {
    #[must_use]
    pub fn from_env() -> Self {
        let builtin = std::env::var("ALARM_DAEMON_SOUND_DIR")
            .unwrap_or_else(|_| DEFAULT_BUILTIN_SOUND_DIR.to_string());
        let custom = std::env::var("ALARM_DAEMON_CUSTOM_SOUND_DIR")
            .unwrap_or_else(|_| DEFAULT_CUSTOM_SOUND_DIR.to_string());
        Self::new(builtin, custom)
    }

    #[must_use]
    pub fn new(builtin_dir: impl Into<PathBuf>, custom_dir: impl Into<PathBuf>) -> Self {
        Self {
            builtin_dir: builtin_dir.into(),
            custom_dir: custom_dir.into(),
        }
    }

    /// Ensures the custom sound directory exists. Built-in directory is owned
    /// by the package and is not auto-created.
    pub fn ensure_dirs(&self) -> Result<(), AudioError> {
        fs::create_dir_all(&self.custom_dir)?;
        Ok(())
    }

    /// Lists every sound the daemon can play, with builtins listed before
    /// customs (sort key is the namespaced id).
    pub fn list_sounds(&self) -> Result<Vec<SoundSummary>, AudioError> {
        let mut sounds = BTreeMap::<String, SoundSummary>::new();
        self.collect(&self.builtin_dir, SoundSource::Builtin, &mut sounds)?;
        self.collect(&self.custom_dir, SoundSource::Custom, &mut sounds)?;
        Ok(sounds.into_values().collect())
    }

    /// Resolves a namespaced `sound_id` to an on-disk path.
    pub fn resolve_path(&self, sound_id: &str) -> Result<PathBuf, AudioError> {
        if let Some(name) = sound_id.strip_prefix("builtin:") {
            return Self::resolve_in(&self.builtin_dir, "builtin", name);
        }
        if let Some(name) = sound_id.strip_prefix("custom:") {
            return Self::resolve_in(&self.custom_dir, "custom", name);
        }
        Err(AudioError::InvalidSoundId(sound_id.to_string()))
    }

    fn collect(
        &self,
        dir: &Path,
        source: SoundSource,
        sounds: &mut BTreeMap<String, SoundSummary>,
    ) -> Result<(), AudioError> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if SoundFormat::from_path(&path).is_none() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let metadata = fs::metadata(&path)?;
            let prefix = match source {
                SoundSource::Builtin => "builtin",
                SoundSource::Custom => "custom",
            };
            let id = format!("{prefix}:{stem}");
            sounds.insert(
                id.clone(),
                SoundSummary {
                    id,
                    name: stem.to_string(),
                    source,
                    bytes: metadata.len(),
                },
            );
        }
        Ok(())
    }

    fn resolve_in(dir: &Path, prefix: &str, name: &str) -> Result<PathBuf, AudioError> {
        for ext in ["wav", "flac"] {
            let path = dir.join(format!("{name}.{ext}"));
            if path.exists() {
                return Ok(path);
            }
        }
        Err(AudioError::UnknownSound(format!("{prefix}:{name}")))
    }
}

#[cfg(test)]
mod test_helpers {
    //! Programmatic registration / decoding helpers used only by tests.
    //!
    //! Kept out of the production surface until there's a D-Bus method backed
    //! by polkit to expose them.

    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{AudioError, SoundFormat, SoundLibrary};
    use crate::audio::DecodedSound;

    /// Hard cap on programmatically registered sounds.
    pub const MAX_CUSTOM_SOUND_BYTES: usize = 5 * 1024 * 1024;

    impl SoundLibrary {
        pub(crate) fn register_sound(
            &self,
            name: &str,
            data: &[u8],
        ) -> Result<String, AudioError> {
            if data.len() > MAX_CUSTOM_SOUND_BYTES {
                return Err(AudioError::SoundTooLarge {
                    size: data.len(),
                    max: MAX_CUSTOM_SOUND_BYTES,
                });
            }
            let format = sniff_format(data)?;
            let _ = DecodedSound::from_bytes(data.to_vec(), Some(format.extension()))?;
            self.ensure_dirs()?;

            let id = make_custom_id(name);
            let file_name = format!("{id}.{}", format.extension());
            let path = self.custom_dir().join(file_name);
            let tmp = path.with_extension(format!("{}.tmp", format.extension()));
            fs::write(&tmp, data)?;
            fs::rename(&tmp, &path)?;
            Ok(format!("custom:{id}"))
        }

        pub(crate) fn unregister_sound(&self, sound_id: &str) -> Result<(), AudioError> {
            let Some(id) = sound_id.strip_prefix("custom:") else {
                return Err(AudioError::InvalidSoundId(sound_id.to_string()));
            };
            let mut removed_any = false;
            for ext in ["wav", "flac"] {
                let path = self.custom_dir().join(format!("{id}.{ext}"));
                if path.exists() {
                    fs::remove_file(&path)?;
                    removed_any = true;
                }
            }
            if removed_any {
                Ok(())
            } else {
                Err(AudioError::UnknownSound(sound_id.to_string()))
            }
        }

        pub(crate) fn load_decoded(&self, sound_id: &str) -> Result<DecodedSound, AudioError> {
            let path = self.resolve_path(sound_id)?;
            DecodedSound::from_path(path)
        }

        pub(crate) fn custom_dir(&self) -> &std::path::Path {
            &self.custom_dir
        }
    }

    fn make_custom_id(name: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut cleaned = String::with_capacity(name.len());
        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                cleaned.push(c.to_ascii_lowercase());
            } else if c == '-' || c == '_' {
                cleaned.push(c);
            }
        }
        if cleaned.is_empty() {
            cleaned.push_str("sound");
        }
        format!("{cleaned}-{now:x}")
    }

    fn sniff_format(data: &[u8]) -> Result<SoundFormat, AudioError> {
        if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
            return Ok(SoundFormat::Wav);
        }
        if data.len() >= 4 && &data[0..4] == b"fLaC" {
            return Ok(SoundFormat::Flac);
        }
        Err(AudioError::UnsupportedFormat(
            "expected WAV (RIFF/WAVE) or FLAC (fLaC)".to_string(),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn synth_wav() -> Vec<u8> {
        let mut buf = Vec::new();
        let sr: u32 = 8_000;
        let ch: u16 = 1;
        let data_size: u32 = 2 * 128;
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36u32 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&ch.to_le_bytes());
        buf.extend_from_slice(&sr.to_le_bytes());
        buf.extend_from_slice(&(sr * u32::from(ch) * 2).to_le_bytes());
        buf.extend_from_slice(&(ch * 2).to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for _ in 0..128 {
            buf.extend_from_slice(&(i16::MAX / 4).to_le_bytes());
        }
        buf
    }

    fn unique_temp_dir(suffix: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("alarm-daemon-{suffix}-{n}"))
    }

    #[test]
    fn register_list_resolve_unregister_round_trip() {
        let builtin = unique_temp_dir("builtin");
        let custom = unique_temp_dir("custom");
        fs::create_dir_all(&builtin).unwrap();
        fs::create_dir_all(&custom).unwrap();
        let wav = synth_wav();
        fs::write(builtin.join("collision.wav"), &wav).unwrap();

        let lib = SoundLibrary::new(&builtin, &custom);
        let sound_id = lib.register_sound("watch alarm", &wav).unwrap();
        assert!(sound_id.starts_with("custom:watchalarm-"));
        let listed = lib.list_sounds().unwrap();
        assert!(listed.iter().any(|s| s.id == "builtin:collision"));
        assert!(listed.iter().any(|s| s.id == sound_id));

        let path = lib.resolve_path(&sound_id).unwrap();
        assert!(path.exists());
        let decoded = lib.load_decoded(&sound_id).unwrap();
        assert!(decoded.frames() > 0);

        lib.unregister_sound(&sound_id).unwrap();
        assert!(lib.resolve_path(&sound_id).is_err());

        let _ = fs::remove_dir_all(&builtin);
        let _ = fs::remove_dir_all(&custom);
    }

    #[test]
    fn rejects_unsupported_bytes() {
        let lib = SoundLibrary::new("/nonexistent/a", "/nonexistent/b");
        let err = lib.register_sound("bad", b"not-a-wave").unwrap_err();
        assert!(matches!(err, AudioError::UnsupportedFormat(_)));
    }

    #[test]
    fn unregister_removes_all_format_variants() {
        let builtin = unique_temp_dir("builtin");
        let custom = unique_temp_dir("custom");
        fs::create_dir_all(&builtin).unwrap();
        fs::create_dir_all(&custom).unwrap();
        // Drop a colliding pair manually to simulate a stale state.
        fs::write(custom.join("dup.wav"), synth_wav()).unwrap();
        fs::write(custom.join("dup.flac"), b"placeholder").unwrap();

        let lib = SoundLibrary::new(&builtin, &custom);
        lib.unregister_sound("custom:dup").unwrap();
        assert!(!custom.join("dup.wav").exists());
        assert!(!custom.join("dup.flac").exists());

        let _ = fs::remove_dir_all(&builtin);
        let _ = fs::remove_dir_all(&custom);
    }
}
