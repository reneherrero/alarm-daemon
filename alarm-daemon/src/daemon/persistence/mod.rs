//! redb-backed persistence of the daemon's externally observable state.
//!
//! ## Storage layout
//!
//! - Single redb file at `Persistence::path` (default
//!   `~/.local/state/helm/alarm-daemon.redb`).
//! - Single table `state`, single key `"v1"` (the table-key is stable; the
//!   *value* layout is versioned independently below).
//!
//! ## Value layout
//!
//! The encoded value carries a `b"AD"` magic + 1-byte version prefix so future
//! schema changes can be detected and either migrated or rejected. Today only
//! version 1 is defined:
//!
//! ```text
//! +-------+---+--------+--------------------+--------+--------------------+
//! | "AD"  | 1 | armed  | next_fire_unix_ms  | sound? | sound bytes        |
//! | 2 B   | 1 | 1 B    | tag(1) + i64 le    | tag(1) | u16 le len + bytes |
//! +-------+---+--------+--------------------+--------+--------------------+
//! ```
//!
//! `Persistence::load` also tolerates the unprefixed v0 layout shipped during
//! v0.1 development so existing dev databases keep working until their next
//! save rewrites them in the versioned form.

mod persistence_error;
mod persisted_state;
mod startup_recovery;

pub use persistence_error::PersistenceError;
pub use persisted_state::PersistedState;
pub use startup_recovery::StartupRecovery;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use redb::{Database, TableDefinition};

const STATE_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
const STATE_KEY: &str = "v1";

const SCHEMA_MAGIC: &[u8; 2] = b"AD";
const SCHEMA_VERSION: u8 = 1;

/// Handle to the redb-backed persistence file.
///
/// Cheap to clone — the underlying database handle is `Arc`-shared so the
/// daemon opens redb exactly once and reuses the connection across every
/// `load`/`save`. All redb calls are synchronous; async callers should hop
/// onto `tokio::task::spawn_blocking` before invoking them.
#[derive(Debug, Clone)]
pub struct Persistence {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
    db: Mutex<Option<Database>>,
}

impl Persistence {
    /// Builds a [`Persistence`] backed by `path`.
    ///
    /// The bus-aware default lives in `crate::config::BusKind::default_db_path`
    /// — `main.rs` consults `ALARM_DAEMON_DB_PATH` first, then falls back to
    /// that default. Persistence stays bus-agnostic so it can be reused from
    /// tests with arbitrary scratch paths.
    #[must_use]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::with_path(path.into())
    }

    /// Test-friendly alias; identical to [`Persistence::from_path`].
    #[must_use]
    #[cfg(test)]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::from_path(path)
    }

    fn with_path(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                path,
                db: Mutex::new(None),
            }),
        }
    }

    /// On-disk path of the redb file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Reads the persisted state. Returns the default state when the file does
    /// not exist or contains no value yet.
    #[allow(clippy::result_large_err)]
    pub fn load(&self) -> Result<PersistedState, PersistenceError> {
        if !self.inner.path.exists() {
            return Ok(PersistedState::default());
        }
        let guard = self.lock_db()?;
        let Some(db) = guard.as_ref() else {
            return Ok(PersistedState::default());
        };
        let read = db.begin_read()?;
        let table = match read.open_table(STATE_TABLE) {
            Ok(t) => t,
            // First save hasn't happened yet — treat as empty.
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(PersistedState::default()),
            Err(e) => return Err(e.into()),
        };
        let Some(raw) = table.get(STATE_KEY)? else {
            return Ok(PersistedState::default());
        };
        decode_state(raw.value())
    }

    /// Writes `state` to disk. Synchronous; call from `spawn_blocking` if the
    /// caller is on a tokio worker.
    #[allow(clippy::result_large_err)]
    pub fn save(&self, state: PersistedState) -> Result<(), PersistenceError> {
        let guard = self.lock_db()?;
        let Some(db) = guard.as_ref() else {
            return Err(PersistenceError::Corrupted(
                "database handle unavailable after lock_db".into(),
            ));
        };
        let write = db.begin_write()?;
        {
            let mut table = write.open_table(STATE_TABLE)?;
            let encoded = encode_state(&state);
            table.insert(STATE_KEY, encoded.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    /// Decides whether the daemon missed a fire while it was down (FR-7,
    /// `MissedWhileDown`).
    pub fn evaluate_recovery(&self, state: &PersistedState) -> StartupRecovery {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let missed_while_down =
            state.armed && state.next_fire_unix_ms.is_some_and(|t| t <= now_ms);
        StartupRecovery { missed_while_down }
    }

    /// Lazily opens the redb file on first use and caches the handle.
    ///
    /// `Database::create` opens an existing file or creates a new one, so we
    /// don't need to branch on `path.exists()`.
    #[allow(clippy::result_large_err)]
    fn lock_db(&self) -> Result<MutexGuard<'_, Option<Database>>, PersistenceError> {
        let mut guard = match self.inner.db.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            let path = &self.inner.path;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let db = Database::create(path)?;
            *guard = Some(db);
        }
        Ok(guard)
    }
}

