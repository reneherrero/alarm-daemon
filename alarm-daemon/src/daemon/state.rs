use crate::daemon::AlarmStatus;
use crate::daemon::persistence::PersistedState;

/// In-memory snapshot of the daemon's externally observable state.
///
/// `State` is the single source of truth shared between IPC handlers; the
/// persisted form ([`PersistedState`]) is derived from it on every transition.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct State {
    pub(crate) status: AlarmStatus,
    pub(crate) next_fire_unix_ms: Option<i64>,
    pub(crate) current_sound: Option<String>,
}

impl State {
    /// Disarmed state with no scheduled fire and no selected sound.
    pub(crate) fn disarmed() -> Self {
        Self {
            status: AlarmStatus::Disarmed,
            next_fire_unix_ms: None,
            current_sound: None,
        }
    }

    /// Armed state with the given selected sound and optional fire timestamp.
    pub(crate) fn armed(sound_id: Option<String>, next_fire_unix_ms: Option<i64>) -> Self {
        Self {
            status: AlarmStatus::Armed,
            next_fire_unix_ms,
            current_sound: sound_id,
        }
    }

    pub(crate) fn to_persisted(&self) -> PersistedState {
        PersistedState {
            armed: self.status.is_armed(),
            next_fire_unix_ms: self.next_fire_unix_ms,
            current_sound: self.current_sound.clone(),
        }
    }
}
