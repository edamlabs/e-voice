use std::path::{Component, Path, PathBuf};

use e_voice_core::schema::lang::Lang;
use serde_json::Value;
use url::Url;

/// The longest command, query, path or name that can be approved by voice: read aloud whole, it is still
/// something a listener can follow.
const RAW: usize = 160;

/// The longest question asked: a faithful reading of a symbol-heavy text grows, and past this it is no
/// longer heard.
const SPOKEN: usize = 400;

/// The ASCII punctuation a reading names, in this order; [`ToolQuestion::of_names`] names each in a
/// language. Any other character that is neither a letter, a digit nor a single space cannot be read.
const SYMBOLS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

/// Why a request is refused without asking, word for word: the agent reads it and can act on it.
pub const LONG: &str = "too long or on several lines to be approved by voice; use a shorter request";
pub const UNREAD: &str = "it cannot be read aloud faithfully (an odd space, a character with no name, or a word \
                          a reading would take for a symbol); use a plainer request";
pub const SPOKEN_LONG: &str = "too long to be read aloud whole; use a shorter request";
pub const OUTSIDE: &str = "the file is outside the project, or not named";
pub const EMPTY: &str = "the request carries nothing to read aloud";

/// What an approval asks: a question to say aloud, or a refusal without asking, with its reason for the
/// agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Ask(String),
    Refuse(String),
}

/// What a permission request is about, by the agent's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Run,
    Edit,
    Write,
    Open,
    Search,
    Use,
}

/// How a character groups with its neighbours in a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Digit,
    Capital,
    Letter,
    Symbol,
}

/// A run of a word as it is read: digits as they are; letters lowercase or caseless as they are, one
/// capital then lowercase behind one marker, two capitals or more behind the other; a symbol by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Run {
    Plain(String),
    Title(String),
    Caps(String),
    Symbol(char),
}

/// The spoken question for one permission request. Only what is read aloud whole and faithfully can be
/// approved by voice, and faithful means reconstructible: [`ToolQuestion::rebuild`] gives the exact text
/// back from what is said, or the request is refused. Words of the text are read one by one, a comma's
/// pause between them; inside a word every symbol is named and capitals are announced; nothing is shortened
/// or normalised. Too long, several lines, two spaces, a tab, a character with no name, or a word a reading
/// would take for a symbol or a marker: refused without asking — a bare yes must never approve something
/// the listener did not hear.
///
/// A file an edit would change is resolved as the tool resolves it — from the session's directory, `..` and
/// symlinks followed through the deepest part that exists — and refused unasked outside the project. A
/// command's own paths are not checked: for a command, the protection is that it is heard whole.
#[derive(Debug)]
pub struct ToolQuestion;

impl ToolQuestion {
    // ##### PRIVATE #####

    /// The words around what is asked about, in `lang`.
    const fn of_frame(kind: ToolKind, lang: Lang) -> (&'static str, &'static str) {
        match (kind, lang) {
            (ToolKind::Run, Lang::Es) => ("¿Ejecuto: ", "?"),
            (ToolKind::Run, Lang::En) => ("Shall I run: ", "?"),
            (ToolKind::Edit, Lang::Es) => ("¿Edito ", "?"),
            (ToolKind::Edit, Lang::En) => ("Shall I edit ", "?"),
            (ToolKind::Write, Lang::Es) => ("¿Escribo ", "?"),
            (ToolKind::Write, Lang::En) => ("Shall I write ", "?"),
            (ToolKind::Open, Lang::Es) => ("¿Abro ", "?"),
            (ToolKind::Open, Lang::En) => ("Shall I open ", "?"),
            (ToolKind::Search, Lang::Es) => ("¿Busco en la web: ", "?"),
            (ToolKind::Search, Lang::En) => ("Shall I search the web for: ", "?"),
            (ToolKind::Use, Lang::Es) => ("¿Uso ", "?"),
            (ToolKind::Use, Lang::En) => ("Shall I use ", "?"),
        }
    }

    /// The markers before a capitalised run, in `lang`: one capital, then two or more.
    const fn of_markers(lang: Lang) -> (&'static str, &'static str) {
        match lang {
            Lang::Es => ("mayúscula", "mayúsculas"),
            Lang::En => ("capital", "capitals"),
        }
    }

