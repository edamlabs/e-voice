use crate::speech::digest::SpeechDigest;
use crate::workflow::queue::TEXT;

/// How a fenced code block opens and closes: a line starting with either, after at most [`INDENT`] spaces.
const FENCES: [&str; 2] = ["```", "~~~"];

/// The most spaces a line may be indented by and still start a block; four make indented code instead.
const INDENT: usize = 3;

/// Besides letters and digits, what a line of plain prose may start with: an opening mark or quote.
const OPENING: &str = "¿¡\"'«“‘";

/// A block of the reply: between blank lines, outside fences.
#[derive(Debug, Default)]
struct Block {
    /// Its text so far, line by line, the lines already said included: a block held later is read whole.
    text: String,
    /// The words already said of it while it streamed.
    said: Vec<String>,
    /// Whether it is held: said whole, less what was already said, when it closes.
    held: bool,
    /// Whether a blank line came while it was held: an indented line after it goes on with the block (a
    /// list item's continuation), anything else closes it.
    gap: bool,
}

/// What the start of a line proves about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Not enough of the line has arrived to tell.
    Unknown,
    Blank,
    Fence,
    Prose,
    Held,
}

/// An agent's streamed reply cut into what to say, as early as the markdown allows. While a block's lines
/// start as plain prose (a letter, a digit that is not a list number, ¿ ¡ or an opening quote) it is said
/// sentence by sentence as it arrives: a sentence closes at a sentence mark followed by whitespace (an
/// abbreviation's dot excepted) or at the end of its line, and says what [`SpeechDigest::speakable`] keeps
/// of it. From the first line that starts otherwise, the block is held until it closes; then the whole
/// block, the lines already said included, goes through `speakable`, and what it reads beyond the words
/// already said is said. A heading, a table or HTML is never said, and a line that only looks like a list
/// or a tag inside prose is not lost. Fenced code is never said. A piece longer than the speech queue takes
/// ([`TEXT`] characters) is split between words.
///
/// A streaming chunker cannot unsay, so three things told only later are read where the whole reply would
/// not: the line above a setext underline (`===`, `---`), a table whose header row has no leading `|`, and
/// the first line of an inline construct (a code span, a link, emphasis) that crosses a line break. Every
/// sentence said still passes the allow-list.
#[derive(Debug, Default)]
pub struct SentenceChunker {
    /// The line still arriving: what of it has not been said or kept yet.
    line: String,
    /// Whether the line still arriving is known to be prose, so its start is no longer read.
    prose: bool,
    /// Whether the reply is inside a fenced code block.
    fenced: bool,
    block: Block,
}

impl SentenceChunker {
    // ##### PRIVATE #####

    /// A line whose newline has arrived.
    fn push_close(&mut self, line: &str) -> Vec<String> {
        let shape = self.push_shape(line, true);
        self.prose = false;
        if self.fenced {
            self.fenced = shape != Shape::Fence;
            return Vec::new();
        }
        let mut said = self.push_gap(line);
        match shape {
            Shape::Blank if self.block.held => {
                self.block.gap = true;
                self.block.text.push('\n');
            }
            Shape::Blank => self.block = Block::default(),
            Shape::Fence => {
                said.extend(self.push_end());
                self.fenced = true;
            }
            Shape::Prose if !self.block.held => {
                let (sentences, _) = Self::push_split(line, true);
                self.block.text.push_str(line);
                self.block.text.push('\n');
                said.extend(self.push_say(&sentences));
            }
            Shape::Prose | Shape::Held | Shape::Unknown => self.push_hold(line),
        }
        said
    }

    /// The line still arriving: when its start proves it prose in a block that is not held, the sentences
    /// it already completed are said; otherwise it waits for its newline.
    fn push_open(&mut self) -> Vec<String> {
        if self.fenced {
            return Vec::new();
        }
        let line = self.line.clone();
        let mut said = self.push_gap(&line);
        let shape = self.push_shape(&self.line, false);
        if shape != Shape::Prose || self.block.held {
            return said;
        }
        self.prose = true;
        let (sentences, rest) = Self::push_split(&self.line, false);
        let streamed = sentences.iter().map(|sentence| format!("{sentence} "));
        self.block.text.extend(streamed);
        self.line = rest;
        said.extend(self.push_say(&sentences));
        said
    }

    /// A held block after a blank line closes when a line starts without indent; an indented one goes on
    /// with it, and a blank one keeps waiting.
    fn push_gap(&mut self, line: &str) -> Vec<String> {
        let unindented = line.chars().next().is_some_and(|first| !first.is_whitespace());
        let closes = self.block.gap && unindented;
        if closes { self.push_end() } else { Vec::new() }
    }

    /// What the start of `line` proves, `complete` when its newline has arrived. The rest of a line already
    /// known as prose stays prose, whatever it starts with now; indented code can only open a block, so an
    /// indented line inside a block is not code.
    fn push_shape(&self, line: &str, complete: bool) -> Shape {
        let start = line.trim_start_matches(' ');
        let indent = line.len().saturating_sub(start.len());
        let fence = indent <= INDENT && FENCES.iter().any(|fence| start.starts_with(*fence));
        let near = !complete && indent <= INDENT && FENCES.iter().any(|fence| fence.starts_with(start));
        let opening = indent > INDENT && self.block.text.is_empty();
        match (start.chars().next(), fence, near) {
            _ if self.prose => Shape::Prose,
            (_, true, _) => Shape::Fence,
            (_, _, true) => Shape::Unknown,
            (None, ..) => Shape::Blank,
            _ if opening => Shape::Held,
            (Some(first), ..) => Self::push_lead(start, first, complete),
        }
    }

