use std::panic::{AssertUnwindSafe, catch_unwind};

use e_voice_daemon::speech::digest::{SPEAKABLE, SpeechDigest};
use e_voice_daemon::talk::chunker::SentenceChunker;
use e_voice_daemon::workflow::queue::TEXT;

/// How many seeded replies a run tries, besides the kept regressions.
const RUNS: u64 = 3_000;

/// Replies that once broke a property, kept for good as literal text.
const REGRESSIONS: &[&str] = &[];

/// Seeds that once broke a property.
const SEEDS: &[u64] = &[];

const MARKER: &str = "FENCEDMARKER";

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
    "snake_case",
    "kebab-case",
    "e-mail",
    "Hola.",
    "Listo.",
    "¿Qué",
    "tal?",
    "¡Bien!",
    "fin.",
    "ok.",
    "e.g.",
    "etc.",
    "p. ej.",
    "Dr.",
];

const MARKS: &[&str] = &[
    ".", "...", "…", "—", "?", "!", "()", "%", ",", ":", ";", "_", "|", "<", "&",
];

const INLINE: &[&str] = &[
    "`queue.rs`",
    "`foo()`",
    "`max_size`",
    "[la doc](https://x.y/z)",
    "https://a.b/c",
    "www.z.com/p",
    "src/a/b.rs",
    "**fuerte**",
    "_énfasis_",
    "~~tachado~~",
    "<b>",
    "🎉",
    "✅",
    "\u{200b}",
    "\u{7f}",
    "&amp;",
];

const LINE_STARTS: &[&str] = &[
    "",
    "",
    "",
    "- ítem ",
    "* ítem ",
    "1. paso ",
    "2) paso ",
    "> cita ",
    "# Título ",
    "## Sección ",
    "    sangrado ",
    "  dos ",
    "| a | b |\n|---|---|\n| 1 | 2 |\n",
];

const FENCES: &[&str] = &["```", "~~~", "```rust", "``` python", "   ```", "~~~~"];

const GAPS: &[&str] = &[" ", " ", " ", "  ", "\n", "\n\n", "\r\n", "\t", ""];

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

/// One reply as pieces; a fenced block is one atomic piece, always whole and balanced, so a marker word
/// inside it is fenced content by construction. `closed` says whether the last fence is left open.
fn pieces(seed: u64) -> Vec<String> {
    let mut rng = Rng(seed);
    let count = 1 + rng.below(30);
    let mut parts: Vec<String> = (0..count)
        .map(|_| match rng.below(10) {
            0..=4 => format!("{}{}", rng.pick(WORDS), rng.pick(GAPS)),
            5 => format!("{}{}", rng.pick(MARKS), rng.pick(GAPS)),
            6 => format!("{}{}", rng.pick(INLINE), rng.pick(GAPS)),
            7 => format!("\n{}", rng.pick(LINE_STARTS)),
            _ => {
                let fence = rng.pick(FENCES);
                let close = if fence.contains('~') { "~~~" } else { "```" };
                format!("\n{fence}\n{MARKER} código;\nfn x() {{}}\n{close}\n")
            }
        })
        .collect();
    if rng.below(8) == 0 {
        parts.push(format!("\n```\n{MARKER} sin cerrar"));
    }
    parts
}

/// The deltas of `text` split at pseudo-random character boundaries; `mode` 0 whole, 1 a character each.
fn split(text: &str, seed: u64, mode: usize) -> Vec<String> {
    match mode {
        0 => vec![text.to_owned()],
        1 => text.chars().map(String::from).collect(),
        _ => {
            let mut rng = Rng(seed ^ 0xABCD);
            let chars: Vec<char> = text.chars().collect();
            let mut out = Vec::new();
            let mut at = 0;
            while at < chars.len() {
                let take = 1 + rng.below(12);
                out.push(chars[at..chars.len().min(at + take)].iter().collect());
                at += take;
            }
            out
        }
    }
}

struct Said {
    pushed: Vec<String>,
    finished: Vec<String>,
}

impl Said {
    fn all(&self) -> Vec<String> {
        self.pushed.iter().chain(&self.finished).cloned().collect()
    }
}

fn chunk(deltas: &[String]) -> Said {
    let mut chunker = SentenceChunker::default();
    let pushed = deltas.iter().flat_map(|delta| chunker.push(delta)).collect();
    Said {
        pushed,
        finished: chunker.finish(),
    }
}

/// The prose of `text`: its lines outside fenced blocks, by the fence rule the chunker states. A fence ends
/// the block before it, as in markdown, so it leaves a blank line where it was.
fn prose(text: &str) -> String {
    let mut fenced = false;
    let mut out = String::new();
    for line in text.split('\n') {
        let start = line.trim_start_matches(' ');
        let fence = line.len() - start.len() <= 3 && (start.starts_with("```") || start.starts_with("~~~"));
        if fence {
            fenced = !fenced;
            out.push('\n');
        } else if !fenced {
            out.push_str(line.trim_end_matches('\r'));
            out.push('\n');
        }
    }
    out
}

