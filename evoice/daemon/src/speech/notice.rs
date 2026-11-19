use e_voice_core::schema::lang::Lang;
use serde::Deserialize;

/// What Claude Code's `Notification` hook tells the daemon to say aloud, read from its `notification_type`
/// (the wire words are the serde names). Any other type is not a notice: it fails to parse, and nothing is
/// said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Notice {
    /// A tool use or a command waits for the user's approval.
    #[serde(rename = "permission_prompt")]
    Permission,
    /// Claude finished a while ago and the user has not answered.
    #[serde(rename = "idle_prompt")]
    Waiting,
}

impl Notice {
    /// The phrase, in `lang`. The match is over every pair, so a language without a phrase does not build.
    #[must_use]
    pub const fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Permission, Lang::Es) => "Necesito tu permiso.",
            (Self::Permission, Lang::En) => "I need your permission.",
            (Self::Waiting, Lang::Es) => "Estoy esperando.",
            (Self::Waiting, Lang::En) => "I'm waiting for you.",
        }
    }
}