fn encode_state(state: &PersistedState) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(SCHEMA_MAGIC);
    out.push(SCHEMA_VERSION);
    encode_body_v1(state, &mut out);
    out
}

fn encode_body_v1(state: &PersistedState, out: &mut Vec<u8>) {
    out.push(u8::from(state.armed));
    match state.next_fire_unix_ms {
        Some(v) => {
            out.push(1);
            out.extend_from_slice(&v.to_le_bytes());
        }
        None => out.push(0),
    }
    match &state.current_sound {
        Some(s) => {
            let bytes = s.as_bytes();
            // Sound IDs are short (`builtin:<word>` / `custom:<id>`); cap at u16
            // to keep the layout fixed-size, and warn if anyone ever sends a
            // pathological ID rather than silently truncating.
            let len = u16::try_from(bytes.len()).unwrap_or_else(|_| {
                tracing::warn!(
                    len = bytes.len(),
                    "persistence: sound id longer than 65535 bytes; truncating"
                );
                u16::MAX
            });
            out.push(1);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&bytes[..usize::from(len)]);
        }
        None => out.push(0),
    }
}

#[allow(clippy::result_large_err)]
fn decode_state(bytes: &[u8]) -> Result<PersistedState, PersistenceError> {
    if bytes.len() >= 3 && &bytes[0..2] == SCHEMA_MAGIC {
        let version = bytes[2];
        return match version {
            SCHEMA_VERSION => decode_body_v1(&bytes[3..]),
            v => Err(PersistenceError::Corrupted(format!(
                "unsupported schema version {v}"
            ))),
        };
    }
    // No magic: legacy v0.1 dev databases. Same body layout, no prefix.
    decode_body_v1(bytes)
}