/// The words of `text` by their letters and digits alone: a mark that whole-text `speakable` glues to the
/// word before it from a line of its own (a line of only "…") is punctuation, not a word, and is not compared.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| word.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|word| !word.is_empty())
        .collect()
}

/// The first property one reply breaks, if any.
fn broken(text: &str, seed: u64) -> Option<String> {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let whole = chunk(&split(text, seed, 0));
        let said = whole.all();
        for piece in &said {
            if piece.is_empty() || piece != piece.trim() {
                return Some(format!("an empty or untrimmed piece {piece:?}"));
            }
            if !piece
                .chars()
                .all(|c| c.is_alphanumeric() || c == ' ' || SPEAKABLE.contains(c))
            {
                return Some(format!("1 allow-list: {piece:?}"));
            }
            if piece.chars().count() > TEXT {
                return Some(format!("2 too long: {} chars", piece.chars().count()));
            }
            if piece.contains(MARKER) {
                return Some(format!("3 fenced content said: {piece:?}"));
            }
        }
        let expected = words(&SpeechDigest::speakable(&prose(text)).join(" "));
        let got: Vec<String> = said.iter().flat_map(|piece| words(piece)).collect();
        if got != expected {
            return Some(format!(
                "4 words lost or reordered: said {got:?} but speakable gives {expected:?}"
            ));
        }
        for mode in 1..4 {
            let other = chunk(&split(text, seed.wrapping_add(mode as u64), mode)).all();
            if other != said {
                return Some(format!("6 split mode {mode} differs: {other:?} vs {said:?}"));
            }
        }
        let after: Vec<String> = whole.pushed.iter().flat_map(|piece| words(piece)).collect();
        if !got.starts_with(&after) {
            return Some("7 pushes alone are not a prefix of the whole".to_owned());
        }
        None
    }));
    outcome.unwrap_or_else(|_| Some("panicked".to_owned()))
}

fn shrunk(mut parts: Vec<String>, seed: u64, class: &str) -> String {
    let mut at = 0;
    while at < parts.len() {
        let mut fewer = parts.clone();
        fewer.remove(at);
        if broken(&fewer.concat(), seed).is_some_and(|why| why.starts_with(class)) {
            parts = fewer;
        } else {
            at += 1;
        }
    }
    parts.concat()
}

#[test]
fn test_every_chunked_reply_holds_the_invariants() {
    let mut classes: std::collections::BTreeMap<String, (usize, Vec<String>)> = std::collections::BTreeMap::new();
    for seed in (0..RUNS).chain(SEEDS.iter().copied()) {
        let parts = pieces(seed);
        let Some(why) = broken(&parts.concat(), seed) else {
            continue;
        };
        let class = why.chars().take(2).collect::<String>();
        let entry = classes.entry(class.clone()).or_default();
        entry.0 += 1;
        if entry.1.len() < 40 {
            let small = shrunk(parts, seed, &class);
            let again = broken(&small, seed).unwrap_or_default();
            let line = format!("shrunk {small:?} -> {again}");
            if !entry.1.iter().any(|known| known.ends_with(&line)) {
                entry.1.push(format!("seed {seed} {line}"));
            }
        }
    }
    let lines: Vec<String> = classes
        .iter()
        .map(|(class, (count, examples))| format!("[{class}] {count} of {RUNS} replies\n  {}", examples.join("\n  ")))
        .collect();
    assert!(
        classes.is_empty(),
        "{} class(es) break:\n{}",
        lines.len(),
        lines.join("\n---\n")
    );
}

