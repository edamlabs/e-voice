use std::panic::{AssertUnwindSafe, catch_unwind};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::speech::digest::{CHARS, SENTENCES, SPEAKABLE, SpeechDigest};

/// How many seeded inputs a run tries, besides the regressions.
const RUNS: u64 = 4_000;

/// Inputs that once broke a property, kept for good: the literal text, so a change of the generator cannot
/// lose them.
const REGRESSIONS: &[&str] = &[
    "` kebab-case\u{a0}",
    "./y ```\n\r\n",
    "café ` ",
    "| x |",
    "\0ñandú\r\n",
    "\u{7f}Dr. a\n\n",
    "I'm\u{2003}# ",
    "https://a.b/c\t###### \na ",
    "parahttps://a.b/c\n\n",
    "e-mailwww.z.com/p",
    "日本語\r\n_  ",
    "queue\n{} `max_size`\n\n",
    "\\\u{2003}e-mail\r\n~~~ ",
    "año ¡!\u{a0}ftp://f.g/h  \u{200b}\n\n",
];

/// Seeds that once broke a property.
const SEEDS: &[u64] = &[0, 1, 2, 8, 9, 69, 86, 231, 397, 1196, 2221];

const WORDS: &[&str] = &[
    "hola",
    "queue",
    "the",
    "cola",
    "ñandú",
    "canción",
    "café",
    "is",
    "lista",
    "done",
    "Hecho",
    "fixed",
    "tests",
    "para",
    "que",
    "and",
    "con",
    "it's",
    "I'm",
    "x",
    "a",
    "3.5",
    "v3.11.0",
    "4",
    "100%",
    "año",
    "über",
    "日本語",
    "supercalifragilisticoespialidosocalifragilisticoespialidoso",
    "snake_case",
    "kebab-case",
    "e-mail",
    "U.S.A",
];

const MARKS: &[&str] = &[
    ".", "...", "…", "—", "–", "¿?", "¡!", "?", "!", "()", "[]", "{}", "«»", "%", "@@@", "$$$", "***", "~~~", ",", ":",
    ";", "-", "--", "_", "__", "&", "=", "+", "/", "\\", "^", "'", "\"", "“”", "|", "#", "##", "###", ">", ">>", "<",
    "*",
];

const CODE: &[&str] = &[
    "`",
    "``",
    "```",
    "```rust\n",
    "```\n",
    "\n```\n",
    "`queue.rs`",
    "`foo()`",
    "`max_size`",
    "`src/a/b.rs`",
    "`a b`",
    "`` ` ``",
    "~~~\n",
    "    indented code",
    "\t\ttabbed code",
];

const BLOCKS: &[&str] = &[
    "# ",
    "## ",
    "###### ",
    "- ",
    "* ",
    "+ ",
    "1. ",
    "2) ",
    "> ",
    ">> ",
    "- [ ] ",
    "- [x] ",
    "---",
    "***",
    "___",
    "| a | b |\n|---|---|\n| 1 | 2 |",
    "| x |",
    "|---|",
    "[^1]",
    "[^1]: nota al pie",
    "<div>",
    "</p>",
    "<br/>",
    "<!-- c -->",
    "![alt](http://x.y/i.png)",
    "![](u)",
    "Term\n: definition",
    "---\ntitle: x\n---\n",
    "+++\n",
];

const LINKS: &[&str] = &[
    "https://a.b/c",
    "http://x.y",
    "www.z.com/p",
    "HTTPS://UP.PER/CASE",
    "[t](https://u.v/w)",
    "<https://a.b>",
    "[ref]: http://r.s",
    "https://a.b/c.",
    "(https://a.b/c)",
    "https://a.b/c,",
    "ftp://f.g/h",
    "mailto:a@b.c",
    "src/a/b.rs",
    "~/x",
    "./y",
    "../z",
    "/etc/passwd",
    "C:\\x\\y.txt",
    "a/b",
    "file.txt",
    "Cargo.toml.",
];

