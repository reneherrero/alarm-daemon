use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::daemon::{AlarmStatus, Persistence, PersistenceError, StartupRecovery, State, Transition};

/// In-memory daemon state shared by the IPC handlers.
#[derive(Debug, Clone, Default)]
pub struct AlarmDaemon {
    inner: Arc<RwLock<State>>,
    persistence: Option<Persistence>,
}

impl AlarmDaemon {
    /// Creates a daemon with the alarm in the disarmed state and no
    /// persistence backing (test-only constructor).
    #[must_use]
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a daemon backed by redb persistence and returns startup
    /// recovery info.
    #[allow(clippy::result_large_err)]
    pub fn new_persistent(
        persistence: Persistence,
    ) -> Result<(Self, StartupRecovery), PersistenceError> {
        let persisted = persistence.load()?;
        let recovery = persistence.evaluate_recovery(&persisted);
        if recovery.missed_while_down {
            warn!("missed alarm while daemon was down; recovery flag set");
        }
        let status = if persisted.armed {
            AlarmStatus::Armed
        } else {
            AlarmStatus::Disarmed
        };
        let state = State {
            status,
            next_fire_unix_ms: persisted.next_fire_unix_ms,
            current_sound: persisted.current_sound,
        };
        Ok((
            Self {
                inner: Arc::new(RwLock::new(state)),
                persistence: Some(persistence),
            },
            recovery,
        ))
    }

    /// Arms the daemon with no specific sound (test-only convenience).
    #[cfg(test)]
    pub async fn arm(&self) -> Transition {
        self.transition_to(State::armed(None, None), "alarm armed (test)")
            .await
    }

    /// Arms the daemon with no specific sound but a known fire timestamp
    /// (test-only; production callers go through `arm_with_sound`).
    #[cfg(test)]
    pub async fn arm_with_next_fire(&self, next_fire_unix_ms: Option<i64>) -> Transition {
        self.transition_to(
            State::armed(None, next_fire_unix_ms),
            "alarm armed (test)",
        )
        .await
    }

    /// Arms the daemon with a specific selected sound id and fire timestamp.
    pub async fn arm_with_sound(
        &self,
        sound_id: String,
        next_fire_unix_ms: Option<i64>,
    ) -> Transition {
        self.transition_to(
            State::armed(Some(sound_id), next_fire_unix_ms),
            "alarm armed",
        )
        .await
    }

    /// Disarms the daemon, clearing both the fire timestamp and selected sound.
    pub async fn disarm(&self) -> Transition {
        self.transition_to(State::disarmed(), "alarm disarmed").await
    }

    /// Returns the current daemon status.
    pub async fn status(&self) -> AlarmStatus {
        self.inner.read().await.status
    }

    /// Returns currently armed/selected sound id, if any.
    pub async fn current_sound(&self) -> Option<String> {
        self.inner.read().await.current_sound.clone()
    }

    /// Atomically swaps the in-memory state to `desired` and (best-effort)
    /// persists the result.
    ///
    /// Persistence runs on a blocking thread pool so the redb commit does not
    /// stall the tokio runtime. Failures are logged but do not roll back the
    /// in-memory transition: the caller already observed the state change in
    /// the prior `Status` reply, and the next transition will retry the write.
    async fn transition_to(&self, desired: State, message: &'static str) -> Transition {
        let mut state = self.inner.write().await;
        if *state == desired {
            return Transition::NoChange;
        }
        let from = state.status;
        *state = desired;
        let snapshot = state.to_persisted();
        let to = state.status;
        drop(state);

        if let Some(persistence) = self.persistence.clone() {
            // Box the per-call result so the heap-large `PersistenceError`
            // doesn't push spawn_blocking's `Result` over clippy's
            // `result_large_err` threshold.
            let join = tokio::task::spawn_blocking(move || {
                persistence
                    .save(snapshot)
                    .map_err(Box::new)
            })
            .await;
            match join {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "failed to persist daemon state"),
                Err(e) => warn!(error = %e, "persistence task panicked"),
            }
        }

        info!(from = ?from, to = ?to, event = message);
        Transition::Changed { from, to }
    }
}
