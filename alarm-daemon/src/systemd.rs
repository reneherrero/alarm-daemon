//! systemd integration: `sd_notify(3)` ready/stopping/status messages and the
//! watchdog ping loop.
//!
//! The helpers here are intentionally tolerant of running outside systemd:
//! `notify_ready`, `notify_stopping`, and `notify_status` are no-ops when
//! `$NOTIFY_SOCKET` is unset (i.e. dev `cargo run`), and `spawn_watchdog`
//! returns `None` whenever the unit was started without `WatchdogSec=`.
//!
//! This keeps the developer/CI path identical to before — `setup.sh` and
//! integration tests don't need to opt into systemd to keep working — while
//! letting Yocto/system deployments use `Type=notify` + `WatchdogSec=` to
//! supervise daemon liveness.
//!
//! See `data/systemd/alarm-daemon.service` for the production-side wiring.

use std::time::Duration;

use sd_notify::NotifyState;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Returns `true` when the process appears to be running under a notify-style
/// systemd service (i.e. systemd has set `$NOTIFY_SOCKET`). Used purely to
/// pick the right log level when an `sd_notify` call fails: a failure under
/// systemd is a real problem worth a `warn!`, while the same failure on a
/// developer laptop is expected and demoted to `debug!`.
fn under_systemd_notify() -> bool {
    std::env::var_os("NOTIFY_SOCKET").is_some()
}

/// Sends `READY=1` to the service manager. Should be called exactly once,
/// after the daemon has finished startup (D-Bus name owned, audio thread
/// alive, persistence opened) so that systemd can correctly gate dependent
/// units behind us.
pub fn notify_ready() {
    notify_one(NotifyState::Ready, "READY=1");
}

/// Sends `STOPPING=1` to the service manager. Called from the shutdown path
/// to let systemd know the exit is intentional and not a crash.
pub fn notify_stopping() {
    notify_one(NotifyState::Stopping, "STOPPING=1");
}

fn notify_one(state: NotifyState<'_>, label: &str) {
    match sd_notify::notify(&[state]) {
        Ok(()) => debug!(notify = label, "sent systemd notification"),
        Err(e) if under_systemd_notify() => {
            warn!(notify = label, error = %e, "failed to send systemd notification")
        }
        Err(_) => debug!(
            notify = label,
            "not running under systemd notify; skipping"
        ),
    }
}

/// Spawns the systemd watchdog ping task on the current Tokio runtime if and
/// only if the unit was started with `WatchdogSec=`. Returns the
/// [`JoinHandle`] for the ping task on success, or `None` when the watchdog
/// is disabled — letting callers ignore the result on dev/CI runs.
///
/// The ping period is set to `WatchdogSec / 2`, matching the recommendation
/// in `sd_watchdog_enabled(3)`.
///
/// The ping runs as a Tokio task on the daemon's runtime so a runtime stall
/// (deadlock, blocked executor) prevents the ping from going out and lets
/// systemd kill + restart us — which is exactly the desired behavior.
pub fn spawn_watchdog() -> Option<JoinHandle<()>> {
    let watchdog_period = sd_notify::watchdog_enabled()?;
    let ping_period = ping_period_for(watchdog_period);

    info!(
        watchdog_period_ms = u64_ms(watchdog_period),
        ping_period_ms = u64_ms(ping_period),
        "systemd watchdog enabled; spawning ping task"
    );

    Some(tokio::spawn(watchdog_loop(ping_period)))
}

async fn watchdog_loop(ping_period: Duration) {
    let mut ticker = tokio::time::interval(ping_period);
    // tokio::time::interval fires immediately on the first tick. We just sent
    // READY=1 (which counts as a fresh ping), so consume that initial tick to
    // avoid double-pinging the moment we start.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match sd_notify::notify(&[NotifyState::Watchdog]) {
            Ok(()) => debug!("systemd watchdog pinged"),
            Err(e) => warn!(error = %e, "failed to send systemd watchdog ping"),
        }
    }
}

/// Computes the watchdog ping period from the configured `WatchdogSec=`
/// timeout. `sd_watchdog_enabled(3)` recommends pinging at half the timeout;
/// we clamp to a 1ms floor so a pathologically small WatchdogSec doesn't
/// turn the ping loop into a CPU hog (or, if it ever rounded to zero, panic
/// Tokio's timer).
fn ping_period_for(watchdog_period: Duration) -> Duration {
    (watchdog_period / 2).max(Duration::from_millis(1))
}

fn u64_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn ping_period_is_half_of_watchdog_period() {
        assert_eq!(
            ping_period_for(Duration::from_secs(10)),
            Duration::from_secs(5)
        );
        assert_eq!(
            ping_period_for(Duration::from_millis(200)),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn ping_period_clamps_to_one_millisecond_for_pathological_inputs() {
        assert_eq!(ping_period_for(Duration::from_millis(1)), Duration::from_millis(1));
        assert_eq!(ping_period_for(Duration::ZERO), Duration::from_millis(1));
    }

    #[test]
    fn u64_ms_handles_typical_durations() {
        assert_eq!(u64_ms(Duration::from_millis(0)), 0);
        assert_eq!(u64_ms(Duration::from_millis(123)), 123);
        assert_eq!(u64_ms(Duration::from_secs(60)), 60_000);
    }

    // Test the full no-op path that production code exercises when
    // NOTIFY_SOCKET is unset. We can't easily mutate the env without going
    // through `unsafe`, but we can at least verify the helpers don't panic
    // and return promptly under any environment the test process happens to
    // have. Cargo test runs strip NOTIFY_SOCKET in practice, so this is
    // effectively the "dev path".
    #[test]
    fn notify_helpers_do_not_panic_when_systemd_absent() {
        notify_ready();
        notify_stopping();
    }

    #[test]
    fn spawn_watchdog_returns_none_when_watchdog_disabled() {
        // In a normal `cargo test` invocation `WATCHDOG_USEC` and friends are
        // unset, so `sd_notify::watchdog_enabled()` should return `None` and
        // we should propagate that.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(spawn_watchdog().is_none());
        });
    }
}