const SYMBOLS: &[&str] = &[
    "🎉",
    "✅",
    "→",
    "✓",
    "❤️",
    "👨‍👩‍👧",
    "🏳️‍🌈",
    "\u{200d}",
    "\u{200b}",
    "\u{feff}",
    "\u{0}",
    "\u{7f}",
    "•",
    "◦",
    "★",
    "©",
    "&amp;",
    "&#x1F600;",
    "&nbsp;",
    "e.g.",
    "etc.",
    "p. ej.",
    "Dr.",
    "i.e.",
];

const GAPS: &[&str] = &[
    " ", " ", " ", "  ", "\n", "\n\n", "\t", "", "\r\n", "\u{a0}", "\u{2003}",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap_or(1)).unwrap_or_default()
    }

    fn pick<'a>(&mut self, bank: &[&'a str]) -> &'a str {
        bank[self.below(bank.len())]
    }
}

/// The pieces of one input, so a failing one can be shrunk by dropping pieces.
fn pieces(seed: u64) -> Vec<String> {
    let mut rng = Rng(seed);
    let count = 1 + rng.below(40);
    (0..count)
        .map(|_| {
            let bank = match rng.below(12) {
                0..=4 => WORDS,
                5..=6 => MARKS,
                7 => CODE,
                8 => BLOCKS,
                9 => LINKS,
                _ => SYMBOLS,
            };
            let piece = rng.pick(bank);
            let gap = rng.pick(GAPS);
            let url = std::ptr::eq(bank, LINKS);
            if url {
                format!(" {piece}{}", if gap.is_empty() { " " } else { gap })
            } else {
                format!("{piece}{gap}")
            }
        })
        .collect()
}

fn fallback(seed: u64) -> Lang {
    if seed.is_multiple_of(2) { Lang::Es } else { Lang::En }
}

