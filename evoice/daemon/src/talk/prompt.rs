use e_voice_core::schema::lang::Lang;

/// What the agent is told, besides its own system prompt, when it is being talked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalkPrompt {
    /// Its replies are heard, not read: short spoken sentences, nothing that only reads on a screen.
    Voice,
}

impl TalkPrompt {
    /// The prompt, in `lang`. The match is over every pair, so a language without a prompt does not build.
    #[must_use]
    pub const fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Voice, Lang::Es) => {
                "Te están escuchando, no leyendo: contesta en frases cortas pensadas para decirse en voz alta, \
                 sin markdown, sin código y sin listas. Antes de una respuesta larga, pregunta si la quieren. \
                 Di qué has hecho, no cómo."
            }
            (Self::Voice, Lang::En) => {
                "You are being heard, not read: answer in short sentences meant to be spoken, with no markdown, \
                 code or lists. Before a long answer, ask whether it is wanted. Say what you did, not how."
            }
        }
    }
}