    /// The names of [`SYMBOLS`], in their order, in `lang`.
    const fn of_names(lang: Lang) -> [&'static str; 32] {
        match lang {
            Lang::Es => [
                "exclamación",
                "comillas",
                "almohadilla",
                "dólar",
                "por ciento",
                "ampersand",
                "apóstrofo",
                "abre paréntesis",
                "cierra paréntesis",
                "asterisco",
                "más",
                "coma",
                "guion",
                "punto",
                "barra",
                "dos puntos",
                "punto y coma",
                "menor que",
                "igual",
                "mayor que",
                "interrogación",
                "arroba",
                "abre corchete",
                "barra invertida",
                "cierra corchete",
                "circunflejo",
                "guion bajo",
                "acento grave",
                "abre llave",
                "barra vertical",
                "cierra llave",
                "virgulilla",
            ],
            Lang::En => [
                "exclamation",
                "quote",
                "hash",
                "dollar",
                "percent",
                "ampersand",
                "apostrophe",
                "open paren",
                "close paren",
                "star",
                "plus",
                "comma",
                "dash",
                "dot",
                "slash",
                "colon",
                "semicolon",
                "less than",
                "equals",
                "greater than",
                "question mark",
                "at",
                "open bracket",
                "backslash",
                "close bracket",
                "caret",
                "underscore",
                "backtick",
                "open brace",
                "pipe",
                "close brace",
                "tilde",
            ],
        }
    }

    /// The class of a character; `None` for one no reading can render.
    fn of_class(c: char) -> Option<Class> {
        match c {
            digit if digit.is_numeric() => Some(Class::Digit),
            capital if capital.is_uppercase() => Some(Class::Capital),
            letter if letter.is_alphabetic() => Some(Class::Letter),
            symbol if SYMBOLS.contains(symbol) => Some(Class::Symbol),
            _ => None,
        }
    }

    /// One word cut into its runs: digits, letters by case (a single capital takes the lowercase run after
    /// it), each symbol alone. `None` when a character cannot be read.
    fn of_runs(word: &str) -> Option<Vec<Run>> {
        let classes: Option<Vec<(char, Class)>> = word.chars().map(|c| Some((c, Self::of_class(c)?))).collect();
        let classes = classes?;
        let groups = classes.chunk_by(|one, other| one.1 == other.1 && one.1 != Class::Symbol);
        let runs = groups.fold(Vec::new(), |mut runs: Vec<Run>, group| {
            let text: String = group.iter().map(|(c, _)| *c).collect();
            let class = group.first().map_or(Class::Symbol, |first| first.1);
            let titled = matches!(runs.last(), Some(Run::Title(open)) if open.chars().count() == 1);
            match (class, runs.last_mut()) {
                (Class::Letter, Some(Run::Title(open))) if titled => open.push_str(&text),
                (Class::Symbol, _) => runs.extend(text.chars().next().map(Run::Symbol)),
                (Class::Capital, _) if text.chars().count() == 1 => runs.push(Run::Title(text)),
                (Class::Capital, _) => runs.push(Run::Caps(text)),
                (Class::Digit | Class::Letter, _) => runs.push(Run::Plain(text)),
            }
            runs
        });
        Some(runs)
    }

    /// One run as it is said in `lang`.
    fn of_say(run: &Run, lang: Lang) -> String {
        let (title, caps) = Self::of_markers(lang);
        let names = Self::of_names(lang);
        match run {
            Run::Plain(text) => text.clone(),
            Run::Title(text) => format!("{title} {}", text.to_lowercase()),
            Run::Caps(text) => format!("{caps} {}", text.to_lowercase()),
            Run::Symbol(symbol) => SYMBOLS
                .find(*symbol)
                .and_then(|at| names.get(at))
                .map(|name| (*name).to_owned())
                .unwrap_or_default(),
        }
    }

    /// `text` read aloud: words a comma's pause apart, each read run by run. `None` when it cannot be read
    /// faithfully: a space that is not single, a character with no name, or a reading that does not give
    /// the text back.
    fn of_spoken(text: &str, lang: Lang) -> Option<String> {
        let words: Vec<&str> = text.split(' ').collect();
        if words.iter().any(|word| word.is_empty()) {
            return None;
        }
        let spoken: Option<Vec<String>> = words
            .iter()
            .map(|word| {
                let runs = Self::of_runs(word)?;
                let said: Vec<String> = runs.iter().map(|run| Self::of_say(run, lang)).collect();
                Some(said.join(" "))
            })
            .collect();
        let spoken = spoken?.join(", ");
        (Self::rebuild(&spoken, lang).as_deref() == Some(text)).then_some(spoken)
    }

    /// The path an edit would change, relative to the project, resolved as the tool resolves it: from the
    /// session's directory, `..` and symlinks followed through the deepest part that exists. `None` outside
    /// the project, for the project itself, or for no path at all. Reads the filesystem: run it off the loop.
    fn of_path(raw: &str, cwd: &Path) -> Option<String> {
        let root = std::fs::canonicalize(cwd).ok()?;
        let full = cwd.join(raw);
        let (real, rest) = full
            .ancestors()
            .find_map(|known| Some((std::fs::canonicalize(known).ok()?, full.strip_prefix(known).ok()?)))?;
        let plain = rest.components().all(|part| matches!(part, Component::Normal(_)));
        let resolved: PathBuf = plain.then(|| real.join(rest))?;
        let relative = resolved.strip_prefix(&root).ok()?.to_string_lossy().into_owned();
        (!raw.is_empty() && !relative.is_empty()).then_some(relative)
    }

