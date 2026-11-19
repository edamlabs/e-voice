use e_voice_core::schema::lang::Lang;

/// What the conversation says on its own, besides the agent's replies and its questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalkNotice {
    /// The conversation ends: a goodbye, or nobody spoke for a while.
    Goodbye,
    /// The agent's process ended mid-turn; the next turn takes the session up again.
    Lost,
    /// The agent did not start as the daemon asked; the conversation ends.
    Refused,
    /// The agent's turn failed.
    Failed,
    /// The agent's turn ran past its bound and was cut.
    TooLong,
    /// Several requests in a row could not be read aloud to be approved.
    Unreadable,
    /// The answer to a permission question was neither a yes nor a no.
    Again,
}

impl TalkNotice {
    /// The phrase, in `lang`. The match is over every pair, so a language without a phrase does not build.
    #[must_use]
    pub const fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Goodbye, Lang::Es) => "Hasta luego.",
            (Self::Goodbye, Lang::En) => "Goodbye.",
            (Self::Lost, Lang::Es) => "Se ha cortado el agente; sigo.",
            (Self::Lost, Lang::En) => "The agent dropped; carrying on.",
            (Self::Refused, Lang::Es) => "El agente no arrancó como debía; termino.",
            (Self::Refused, Lang::En) => "The agent did not start as it should; ending.",
            (Self::Failed, Lang::Es) => "Algo ha fallado en el agente.",
            (Self::Failed, Lang::En) => "Something failed in the agent.",
            (Self::TooLong, Lang::Es) => "Llevaba demasiado tiempo; lo he cortado.",
            (Self::TooLong, Lang::En) => "It was taking too long; I cut it.",
            (Self::Unreadable, Lang::Es) => "Le he pedido que lo simplifique; no podía leértelo.",
            (Self::Unreadable, Lang::En) => "I asked it to simplify; I could not read it to you.",
            (Self::Again, Lang::Es) => "¿Sí o no?",
            (Self::Again, Lang::En) => "Yes or no?",
        }
    }

    /// The phrases that end a conversation, `|` apart: said as a whole utterance, never as the start of one.
    const fn bye_phrases(lang: Lang) -> &'static str {
        match lang {
            Lang::Es => "adiós|hasta luego|terminamos",
            Lang::En => "goodbye|bye|that's all",
        }
    }

    /// Whether `heard` is a goodbye: the whole utterance, lowercased and without the punctuation around it
    /// («¡Adiós!»), is one of the phrases. «adiós a todos» or «para…» are turns.
    #[must_use]
    pub fn bye(heard: &str, lang: Lang) -> bool {
        let said = heard
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
            .replace('’', "'");
        Self::bye_phrases(lang).split('|').any(|phrase| phrase == said)
    }
}
