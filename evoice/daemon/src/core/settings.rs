use std::path::Path;

use e_voice_core::config::server::ServerConfig;
use e_voice_core::settings::{Foreign, Settings as Loader, SettingsError};
use serde::{Deserialize, Serialize};

use crate::config::daemon::DaemonConfig;
use crate::workflow::listen::LONGEST;

/// The daemon's contract over the shared `evoice.toml` (e.g. `EVOICE_DAEMON__PORT=5701`). Every table
/// rejects unknown keys; `[stt]` and `[tts]` belong to the services.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub server: ServerConfig,
    pub daemon: DaemonConfig,
    pub stt: Foreign,
    pub tts: Foreign,
}

impl Settings {
    // ##### PRIVATE #####

    /// Clients find the daemon by its port, so the OS may not pick one; the WebSocket client is built
    /// without TLS, so `wss://` would only fail at the first request; the default listen is one the
    /// listener accepts.
    fn validate_rules(&self) -> [(bool, &'static str, &'static str); 4] {
        [
            (self.daemon.port >= 1, "daemon.port", "must be at least 1"),
            (
                self.daemon.tts.starts_with("ws://"),
                "daemon.tts",
                "must be a ws:// URL (TLS is not built in)",
            ),
            (
                self.daemon.stt.starts_with("ws://"),
                "daemon.stt",
                "must be a ws:// URL (TLS is not built in)",
            ),
            (
                !self.daemon.listen.is_zero() && self.daemon.listen <= LONGEST,
                "daemon.listen",
                "must be above zero and at most 300 s",
            ),
        ]
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// An explicit `path` must exist; without one, `evoice.toml` is read only if present.
    ///
    /// # Errors
    /// Missing explicit file, malformed TOML, unknown keys, ill-typed or out-of-range values.
    pub fn load(path: Option<&Path>) -> Result<Self, SettingsError> {
        let settings: Self = Loader::load(path)?;
        settings.validate()?;
        Ok(settings)
    }

    /// # Errors
    /// The first rule the values break.
    pub fn validate(&self) -> Result<(), SettingsError> {
        self.validate_rules()
            .into_iter()
            .find(|(ok, ..)| !ok)
            .map_or(Ok(()), |(_, key, reason)| {
                Err(SettingsError::Invalid {
                    key,
                    reason: reason.to_owned(),
                })
            })
    }
}