    /// What the request is about, before it is read: the command, the path, the host, the query or the tool,
    /// or why it cannot be asked about.
    async fn of_subject(tool: &str, input: &Value, cwd: &Path) -> Result<(ToolKind, String), String> {
        let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
        let found =
            |kind: ToolKind, text: Option<String>| text.map(|text| (kind, text)).ok_or_else(|| EMPTY.to_owned());
        let host = |url: String| Url::parse(&url).ok()?.host_str().map(str::to_owned);
        let kind = match tool {
            "Bash" | "PowerShell" | "Monitor" => return found(ToolKind::Run, field("command")),
            "WebFetch" => return found(ToolKind::Open, field("url").and_then(host)),
            "WebSearch" => return found(ToolKind::Search, field("query")),
            "Edit" | "NotebookEdit" => ToolKind::Edit,
            "Write" => ToolKind::Write,
            name => return Ok((ToolKind::Use, name.replace("__", " "))),
        };
        let raw = field("file_path")
            .or_else(|| field("notebook_path"))
            .unwrap_or_default();
        let base = cwd.to_path_buf();
        let resolved = tokio::task::spawn_blocking(move || Self::of_path(&raw, &base)).await;
        resolved
            .ok()
            .flatten()
            .map(|path| (kind, path))
            .ok_or_else(|| OUTSIDE.to_owned())
    }

    /// A titled run: its first letter capital, the rest as said.
    fn rebuild_title(part: &str) -> String {
        let mut chars = part.chars();
        chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default()
    }

    /// One spoken word rebuilt: a marker capitalises the part after it, the longest symbol name that matches
    /// at a part is that symbol, anything else is taken as it is. `None` for a marker with nothing after it.
    fn rebuild_word(word: &str, lang: Lang) -> Option<String> {
        let (title, caps) = Self::of_markers(lang);
        let names = Self::of_names(lang);
        let parts: Vec<&str> = word.split(' ').collect();
        let mut text = String::new();
        let mut at = 0usize;
        while let Some(part) = parts.get(at) {
            let symbol = names
                .iter()
                .zip(SYMBOLS.chars())
                .map(|(name, symbol)| (name.split(' ').collect::<Vec<_>>(), symbol))
                .filter(|(name, _)| parts.get(at..at.saturating_add(name.len())) == Some(name.as_slice()))
                .max_by_key(|(name, _)| name.len());
            let next = parts.get(at.saturating_add(1));
            let (piece, used) = match (*part, symbol, next) {
                (marker, _, Some(after)) if marker == title => (Self::rebuild_title(after), 2),
                (marker, _, Some(after)) if marker == caps => (after.to_uppercase(), 2),
                (marker, _, None) if marker == title || marker == caps => return None,
                (_, Some((name, symbol)), _) => (symbol.to_string(), name.len()),
                (plain, None, _) => (plain.to_owned(), 1),
            };
            text.push_str(&piece);
            at = at.saturating_add(used);
        }
        Some(text)
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// The question for the agent's request to use `tool` with `input`, in `lang`, in the project at `cwd`
    /// (the session's directory); or why it is refused without asking.
    pub async fn of(tool: &str, input: &Value, lang: Lang, cwd: &Path) -> Asked {
        let subject = Self::of_subject(tool, input, cwd).await.and_then(|(kind, subject)| {
            let oversized = subject.chars().count() > RAW || subject.contains(['\n', '\r']);
            (!oversized).then_some((kind, subject)).ok_or_else(|| LONG.to_owned())
        });
        let question = subject.and_then(|(kind, subject)| {
            let spoken = Self::of_spoken(&subject, lang).ok_or_else(|| UNREAD.to_owned())?;
            let (before, after) = Self::of_frame(kind, lang);
            Ok(format!("{before}{spoken}{after}"))
        });
        match question {
            Ok(question) if question.chars().count() <= SPOKEN => Asked::Ask(question),
            Ok(_) => Asked::Refuse(SPOKEN_LONG.to_owned()),
            Err(reason) => Asked::Refuse(reason),
        }
    }

    /// The text a reading in `lang` says, rebuilt from it: words split at ", ", each word's parts at single
    /// spaces, rebuilt part by part and joined without spaces. `None` for a marker with nothing after it.
    #[must_use]
    pub fn rebuild(spoken: &str, lang: Lang) -> Option<String> {
        let words: Option<Vec<String>> = spoken.split(", ").map(|word| Self::rebuild_word(word, lang)).collect();
        words.map(|words| words.join(" "))
    }
}
