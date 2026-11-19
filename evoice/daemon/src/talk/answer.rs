use e_voice_core::schema::lang::Lang;

/// A yes or a no, as heard: what a spoken answer to a question means. A negation anywhere makes it a no
/// (the safe side: «claro que no» refuses); otherwise its first words decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

impl Answer {
    // ##### PRIVATE #####

    /// The phrases that give this answer in `lang`, `|` apart, lowercased. A phrase matches the first
    /// words of what was heard, so "de acuerdo" needs both words; the two banks share no phrase.
    const fn of_phrases(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Yes, Lang::Es) => "sí|si|vale|adelante|hazlo|claro|dale|venga|ok|okay|de acuerdo",
            (Self::No, Lang::Es) => "no|nunca|para|cancela|espera|negativo",
            (Self::Yes, Lang::En) => "yes|yeah|yep|sure|ok|okay|go ahead|do it",
            (Self::No, Lang::En) => "no|nope|never|stop|cancel|wait|don't",
        }
    }

    /// The words that make any answer a no wherever they stand, `|` apart. Only plain negations: «para» or
    /// «wait» start a refusal but also mean other things inside a sentence, so they count first only.
    const fn of_negations(lang: Lang) -> &'static str {
        match lang {
            Lang::Es => "no|nunca|jamás",
            Lang::En => "no|not|never|don't|nope",
        }
    }

    /// A heard word as the banks spell it: lowercased, the typographic apostrophe as the plain one, with the
    /// punctuation around it gone.
    fn of_word(word: &str) -> String {
        word.to_lowercase()
            .replace('’', "'")
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
            .to_owned()
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// What `heard` answers, in `lang`; `None` when it holds no negation and its first words are neither a
    /// yes nor a no.
    #[must_use]
    pub fn of(heard: &str, lang: Lang) -> Option<Self> {
        let words: Vec<String> = heard
            .split_whitespace()
            .map(Self::of_word)
            .filter(|word| !word.is_empty())
            .collect();
        let negations: Vec<&str> = Self::of_negations(lang).split('|').collect();
        if words.iter().any(|word| negations.contains(&word.as_str())) {
            return Some(Self::No);
        }
        let says = |phrase: &str| {
            let phrase: Vec<&str> = phrase.split(' ').collect();
            words.len() >= phrase.len() && words.iter().zip(&phrase).all(|(word, said)| word == said)
        };
        [Self::Yes, Self::No]
            .into_iter()
            .find(|answer| answer.of_phrases(lang).split('|').any(says))
    }
}
