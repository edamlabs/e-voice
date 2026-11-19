use e_voice_core::schema::lang::Lang;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// The most sentences said of a message: its opening, which is where a reply says what it did.
pub const SENTENCES: usize = 2;

/// The most characters said of a message, about 20 s of speech; a longer opening is cut at a word.
pub const CHARS: usize = 300;

/// The longest inline code read aloud as a word.
const WORD: usize = 24;

/// Marks that close a sentence when whitespace or the end follows them.
const ENDS: [char; 4] = ['.', '!', '?', '…'];

/// Punctuation a voice can read, as pauses and tone: the only characters said besides letters, digits and
/// spaces. Everything else is left out, whatever it is: an allow-list, so no unnamed symbol slips through.
pub const SPEAKABLE: &str = ".,;:!?…¿¡'\"«»“”‘’()-–—%";

/// How a path anchored somewhere begins: such a word is read as its file name.
const ROOTS: [&str; 4] = ["~/", "./", "../", "/"];

/// What one token of prose becomes: a word to say, or the sentence marks left of a token that is not said.
enum Piece {
    Word(String),
    Marks(String),
}

/// Punctuation that may open a word before its letters.
const OPENERS: [char; 9] = ['(', '[', '<', '"', '\'', '«', '“', '¿', '¡'];

/// Punctuation that may close a word after its letters, the sentence marks included.
const CLOSERS: [char; 14] = ['.', ',', ';', ':', '!', '?', '…', ')', ']', '>', '"', '\'', '»', '”'];

/// A spoken digest of a markdown reply: its first [`SENTENCES`] sentences of prose, at most [`CHARS`]
/// characters, and the language they are in. The parser leaves out code blocks, tables, headings, images,
/// HTML and footnotes; of the prose, only words are said: letters, digits and [`SPEAKABLE`] punctuation.
/// A URL is not said, a path is read as its file name, inline code only when it is a word or a file name.
#[derive(Debug, Default)]
pub struct SpeechDigest {
    blocks: Vec<String>,
    current: String,
    hidden: usize,
}

impl SpeechDigest {
    // ##### PRIVATE #####