fn has_alphanumeric(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

fn terminators(text: &str) -> usize {
    let joined = text.replace("p. ej.", "p.ej.");
    let tokens: Vec<&str> = joined.split(' ').collect();
    tokens
        .iter()
        .enumerate()
        .filter(|(at, token)| {
            let ends = token.ends_with(['.', '!', '?']) || (token.ends_with('…') && *at + 1 < tokens.len());
            ends && !["e.g.", "i.e.", "etc.", "p.ej.", "dr."].contains(&token.to_lowercase().as_str())
        })
        .count()
}

/// No token without a letter or a digit stands alone between spaces, anywhere: a dash included.
fn alone_ok(tokens: &[&str]) -> bool {
    tokens.iter().all(|token| has_alphanumeric(token))
}

/// Every property that must hold of one digest, as the first one it breaks.
fn broken(input: &str, fallback: Lang) -> Option<String> {
    let Ok(result) = catch_unwind(AssertUnwindSafe(|| SpeechDigest::of(input, fallback))) else {
        return Some("panicked".to_owned());
    };
    let (text, lang) = result?;
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = text.split(' ').collect();
    let checks: [(&str, bool); 12] = [
        (
            "every character is a letter, a digit, a space or speakable punctuation",
            text.chars()
                .all(|c| c.is_alphanumeric() || c == ' ' || SPEAKABLE.contains(c)),
        ),
        ("has a letter or a digit", has_alphanumeric(&text)),
        ("at most the cap plus the ellipsis", text.chars().count() <= CHARS + 1),
        ("no backtick or pipe", !text.contains(['`', '|'])),
        (
            "no heading marker",
            !tokens.iter().any(|t| !t.is_empty() && t.chars().all(|c| c == '#')),
        ),
        (
            "no URL, glued to a word or not",
            !(lower.contains("://") || lower.contains("www.")),
        ),
        ("no token of only punctuation between spaces", alone_ok(&tokens)),
        ("no leading or trailing space", text == text.trim()),
        (
            "only single plain spaces",
            !text.contains("  ") && !text.chars().any(|c| c.is_whitespace() && c != ' '),
        ),
        ("no control character", !text.chars().any(char::is_control)),
        ("at most two sentence terminators", terminators(&text) <= SENTENCES),
        ("never empty", !text.is_empty()),
    ];
    if let Some((name, _)) = checks.iter().find(|(_, held)| !held) {
        return Some(format!("{name}: {text:?}"));
    }
    let cut = text.ends_with('…') && text.chars().count() > CHARS - 40;
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let rest = text.chars().skip(digits);
    let numbered = digits > 0
        && rest.clone().next().is_some_and(|c| c == '.' || c == ')')
        && rest.clone().nth(1).is_none_or(|c| c == ' ');
    if !cut && !numbered {
        let again = catch_unwind(AssertUnwindSafe(|| SpeechDigest::of(&text, lang)))
            .ok()
            .flatten();
        if again.as_ref().map(|(second, _)| second.as_str()) != Some(text.as_str()) {
            return Some(format!("not a fixed point: {text:?} -> {again:?}"));
        }
    }
    None
}

/// The fixed point is not asserted for an output that opens like a numbered list item ("2) 1 4", "1."): read as
/// markdown a second time, the marker is a list marker and is dropped. The output is what is spoken, never
/// digested again, so this is no flaw of the digest. It is also not asserted for an output cut at the cap.
/// The input with pieces dropped one at a time for as long as it still breaks a property.
fn shrunk(mut parts: Vec<String>, fallback: Lang, class: &str) -> String {
    let mut at = 0;
    while at < parts.len() {
        let mut fewer = parts.clone();
        fewer.remove(at);
        if broken(&fewer.concat(), fallback).is_some_and(|why| why.starts_with(class)) {
            parts = fewer;
        } else {
            at += 1;
        }
    }
    parts.concat()
}

fn report(failures: &[String]) -> String {
    format!(
        "{} input(s) break a property:\n{}",
        failures.len(),
        failures.join("\n---\n")
    )
}

#[test]
fn test_every_digest_of_generated_markdown_holds_the_invariants() {
    let mut classes: std::collections::BTreeMap<String, (usize, Vec<String>)> = std::collections::BTreeMap::new();
    for seed in (0..RUNS).chain(SEEDS.iter().copied()) {
        let lang = fallback(seed);
        let parts = pieces(seed);
        let Some(why) = broken(&parts.concat(), lang) else {
            continue;
        };
        let class = why.split(':').next().unwrap_or_default().to_owned();
        let entry = classes.entry(class.clone()).or_default();
        entry.0 += 1;
        if entry.1.len() < 4 {
            let small = shrunk(parts, lang, &class);
            let again = broken(&small, lang).unwrap_or_default();
            entry
                .1
                .push(format!("seed {seed} ({lang:?}) shrunk {small:?} -> {again}"));
        }
    }
    let lines: Vec<String> = classes
        .iter()
        .map(|(class, (count, examples))| format!("[{class}] {count} of {RUNS} inputs\n  {}", examples.join("\n  ")))
        .collect();
    assert!(classes.is_empty(), "{}", report(&lines));
}

#[test]
fn test_the_kept_regressions_still_hold() {
    let failures: Vec<String> = REGRESSIONS
        .iter()
        .flat_map(|input| [Lang::Es, Lang::En].map(|lang| (input, lang)))
        .filter_map(|(input, lang)| broken(input, lang).map(|why| format!("{input:?} ({lang:?}): {why}")))
        .collect();
    assert!(failures.is_empty(), "{}", report(&failures));
}

#[test]
fn test_the_generator_covers_what_it_claims_and_is_deterministic() {
    let all: Vec<String> = (0..RUNS).map(|seed| pieces(seed).concat()).collect();
    assert_eq!(all, (0..RUNS).map(|seed| pieces(seed).concat()).collect::<Vec<_>>());
    for needle in [
        "```",
        "http",
        "🎉",
        "ñandú",
        "the ",
        "| a |",
        "\n\n",
        "...",
        "`queue.rs`",
        "<div>",
        "[^1]",
    ] {
        assert!(
            all.iter().filter(|input| input.contains(needle)).count() > 50,
            "{needle} is rare"
        );
    }
    let digested = all
        .iter()
        .enumerate()
        .filter(|(seed, input)| SpeechDigest::of(input, fallback(*seed as u64)).is_some())
        .count();
    assert!(digested > 1_500, "{digested} of {RUNS} inputs gave something to say");
    assert!(
        digested < usize::try_from(RUNS).unwrap(),
        "every input gave something to say"
    );
}