#[test]
fn test_the_kept_regressions_still_hold() {
    let failures: Vec<String> = REGRESSIONS
        .iter()
        .filter_map(|text| broken(text, 0).map(|why| format!("{text:?}: {why}")))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn test_the_generator_is_deterministic_and_covers_what_it_claims() {
    let all: Vec<String> = (0..RUNS).map(|seed| pieces(seed).concat()).collect();
    assert_eq!(all, (0..RUNS).map(|seed| pieces(seed).concat()).collect::<Vec<_>>());
    for needle in [
        MARKER,
        "```",
        "~~~",
        "    ",
        "- ",
        "\n\n",
        "Hola.",
        "`queue.rs`",
        "https://",
        "\r\n",
        "| a | b |",
    ] {
        assert!(
            all.iter().filter(|text| text.contains(needle)).count() > 100,
            "{needle} is rare"
        );
    }
    let unclosed = all.iter().filter(|text| text.ends_with("sin cerrar")).count();
    assert!(unclosed > 100, "{unclosed}");
}

#[test]
fn test_a_long_reply_is_chunked_in_linear_time() {
    let reply = "Una frase de prueba más. ".repeat(40_000);
    let started = std::time::Instant::now();
    let chars: Vec<String> = reply.chars().map(String::from).collect();
    let mut chunker = SentenceChunker::default();
    let mut count = 0;
    for delta in &chars[..100_000] {
        count += chunker.push(delta).len();
    }
    count += chunker.finish().len();
    assert!(count >= 3_900, "{count}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

/// The two divergences from whole-text `speakable` that the design allows: the prose line under which a
/// setext underline arrives is already said, and a table whose header has no leading pipe is streamed as prose.
#[test]
fn test_the_two_allowed_divergences_are_exactly_these() {
    let cases: [(&str, Vec<&str>); 5] = [
        ("Título\n=====\n", vec!["Título"]),
        ("Título\n-----\n", vec!["Título"]),
        ("Hola\n---\n", vec!["Hola"]),
        ("Hola.\n---\n", vec!["Hola."]),
        ("a | b\n--|--\n1 | 2\n", vec!["a b"]),
    ];
    for (reply, said) in cases {
        let got = chunk(&split(reply, 0, 0)).all();
        let whole: Vec<String> = words(&SpeechDigest::speakable(&prose(reply)).join(" "));
        assert_eq!(got, said, "{reply:?}");
        assert!(whole.is_empty(), "{reply:?}: whole-text says {whole:?}");
    }
}

/// Block kinds that may directly follow prose, without a blank line: where pulldown lets them interrupt a
/// paragraph the held block must match whole-text; where it does not, whole-text reads them as prose.
#[test]
fn test_what_directly_follows_prose_without_a_blank_line() {
    let cases = [
        "Hola.\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        "Hola\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        "Hola.\n<div>\nhtml\n</div>\n",
        "Hola.\n<b>\nhtml\n",
        "Hola.\n> cita\n",
        "Hola.\n- ítem\n",
        "Hola.\n1. paso\n",
        "Hola.\n2. paso\n",
        "Hola.\n# Título\n",
        "Hola.\n    sangrado\n",
    ];
    let mut report = Vec::new();
    for reply in cases {
        let got: Vec<String> = chunk(&split(reply, 0, 0))
            .all()
            .iter()
            .flat_map(|piece| words(piece))
            .collect();
        let whole = words(&SpeechDigest::speakable(&prose(reply)).join(" "));
        if got != whole {
            report.push(format!("{reply:?}: said {got:?}, whole-text {whole:?}"));
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}

/// A block held after some of its lines were already streamed never repeats those words.
#[test]
fn test_a_held_block_never_says_again_what_its_streamed_lines_said() {
    let cases: [(&str, Vec<&str>); 6] = [
        ("Hola.\n2. paso\n", vec!["Hola.", "2.", "paso"]),
        ("Hola.\n<b>\nhtml\n", vec!["Hola.", "html"]),
        ("* ítem\n\n    sangrado", vec!["ítem", "sangrado"]),
        ("Hola mundo\n- a\n- b\n", vec!["Hola mundo", "a", "b"]),
        ("Uno. Dos\n> cita\n", vec!["Uno.", "Dos", "cita"]),
        ("Dato 1\n| a | b |\n|---|---|\n| 1 | 2 |\n", vec!["Dato 1"]),
    ];
    let mut wrong = Vec::new();
    for (reply, said) in cases {
        for mode in 0..3 {
            let got = chunk(&split(reply, 7, mode)).all();
            if got != said {
                wrong.push(format!(
                    "{reply:?} (split mode {mode}): said {got:?}, expected {said:?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// An inline code span keeps its inner whitespace when its line is cut into sentences.
#[test]
fn test_a_code_span_with_a_tab_inside_is_read_like_the_whole_text_reads_it() {
    let reply = "para` 4\t` ";
    let got: Vec<String> = chunk(&split(reply, 0, 0))
        .all()
        .iter()
        .flat_map(|piece| words(piece))
        .collect();
    assert_eq!(got, words(&SpeechDigest::speakable(&prose(reply)).join(" ")));
}

/// The third allowed divergence: the first line of an inline construct that crosses a line break is already
/// said when the rest arrives, so what the construct hides (here a code span) can leak. The allow-list still
/// holds, whatever leaks.
#[test]
fn test_the_third_allowed_divergence_is_an_inline_construct_crossing_a_line_break() {
    let reply = "para` \n4\t` ";
    let got = chunk(&split(reply, 0, 0)).all();
    assert_eq!(got, ["para", "4"]);
    assert_eq!(words(&SpeechDigest::speakable(&prose(reply)).join(" ")), ["para"]);
    for piece in &got {
        assert!(
            piece
                .chars()
                .all(|c| c.is_alphanumeric() || c == ' ' || SPEAKABLE.contains(c))
        );
    }
}
