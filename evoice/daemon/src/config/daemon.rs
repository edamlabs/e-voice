use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The daemon's own table. `host` is never `[server] host`: the daemon owns the audio devices, so it
/// listens on loopback unless told otherwise. `tts` and `stt` are the services' WebSocket bases; `output`
/// and `input` pick a device by a part of its name, the system default when absent; `listen` is how long
/// one listen waits for a segment heard in full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub host: IpAddr,
    pub port: u16,
    pub tts: String,
    pub stt: String,
    pub output: Option<String>,
    pub input: Option<String>,
    #[serde(with = "humantime_serde")]
    pub listen: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 5700,
            tts: "ws://127.0.0.1:5600".to_owned(),
            stt: "ws://127.0.0.1:5500".to_owned(),
            output: None,
            input: None,
            listen: Duration::from_secs(15),
        }
    }
}
