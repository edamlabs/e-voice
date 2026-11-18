use serde::{Deserialize, Serialize};

/// The daemon's own table: the port every local voice client reaches it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub port: u16,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self { port: 5700 }
    }
}