    /// What a line's first character proves.
    fn push_lead(start: &str, first: char, complete: bool) -> Shape {
        match first {
            letter if letter.is_alphabetic() || OPENING.contains(letter) => Shape::Prose,
            digit if digit.is_ascii_digit() => Self::push_number(start, complete),
            _ => Shape::Held,
        }
    }

    /// A line starting with a digit is prose unless the digits are a list number: followed by `.` or `)`
    /// and a space, or nothing more.
    fn push_number(start: &str, complete: bool) -> Shape {
        let mut after = start.trim_start_matches(|c: char| c.is_ascii_digit()).chars();
        match (after.next(), after.next()) {
            (None, _) | (Some('.' | ')'), None) if !complete => Shape::Unknown,
            (Some('.' | ')'), None | Some(' ' | '\t')) => Shape::Held,
            _ => Shape::Prose,
        }
    }

    /// A line the block is held for, kept with the rest of the block.
    fn push_hold(&mut self, line: &str) {
        self.block.held = true;
        self.block.gap = false;
        self.block.text.push_str(line);
        self.block.text.push('\n');
    }

    /// The block closes. A held block is read whole by `speakable`, and what it reads beyond the words
    /// already said of it is said.
    fn push_end(&mut self) -> Vec<String> {
        let block = std::mem::take(&mut self.block);
        if !block.held {
            return Vec::new();
        }
        let whole = SpeechDigest::speakable(&block.text);
        let same = whole
            .iter()
            .flat_map(|sentence| sentence.split(' '))
            .zip(&block.said)
            .take_while(|(word, said)| Self::push_same(word, said))
            .count();
        Self::push_skip(whole, same)
            .iter()
            .flat_map(|sentence| Self::push_pack(sentence))
            .collect()
    }

    /// `sentences` without their first `count` words: a sentence left with none is gone, one cut into keeps
    /// its last words as a sentence of its own.
    fn push_skip(sentences: Vec<String>, count: usize) -> Vec<String> {
        let mut left = count;
        sentences
            .into_iter()
            .filter_map(|sentence| {
                let words: Vec<&str> = sentence.split(' ').collect();
                let skipped = left.min(words.len());
                left = left.saturating_sub(skipped);
                let kept = words.get(skipped..).unwrap_or_default().join(" ");
                (!kept.is_empty()).then_some(kept)
            })
            .collect()
    }

    /// Whether two said words are the same word: by their letters and digits, whatever the case.
    fn push_same(one: &str, other: &str) -> bool {
        let bare = |word: &str| -> String {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect()
        };
        bare(one) == bare(other)
    }

    /// The complete sentences of `text`, and what is left of it still open. In a line still arriving the
    /// last word may not be whole yet, so only words followed by whitespace can close a sentence. The text
    /// is cut, never re-joined: whitespace inside a sentence (a tab in a code span) reaches the parser as it
    /// came.
    fn push_split(text: &str, complete: bool) -> (Vec<String>, String) {
        let cut = text.trim_end_matches(|c: char| !c.is_whitespace()).len();
        let (body, tail) = if complete { (text, "") } else { text.split_at(cut) };
        let pieces: Vec<&str> = body.split_inclusive(char::is_whitespace).collect();
        let ends = |piece: &&str| SpeechDigest::ends(piece.trim_end());
        let mut groups: Vec<&[&str]> = pieces.split_inclusive(ends).collect();
        let closed = groups
            .last()
            .and_then(|last| last.iter().rev().find(|piece| !piece.trim().is_empty()))
            .is_none_or(ends);
        let open = (!complete && !closed).then(|| groups.pop()).flatten();
        let rest = open.map_or_else(|| tail.to_owned(), |open| format!("{}{tail}", open.concat()));
        let sentences = groups.iter().map(|group| group.concat().trim().to_owned());
        (sentences.filter(|sentence| !sentence.is_empty()).collect(), rest)
    }

    /// What streamed sentences say, each through `speakable`, in pieces the speech queue takes; the words
    /// said are kept with the block.
    fn push_say(&mut self, sentences: &[String]) -> Vec<String> {
        let pieces: Vec<String> = sentences
            .iter()
            .flat_map(|sentence| SpeechDigest::speakable(sentence))
            .flat_map(|said| Self::push_pack(&said))
            .collect();
        let words = pieces.iter().flat_map(|piece| piece.split(' ')).map(str::to_owned);
        self.block.said.extend(words);
        pieces
    }

    /// The words of `text` packed into pieces of at most [`TEXT`] characters; none when it has no word.
    fn push_pack(text: &str) -> Vec<String> {
        let words = text.split_whitespace();
        words.fold(Vec::new(), |mut pieces: Vec<String>, word| {
            let word: String = word.chars().take(TEXT).collect();
            let used = pieces.last().map_or(0, |piece| piece.chars().count());
            let room = TEXT.saturating_sub(used);
            match pieces.last_mut() {
                Some(piece) if room > word.chars().count() => {
                    piece.push(' ');
                    piece.push_str(&word);
                }
                _ => pieces.push(word),
            }
            pieces
        })
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Takes the next part of the reply and returns what it lets be said, in order.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.line.push_str(delta);
        let mut said = Vec::new();
        while let Some(at) = self.line.find('\n') {
            let line: String = self.line.drain(..=at).collect();
            said.extend(self.push_close(line.trim_end_matches(['\n', '\r'])));
        }
        said.extend(self.push_open());
        said
    }

    /// The reply has ended: what is still open or held is said, unless it is inside a fence that never
    /// closed. The chunker is then as new.
    pub fn finish(&mut self) -> Vec<String> {
        let mut said = self.push("\n");
        said.extend(self.push_end());
        *self = Self::default();
        said
    }
}
