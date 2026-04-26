use tokio::sync::OnceCell;
use zbus::{Connection, Proxy};

use crate::constants::{BUS_NAME, CONTROL_INTERFACE, OBJECT_PATH};
use crate::error::ClientError;

/// High-level client for the `org.helm.AlarmDaemon.Control` D-Bus interface.
///
/// The underlying [`zbus::Proxy`] is lazily constructed and cached on first
/// use, so repeated calls share a single proxy (and the cheap `Arc`-backed
/// connection it owns). All method calls are safe to invoke concurrently.
#[derive(Debug)]
pub struct AlarmDaemonClient {
    connection: Connection,
    proxy: OnceCell<Proxy<'static>>,
}

impl AlarmDaemonClient {
    /// Connect to the current user's session bus.
    pub async fn connect_session() -> Result<Self, ClientError> {
        let connection = Connection::session().await?;
        Ok(Self::from_connection(connection))
    }

    /// Connect to the system bus.
    pub async fn connect_system() -> Result<Self, ClientError> {
        let connection = Connection::system().await?;
        Ok(Self::from_connection(connection))
    }

    /// Wrap an existing zbus connection. The connection is not validated until
    /// the first method call (which lazily builds the proxy).
    #[must_use]
    pub fn from_connection(connection: Connection) -> Self {
        Self {
            connection,
            proxy: OnceCell::new(),
        }
    }

    /// List all available alarm sound ids (`builtin:*`, `custom:*`).
    pub async fn list_sounds(&self) -> Result<Vec<String>, ClientError> {
        let proxy = self.proxy().await?;
        let ids: Vec<String> = proxy.call("ListSounds", &()).await?;
        Ok(ids)
    }

    /// Arm the daemon using the provided sound id.
    pub async fn arm(&self, sound_id: &str) -> Result<(), ClientError> {
        let proxy = self.proxy().await?;
        proxy.call::<_, _, ()>("Arm", &(sound_id)).await?;
        Ok(())
    }

    /// Disarm the daemon.
    pub async fn disarm(&self) -> Result<(), ClientError> {
        let proxy = self.proxy().await?;
        proxy.call::<_, _, ()>("Disarm", &()).await?;
        Ok(())
    }

    /// Snooze the current alarm for `duration_s` seconds.
    pub async fn snooze(&self, duration_s: u32) -> Result<(), ClientError> {
        let proxy = self.proxy().await?;
        proxy.call::<_, _, ()>("Snooze", &(duration_s)).await?;
        Ok(())
    }

    /// Dismiss the current alarm (stop playback and clear armed state).
    pub async fn dismiss(&self) -> Result<(), ClientError> {
        let proxy = self.proxy().await?;
        proxy.call::<_, _, ()>("Dismiss", &()).await?;
        Ok(())
    }

    /// Return whether the daemon is currently armed.
    pub async fn status(&self) -> Result<bool, ClientError> {
        let proxy = self.proxy().await?;
        let armed: bool = proxy.call("Status", &()).await?;
        Ok(armed)
    }

    /// Return the currently selected sound id when armed.
    ///
    /// The daemon returns an empty string when no sound is selected; this
    /// helper normalises that to `None` so callers can pattern-match cleanly.
    pub async fn current_sound(&self) -> Result<Option<String>, ClientError> {
        let proxy = self.proxy().await?;
        let sound_id: String = proxy.call("CurrentSound", &()).await?;
        if sound_id.is_empty() {
            Ok(None)
        } else {
            Ok(Some(sound_id))
        }
    }

    /// Lazily build (and cache) the underlying proxy.
    async fn proxy(&self) -> Result<&Proxy<'static>, ClientError> {
        self.proxy
            .get_or_try_init(|| async {
                Proxy::new(&self.connection, BUS_NAME, OBJECT_PATH, CONTROL_INTERFACE)
                    .await
                    .map_err(ClientError::from)
            })
            .await
    }
}
