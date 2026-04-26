//! Core daemon state.
//!
//! For this first step the daemon owns a single global armed/disarmed flag.
//! The end-goal model (per-alarm records keyed by UUIDv7, see FR-1.1) will
//! replace `State` with a registry; the IPC surface is shaped so that change
//! is additive.

mod alarm_daemon;
mod alarm_status;
mod persistence;
mod state;
mod transition;

pub use alarm_daemon::AlarmDaemon;
pub use alarm_status::AlarmStatus;
pub use persistence::{Persistence, PersistenceError, StartupRecovery};
pub(crate) use state::State;
pub use transition::Transition;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_disarmed() {
        let d = AlarmDaemon::new();
        assert_eq!(d.status().await, AlarmStatus::Disarmed);
        assert!(!d.status().await.is_armed());
    }

    #[tokio::test]
    async fn arm_then_disarm_reports_transitions() {
        let d = AlarmDaemon::new();

        let t = d.arm().await;
        assert_eq!(
            t,
            Transition::Changed {
                from: AlarmStatus::Disarmed,
                to: AlarmStatus::Armed,
            }
        );
        assert_eq!(d.status().await, AlarmStatus::Armed);

        let t = d.disarm().await;
        assert_eq!(
            t,
            Transition::Changed {
                from: AlarmStatus::Armed,
                to: AlarmStatus::Disarmed,
            }
        );
        assert_eq!(d.status().await, AlarmStatus::Disarmed);
    }

    #[tokio::test]
    async fn idempotent_arm_and_disarm_are_nochange() {
        let d = AlarmDaemon::new();
        let _ = d.arm().await;
        assert_eq!(d.arm().await, Transition::NoChange);
        let _ = d.disarm().await;
        assert_eq!(d.disarm().await, Transition::NoChange);
    }

    #[tokio::test]
    async fn persistent_state_round_trip_and_recovery_flag() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("alarm-daemon-state-{n}.redb"));
        let persistence = Persistence::new(&path);
        let (d, first_recovery) = AlarmDaemon::new_persistent(persistence.clone()).unwrap();
        assert!(!first_recovery.missed_while_down);
        let past_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 1_000;
        let _ = d.arm_with_next_fire(Some(past_ms)).await;
        assert!(d.current_sound().await.is_none());

        let (_d2, second_recovery) = AlarmDaemon::new_persistent(persistence).unwrap();
        assert!(second_recovery.missed_while_down);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn current_sound_round_trip_with_persistence() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("alarm-daemon-sound-{n}.redb"));
        let persistence = Persistence::new(&path);
        let (d, _recovery) = AlarmDaemon::new_persistent(persistence.clone()).unwrap();
        let _ = d
            .arm_with_sound("builtin:collision".to_string(), Some(1_700_000_000_000))
            .await;
        assert_eq!(d.current_sound().await.as_deref(), Some("builtin:collision"));

        let (d2, _recovery2) = AlarmDaemon::new_persistent(persistence).unwrap();
        assert_eq!(d2.current_sound().await.as_deref(), Some("builtin:collision"));
        let _ = d2.disarm().await;
        assert!(d2.current_sound().await.is_none());

        let _ = std::fs::remove_file(path);
    }
}