#[allow(clippy::result_large_err)]
fn decode_body_v1(bytes: &[u8]) -> Result<PersistedState, PersistenceError> {
    if bytes.is_empty() {
        return Ok(PersistedState::default());
    }
    let armed = bytes[0] != 0;
    let mut cursor = 1usize;

    let next_fire_unix_ms = match bytes.get(cursor).copied() {
        Some(0) => {
            cursor += 1;
            None
        }
        Some(1) => {
            cursor += 1;
            let end = cursor.checked_add(8).ok_or_else(|| {
                PersistenceError::Corrupted("next_fire offset overflow".into())
            })?;
            let slice = bytes.get(cursor..end).ok_or_else(|| {
                PersistenceError::Corrupted("missing next_fire bytes".into())
            })?;
            let mut buf = [0u8; 8];
            buf.copy_from_slice(slice);
            cursor = end;
            Some(i64::from_le_bytes(buf))
        }
        Some(tag) => {
            return Err(PersistenceError::Corrupted(format!(
                "unknown next_fire tag: {tag}"
            )));
        }
        None => return Ok(PersistedState { armed, ..Default::default() }),
    };

    let current_sound = match bytes.get(cursor).copied() {
        Some(0) | None => None,
        Some(1) => {
            cursor += 1;
            let len_end = cursor.checked_add(2).ok_or_else(|| {
                PersistenceError::Corrupted("sound len offset overflow".into())
            })?;
            let len_slice = bytes.get(cursor..len_end).ok_or_else(|| {
                PersistenceError::Corrupted("missing sound length bytes".into())
            })?;
            let mut len_buf = [0u8; 2];
            len_buf.copy_from_slice(len_slice);
            let len = usize::from(u16::from_le_bytes(len_buf));
            cursor = len_end;
            let body_end = cursor.checked_add(len).ok_or_else(|| {
                PersistenceError::Corrupted("sound body offset overflow".into())
            })?;
            let body = bytes.get(cursor..body_end).ok_or_else(|| {
                PersistenceError::Corrupted("missing sound body bytes".into())
            })?;
            Some(
                String::from_utf8(body.to_vec())
                    .map_err(|e| PersistenceError::Corrupted(format!("sound id utf8: {e}")))?,
            )
        }
        Some(tag) => {
            return Err(PersistenceError::Corrupted(format!(
                "unknown sound tag: {tag}"
            )));
        }
    };

    Ok(PersistedState {
        armed,
        next_fire_unix_ms,
        current_sound,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_path() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("alarm-daemon-persist-{n}.redb"))
    }

    #[test]
    fn round_trip_state() {
        let path = unique_path();
        let p = Persistence::new(&path);
        p.save(PersistedState {
            armed: true,
            next_fire_unix_ms: Some(1_700_000_000_000),
            current_sound: Some("builtin:collision".to_string()),
        })
        .unwrap();
        let loaded = p.load().unwrap();
        assert!(loaded.armed);
        assert_eq!(loaded.next_fire_unix_ms, Some(1_700_000_000_000));
        assert_eq!(loaded.current_sound.as_deref(), Some("builtin:collision"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn round_trip_default_state() {
        let path = unique_path();
        let p = Persistence::new(&path);
        p.save(PersistedState::default()).unwrap();
        let loaded = p.load().unwrap();
        assert!(!loaded.armed);
        assert_eq!(loaded.next_fire_unix_ms, None);
        assert_eq!(loaded.current_sound, None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encoded_payload_starts_with_magic_and_version() {
        let mut out = Vec::new();
        out.extend_from_slice(SCHEMA_MAGIC);
        out.push(SCHEMA_VERSION);
        let encoded = encode_state(&PersistedState::default());
        assert!(encoded.starts_with(&out));
    }

    #[test]
    fn legacy_payload_without_magic_is_accepted() {
        // Hand-build the v0 body for `armed=true, next=None, sound=None`.
        let legacy = vec![1u8, 0u8, 0u8];
        let state = decode_state(&legacy).unwrap();
        assert!(state.armed);
        assert_eq!(state.next_fire_unix_ms, None);
        assert_eq!(state.current_sound, None);
    }

    #[test]
    fn unknown_schema_version_is_rejected() {
        let mut payload = Vec::new();
        payload.extend_from_slice(SCHEMA_MAGIC);
        payload.push(99);
        payload.push(0);
        let err = decode_state(&payload).unwrap_err();
        assert!(matches!(err, PersistenceError::Corrupted(_)));
    }

    #[test]
    fn truncated_body_is_reported_corrupted() {
        let mut payload = Vec::new();
        payload.extend_from_slice(SCHEMA_MAGIC);
        payload.push(SCHEMA_VERSION);
        payload.push(1); // armed
        payload.push(1); // has next_fire
        payload.extend_from_slice(&[0, 0, 0]); // only 3 of 8 bytes
        let err = decode_state(&payload).unwrap_err();
        assert!(matches!(err, PersistenceError::Corrupted(_)));
    }
}
