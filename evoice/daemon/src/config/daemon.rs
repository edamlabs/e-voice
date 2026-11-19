use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use serde::{Deserialize, Serialize};

/// The daemon's own table. `host` is never `[server] host`: the daemon owns the audio devices, so it
/// listens on loopback unless told otherwise. `tts` and `stt` are the services' WebSocket bases; `output`
/// and `input` pick a device by a part of its name, the system default when absent; `listen` is how long
/// one listen waits for someone to start speaking. `voices` maps an MCP client to the voice it speaks with
/// when a call names none: by its `X-Evoice-Client` header, which every client can send, else by the name
/// it declares, which counts only when the request itself carries it (protocol 2026-07-28 and later).
/// `lang` is the fallback language of what the hooks say: the notices, and a message whose language its
/// words do not decide.
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
    pub voices: BTreeMap<String, String>,
    pub lang: Lang,
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
            voices: BTreeMap::new(),
            lang: Lang::Es,
        }
    }
}
