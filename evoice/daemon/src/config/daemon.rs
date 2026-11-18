use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

/// The daemon's own table. `host` is never `[server] host`: the daemon owns the audio devices, so it
/// listens on loopback unless told otherwise. `tts` is the TTS service's WebSocket base; `output` picks
/// the output device by a part of its name, the system default when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub host: IpAddr,
    pub port: u16,
    pub tts: String,
    pub output: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 5700,
            tts: "ws://127.0.0.1:5600".to_owned(),
            output: None,
        }
    }
}
