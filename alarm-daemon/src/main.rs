#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use tracing::{error, info};

mod audio;
mod config;
mod daemon;
mod error;
mod ipc;
mod logging;
mod systemd;

use crate::{
    audio::{CpalAudio, CpalConfig, SoundLibrary},
    config::BusKind,
    daemon::{AlarmDaemon, Persistence},
    error::{AlarmError, Result},
    ipc::Control,
};

#[tokio::main]
async fn main() -> ExitCode {
    let log_backend = logging::init();

    match run(log_backend).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "alarm-daemon exited with error");
            ExitCode::FAILURE
        }
    }
}

async fn run(log_backend: logging::LogBackend) -> Result<()> {
    let bus_kind = BusKind::from_env()?;

    let db_path = std::env::var("ALARM_DAEMON_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| bus_kind.default_db_path());
    let persistence = Persistence::from_path(db_path);

    let (daemon, recovery) = AlarmDaemon::new_persistent(persistence.clone())
        .map_err(|e| AlarmError::Config(e.to_string()))?;
    if recovery.missed_while_down {
        info!(
            db_path = %persistence.path().display(),
            event = "MissedWhileDown",
            "startup recovery detected a missed alarm while daemon was down"
        );
    }

    let sounds = SoundLibrary::from_env();
    sounds
        .ensure_dirs()
        .map_err(|e| AlarmError::Config(e.to_string()))?;
    let player = Arc::new(CpalAudio::spawn(CpalConfig::default()).map_err(|e| {
        AlarmError::Config(format!("failed to initialize audio output: {e}"))
    })?);
    let control = Control::new(daemon, sounds, player);

    let builder = match bus_kind {
        BusKind::Session => zbus::connection::Builder::session()?,
        BusKind::System => zbus::connection::Builder::system()?,
    };
    let _connection = builder
        .name(ipc::BUS_NAME)?
        .serve_at(ipc::OBJECT_PATH, control)?
        .build()
        .await?;

    info!(
        bus = bus_kind.as_str(),
        db_path = %persistence.path().display(),
        name = ipc::BUS_NAME,
        path = ipc::OBJECT_PATH,
        interface = ipc::CONTROL_INTERFACE,
        logging = log_backend.as_str(),
        "alarm-daemon ready"
    );

    // Tell systemd we're up and start the watchdog ping loop. Both calls are
    // safe no-ops when not running under a notify-style systemd unit (e.g.
    // dev `cargo run`, integration tests, or the `Type=simple` user unit).
    systemd::notify_ready();
    let _watchdog = systemd::spawn_watchdog();

    let signal = wait_for_shutdown().await?;
    info!(signal, "shutdown signal received, exiting");
    systemd::notify_stopping();
    Ok(())
}

/// Waits for a graceful shutdown signal. Handles both `SIGINT` (developer
/// `Ctrl-C`) and `SIGTERM` (systemd / process supervisor stop), returning
/// the human-readable name of whichever fired first.
async fn wait_for_shutdown() -> Result<&'static str> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    Ok(tokio::select! {
        _ = sigint.recv()  => "SIGINT",
        _ = sigterm.recv() => "SIGTERM",
    })
}