    /// One parser event: prose is gathered, a block boundary closes what was gathered, and everything
    /// inside an element that does not read aloud is skipped, however deep.
    fn of_event(mut self, event: Event<'_>) -> Self {
        match event {
            Event::Start(tag) if Self::of_hidden(&tag) => self.hidden = self.hidden.saturating_add(1),
            Event::End(end) if Self::of_hidden_end(end) => self.hidden = self.hidden.saturating_sub(1),
            _ if self.hidden > 0 => {}
            Event::Start(Tag::Item) | Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::BlockQuote(_)) => {
                self.of_break();
            }
            Event::Text(text) => self.current.push_str(&text),
            Event::Code(code) => self.current.push_str(&Self::of_code(&code).unwrap_or_default()),
            Event::SoftBreak | Event::HardBreak => self.current.push(' '),
            _ => {}
        }
        self
    }

    const fn of_hidden(tag: &Tag<'_>) -> bool {
        matches!(
            tag,
            Tag::CodeBlock(_)
                | Tag::Table(_)
                | Tag::HtmlBlock
                | Tag::Heading { .. }
                | Tag::Image { .. }
                | Tag::FootnoteDefinition(_)
                | Tag::MetadataBlock(_)
        )
    }

    const fn of_hidden_end(end: TagEnd) -> bool {
        matches!(
            end,
            TagEnd::CodeBlock
                | TagEnd::Table
                | TagEnd::HtmlBlock
                | TagEnd::Heading(_)
                | TagEnd::Image
                | TagEnd::FootnoteDefinition
                | TagEnd::MetadataBlock(_)
        )
    }

    /// Closes the block being gathered, keeping it when a word is left in it.
    fn of_break(&mut self) {
        let block = std::mem::take(&mut self.current);
        let pieces: Vec<Piece> = block.split_whitespace().filter_map(Self::of_piece).collect();
        let text = Self::of_join(&pieces);
        if !text.is_empty() {
            self.blocks.push(text);
        }
    }

    /// Inline code: a word is read (`snake_case` as two words), a path or a file name as its file name, the
    /// rest not at all.
    fn of_code(code: &str) -> Option<String> {
        let word = code.chars().count() <= WORD && code.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
        let path = code.contains(['/', '\\']) || Self::of_extension(code);
        match (word, path) {
            (true, _) => Some(code.replace('_', " ")),
            (false, true) => Self::of_last(code),
            (false, false) => None,
        }
    }

    /// The last component of a path, without the separators around it.
    fn of_last(path: &str) -> Option<String> {
        path.trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|last| !last.is_empty())
            .map(str::to_owned)
    }

    /// One token of prose as it is said. A URL (`://` or `www.` anywhere in it, a word glued to it included)
    /// is not said, though its sentence mark is kept; a path becomes its file name; `_` separates words.
    /// Then only letters, digits and [`SPEAKABLE`] punctuation are kept, and a token left without a letter
    /// or digit is no word: at most the sentence marks it carried.
    fn of_piece(token: &str) -> Option<Piece> {
        let lower = token.to_lowercase();
        let url = lower.contains("://") || lower.contains("www.");
        let core = token.trim_start_matches(OPENERS).trim_end_matches(CLOSERS);
        let lead = token.len().saturating_sub(token.trim_start_matches(OPENERS).len());
        let (opening, rest) = token.split_at(lead);
        let closing = rest.get(core.len()..).unwrap_or_default();
        let rooted = ROOTS.iter().any(|root| core.starts_with(*root));
        let path = !url && core.contains(['/', '\\']) && (rooted || Self::of_extension(core));
        let shown = match (url, path) {
            (true, _) => closing.to_owned(),
            (false, true) => format!("{opening}{}{closing}", Self::of_last(core).unwrap_or_default()),
            (false, false) => token.to_owned(),
        };
        let kept: String = shown
            .replace('_', " ")
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == ' ' || SPEAKABLE.contains(*c))
            .collect();
        let word = kept.split_whitespace().collect::<Vec<_>>().join(" ");
        let marks: String = word.chars().filter(|c| ENDS.contains(c)).collect();
        match (word.chars().any(char::is_alphanumeric), marks.is_empty()) {
            (true, _) => Some(Piece::Word(word)),
            (false, false) => Some(Piece::Marks(marks)),
            (false, true) => None,
        }
    }

    /// Whether `name` ends in a file extension: a dot, then one to five letters or digits.
    fn of_extension(name: &str) -> bool {
        let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
        (1..=5).contains(&ext.len()) && ext.chars().all(char::is_alphanumeric)
    }

    /// Pieces back into text. Sentence marks without a word close the sentence of the word before them, if
    /// it is still open, and are dropped otherwise: they may end a sentence, never stand as one.
    fn of_join(pieces: &[Piece]) -> String {
        pieces.iter().fold(String::new(), |mut text, piece| {
            match piece {
                Piece::Word(word) => {
                    text.push_str(if text.is_empty() { "" } else { " " });
                    text.push_str(word);
                }
                Piece::Marks(marks) if !text.is_empty() && !text.ends_with(ENDS) => text.push_str(marks),
                Piece::Marks(_) => {}
            }
            text
        })
    }

    /// Every block's sentences, in order: a block boundary always ends one.
    fn of_sentences(&self) -> impl Iterator<Item = String> + '_ {
        self.blocks.iter().flat_map(|block| {
            let words: Vec<&str> = block.split(' ').collect();
            words
                .split_inclusive(|word| Self::ends(word))
                .map(|sentence| sentence.join(" "))
                .collect::<Vec<_>>()
        })
    }

    /// At most [`CHARS`] characters, cut back to the last whole word and marked as cut.
    fn of_cap(text: String) -> String {
        let cut: String = text.chars().take(CHARS).collect();
        let kept = cut.rsplit_once(' ').map_or(cut.as_str(), |(kept, _)| kept);
        let ending = kept.trim_end_matches([',', ';', ':']);
        let truncated = cut.len() < text.len();
        if truncated { format!("{ending}…") } else { text }
    }

    /// The language whose common words are the most frequent in `text`; a tie, or none at all, is `fallback`.
    fn of_lang(text: &str, fallback: Lang) -> Lang {
        let words: Vec<String> = text
            .split_whitespace()
            .map(str::to_lowercase)
            .map(|word| word.trim_matches(Self::of_unlettered).to_owned())
            .collect();
        let scores = Lang::ALL.map(|lang| {
            let bank: Vec<&str> = Self::of_stopwords(lang).split(' ').collect();
            (words.iter().filter(|word| bank.contains(&word.as_str())).count(), lang)
        });
        let top = scores.iter().map(|(score, _)| *score).max().unwrap_or_default();
        let mut leaders = scores.iter().filter(|(score, _)| *score == top && top > 0);
        match (leaders.next(), leaders.next()) {
            (Some((_, lang)), None) => *lang,
            _ => fallback,
        }
    }

    /// Whether `c` is not part of a word as the banks spell it: letters, digits and the apostrophe are.
    fn of_unlettered(c: char) -> bool {
        !c.is_alphanumeric() && c != '\''
    }

    /// Words common in `lang` and absent from the other, one space apart: the banks are disjoint, so a word
    /// counts for one language only.
    const fn of_stopwords(lang: Lang) -> &'static str {
        match lang {
            Lang::Es => {
                "de la que el en y los se del las un por con una su para es al lo como más pero sus le ya este sí \
                 porque esta entre cuando muy sin sobre también me hasta hay donde desde todo nos esto qué eso está \
                 están ha he ahora aquí tu te"
            }
            Lang::En => {
                "the of and to in is it you that was for on are with as i they be at have this from or had by but \
                 what we can all there when your how which do their if will about then would so been now its not \
                 could into should just here it's i've i'm"
            }
        }
    }

    /// Abbreviations whose dot does not end a sentence, lowercased, one space apart.
    const fn of_abbreviations(lang: Lang) -> &'static str {
        match lang {
            Lang::Es => "p. ej. etc. sr. sra. dr. dra. aprox. pág. núm. ud. uds.",
            Lang::En => "e.g. i.e. etc. vs. mr. mrs. ms. dr. approx. fig.",
        }
    }

    /// The markdown parsed into its readable blocks, the last one closed.
    fn speakable_parse(markdown: &str) -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS;
        let events = Parser::new_ext(markdown, options);
        let mut digest = events.fold(Self::default(), Self::of_event);
        digest.of_break();
        digest
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// The digest of `markdown`, with the language it is in (`fallback` when its words do not decide);
    /// `None` when nothing in it reads aloud.
    #[must_use]
    pub fn of(markdown: &str, fallback: Lang) -> Option<(String, Lang)> {
        let text = Self::speakable(markdown)
            .into_iter()
            .take(SENTENCES)
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then(|| {
            let text = Self::of_cap(text);
            let lang = Self::of_lang(&text, fallback);
            (text, lang)
        })
    }

    /// All of `markdown` that reads aloud, sentence by sentence, with no cut: a block or a list item always
    /// ends a sentence. [`SpeechDigest::of`] is its first [`SENTENCES`], so the rule lives here once.
    #[must_use]
    pub fn speakable(markdown: &str) -> Vec<String> {
        Self::speakable_parse(markdown).of_sentences().collect()
    }

    /// Whether `word` closes a sentence: it ends with a sentence mark, and is not an abbreviation.
    #[must_use]
    pub fn ends(word: &str) -> bool {
        let core = word.trim_end_matches(['"', '\'', ')', ']', '»', '”']);
        let lower = core.trim_start_matches(OPENERS).to_lowercase();
        let abbreviation = Lang::ALL
            .iter()
            .any(|lang| Self::of_abbreviations(*lang).split(' ').any(|known| known == lower));
        core.ends_with(ENDS) && !abbreviation
    }
}
