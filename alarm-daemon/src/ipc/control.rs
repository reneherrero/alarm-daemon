use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::warn;
use zbus::{interface, object_server::SignalEmitter};

use crate::audio::{AlarmAudio, Ramp, SoundLibrary, VolumeProfile};
use crate::daemon::{AlarmDaemon, Transition};

/// Default delay between `Arm` and the alarm firing when
/// `ALARM_DAEMON_TRIGGER_DELAY_MS` is unset. Kept short so dev sessions hear
/// the alarm without configuration; the production scheduler will compute its
/// own delays from the requested fire time.
const DEFAULT_TRIGGER_DELAY: Duration = Duration::from_secs(3);

/// D-Bus control object exposing arm/disarm/status operations.
#[derive(Debug)]
pub struct Control {
    pub(super) daemon: AlarmDaemon,
    pub(super) sounds: SoundLibrary,
    pub(super) player: Arc<dyn AlarmAudio>,
    pub(super) trigger_delay: Duration,
    pub(super) trigger_task: Mutex<Option<JoinHandle<()>>>,
}

impl Control {
    /// Creates a new control object backed by the provided daemon state.
    #[must_use]
    pub fn new(daemon: AlarmDaemon, sounds: SoundLibrary, player: Arc<dyn AlarmAudio>) -> Self {
        let trigger_delay = std::env::var("ALARM_DAEMON_TRIGGER_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TRIGGER_DELAY);
        Self {
            daemon,
            sounds,
            player,
            trigger_delay,
            trigger_task: Mutex::new(None),
        }
    }

    /// Aborts a pending trigger task and stops any in-flight playback. Safe
    /// to call when nothing is scheduled or playing.
    pub(super) async fn cancel_pending(&self) {
        if let Some(handle) = self.trigger_task.lock().await.take() {
            handle.abort();
        }
        if let Err(e) = self.player.stop() {
            warn!(error = %e, "failed to stop player while cancelling trigger");
        }
    }

    /// Validates `sound_id`, arms the daemon, and schedules a single firing
    /// after `delay`.
    ///
    /// Always cancels any prior pending trigger and stops in-flight playback
    /// so callers (`Arm`, `Snooze`) get consistent reset semantics — re-arming
    /// never leaves a previous sound playing.
    pub(super) async fn schedule_trigger_with_delay(
        &self,
        sound_id: String,
        delay: Duration,
    ) -> zbus::fdo::Result<Transition> {
        let sound_path = self
            .sounds
            .resolve_path(&sound_id)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let fire_at = now_ms.saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));

        self.cancel_pending().await;

        let transition = self
            .daemon
            .arm_with_sound(sound_id, Some(fire_at))
            .await;

        let player = Arc::clone(&self.player);
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let decoded = match crate::audio::DecodedSound::from_path(&sound_path) {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    warn!(error = %e, "failed to decode selected alarm sound");
                    return;
                }
            };
            let profile = match VolumeProfile::new(1.0, Ramp::immediate(), false) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "failed to build playback profile");
                    return;
                }
            };
            if let Err(e) = player.play(decoded, profile) {
                warn!(error = %e, "failed to play selected alarm sound");
            }
        });
        *self.trigger_task.lock().await = Some(handle);
        Ok(transition)
    }

    pub(super) async fn schedule_trigger(&self, sound_id: String) -> zbus::fdo::Result<Transition> {
        self.schedule_trigger_with_delay(sound_id, self.trigger_delay).await
    }
}

#[allow(missing_docs)]
#[interface(name = "org.helm.AlarmDaemon.Control")]
impl Control {
    /// Arm the alarm for `sound_id` and schedule the trigger.
    ///
    /// `sound_id` must reference an installed sound:
    /// - `builtin:<name>`
    /// - `custom:<id>`
    async fn arm(
        &self,
        sound_id: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if let Transition::Changed { from, to } = self.schedule_trigger(sound_id.to_string()).await? {
            if from != to && let Err(e) = Self::state_changed(&emitter, to.is_armed()).await {
                warn!(error = %e, "failed to emit StateChanged after arm");
            }
        }
        Ok(())
    }

    /// Disarm the alarm. Idempotent in the same way as `Arm`.
    async fn disarm(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.cancel_pending().await;
        if let Transition::Changed { from, to } = self.daemon.disarm().await {
            if from != to && let Err(e) = Self::state_changed(&emitter, to.is_armed()).await {
                warn!(error = %e, "failed to emit StateChanged after disarm");
            }
        }
        Ok(())
    }

    /// Snooze the currently selected alarm sound by `duration_s` seconds.
    ///
    /// Stops current playback and re-schedules trigger with the same sound.
    async fn snooze(&self, duration_s: u32) -> zbus::fdo::Result<()> {
        if duration_s == 0 {
            return Err(zbus::fdo::Error::InvalidArgs(
                "duration_s must be > 0".to_string(),
            ));
        }
        let Some(sound_id) = self.daemon.current_sound().await else {
            return Err(zbus::fdo::Error::Failed(
                "cannot snooze: no armed alarm sound".to_string(),
            ));
        };
        let delay = Duration::from_secs(u64::from(duration_s));
        let _ = self.schedule_trigger_with_delay(sound_id, delay).await?;
        Ok(())
    }

    /// Dismiss the currently firing/armed alarm.
    ///
    /// Equivalent to explicit user acknowledgement: stop playback, clear pending
    /// trigger, and disarm.
    async fn dismiss(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.cancel_pending().await;
        if let Transition::Changed { from, to } = self.daemon.disarm().await {
            if from != to && let Err(e) = Self::state_changed(&emitter, to.is_armed()).await {
                warn!(error = %e, "failed to emit StateChanged after dismiss");
            }
        }
        Ok(())
    }

    /// Returns `true` when the alarm is armed, `false` otherwise.
    async fn status(&self) -> bool {
        self.daemon.status().await.is_armed()
    }

    /// Returns the currently selected sound id for the armed alarm.
    ///
    /// Empty string means no sound is currently selected.
    async fn current_sound(&self) -> String {
        self.daemon.current_sound().await.unwrap_or_default()
    }

    /// Lists sound IDs currently available to callers.
    ///
    /// IDs are namespaced (`builtin:<name>` / `custom:<id>`), and are suitable
    /// for use as alarm output sound selectors.
    async fn list_sounds(&self) -> zbus::fdo::Result<Vec<String>> {
        let sounds = self
            .sounds
            .list_sounds()
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(sounds.into_iter().map(|s| s.id).collect())
    }

    /// Emitted whenever the armed state actually changes.
    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, armed: bool) -> zbus::Result<()>;
}
