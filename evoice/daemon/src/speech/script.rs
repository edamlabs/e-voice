use e_voice_core::schema::lang::Lang;

/// What a voice is learned from and first heard with: the text read aloud to record it, about 25 s at a
/// calm pace and phonetically varied (every vowel, stress patterns, a question, a number in words), and
/// the sentence said in the new voice once it is learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceScript {
    Reading,
    Sample,
}

impl VoiceScript {
    /// The text, in `lang`. The match is over every pair, so a language without a text does not build.
    #[must_use]
    pub const fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Reading, Lang::Es) => {
                "Hoy he decidido grabar mi voz con calma. El sol de la mañana entra por la ventana y el café \
                 todavía está caliente. ¿Quién diría que una máquina pudiera aprender a hablar como yo? Cuento \
                 hasta cinco: uno, dos, tres, cuatro y cinco. Los jueves llueve en Zaragoza, pero el viernes \
                 hará buen tiempo junto al río."
            }
            (Self::Reading, Lang::En) => {
                "Today I decided to record my voice slowly and clearly. The morning light comes through the \
                 window and the coffee is still warm. Who would have thought a machine could learn to speak \
                 the way I do? I'll count to five: one, two, three, four and five. It often rains on \
                 Thursdays, but Friday should be bright by the river."
            }
            (Self::Sample, Lang::Es) => "Hola, así suena mi voz nueva.",
            (Self::Sample, Lang::En) => "Hello, this is how my new voice sounds.",
        }
    }
}
