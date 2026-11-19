use std::collections::BTreeMap;
use std::sync::Arc;

/// The header a client is registered with to be told apart: the one identity every client can carry, an
/// MCP client of any protocol version as much as a hook. Self-declared and unauthenticated: a preference,
/// never a permission — read for the voice alone, nowhere else.
pub const CLIENT: &str = "x-evoice-client";

/// The voice each caller speaks with when it names none, by the identity it gives (`[daemon.voices]`).
/// Shared by every door that knows its caller: MCP and the hooks.
#[derive(Debug, Clone, Default)]
pub struct VoiceTable {
    voices: Arc<BTreeMap<String, String>>,
}

impl VoiceTable {
    #[must_use]
    pub fn new(voices: BTreeMap<String, String>) -> Self {
        Self {
            voices: Arc::new(voices),
        }
    }

    /// The voice `asked` for, else the one configured for `caller`, else none: the service's default.
    #[must_use]
    pub fn voice(&self, asked: Option<String>, caller: Option<&str>) -> Option<String> {
        asked.or_else(|| self.voices.get(caller?).cloned())
    }
}
