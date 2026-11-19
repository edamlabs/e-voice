use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::speech::digest::{CHARS, SENTENCES, SpeechDigest};

use Lang::{En, Es};

type Case = (&'static str, Lang, Option<(&'static str, Lang)>);

fn of(markdown: &str, fallback: Lang) -> Option<(String, Lang)> {
    SpeechDigest::of(markdown, fallback)
}

/// Every case is run; the failures are listed whole, so one report names them all.
fn check(cases: &[Case]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(input, fallback, expected)| {
            let seen = of(input, *fallback);
            let want = expected.map(|(text, lang)| (text.to_owned(), lang));
            (seen != want).then(|| format!("{input:?} (fallback {fallback:?}): expected {want:?}, got {seen:?}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} case(s) differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn test_the_limits_are_two_sentences_and_three_hundred_characters() {
    assert_eq!((SENTENCES, CHARS), (2, 300));
}

#[test]
fn test_plain_prose_is_the_first_two_sentences_in_the_language_its_words_decide() {
    check(&[
        (
            "Hecho. He cambiado la cola.",
            Es,
            Some(("Hecho. He cambiado la cola.", Es)),
        ),
        ("One. Two. Three.", En, Some(("One. Two.", En))),
        ("¿Lo pruebo? ¡Sí! Luego más.", Es, Some(("¿Lo pruebo? ¡Sí!", Es))),
        ("Wait… then go. Last.", Es, Some(("Wait… then go.", En))),
        ("Canción, acción, ñandú.", Es, Some(("Canción, acción, ñandú.", Es))),
        (
            "Version v3.11.0 took 3.5 s. Done here.",
            Es,
            Some(("Version v3.11.0 took 3.5 s. Done here.", En)),
        ),
        ("Use e.g. this. And that.", Es, Some(("Use e.g. this. And that.", En))),
        (
            "Por ejemplo, p. ej. esto. Y eso.",
            En,
            Some(("Por ejemplo, p. ej. esto. Y eso.", Es)),
        ),
    ]);
}

#[test]
fn test_what_does_not_read_aloud_is_left_out() {
    check(&[
        (
            "```rust\nfn x() {}\n```\nI fixed the queue. It works now.",
            Es,
            Some(("I fixed the queue. It works now.", En)),
        ),
        ("# Title\n\nThe text is here.", Es, Some(("The text is here.", En))),
        ("Call `foo()` now.", Es, Some(("Call now.", En))),
        ("Set `max_size` to 4.", Es, Some(("Set max size to 4.", En))),
        ("Listo ✅ → todo bien. 🎉", Es, Some(("Listo todo bien.", Es))),
        ("| a | b |\n|---|---|\n| 1 | 2 |", Es, None),
        ("```\nonly code\n```", Es, None),
        ("   \n  ", Es, None),
        ("", Es, None),
    ]);
}

#[test]
fn test_paths_urls_and_file_names_are_read_as_their_last_component_or_not_at_all() {
    check(&[
        (
            "I edited `queue.rs` and `src/workflow/listen.rs`.",
            Es,
            Some(("I edited queue.rs and listen.rs.", En)),
        ),
        (
            "See [the docs](https://x.y/z) and https://a.b/c.",
            Es,
            Some(("See the docs and.", En)),
        ),
    ]);
}

#[test]
fn test_block_boundaries_end_a_sentence() {
    check(&[
        (
            "Changes:\n\n- added a test\n- removed a bug",
            Es,
            Some(("Changes: added a test", Es)),
        ),
        ("> quoted line\n\nAfter.", Es, Some(("quoted line After.", Es))),
    ]);
}

#[test]
fn test_the_language_is_the_one_with_more_common_words_and_a_tie_is_the_fallback() {
    check(&[
        ("the cola is lista", Es, Some(("the cola is lista", En))),
        ("el the", Es, Some(("el the", Es))),
        ("el the", En, Some(("el the", En))),
        ("ñandú perro gato", En, Some(("ñandú perro gato", En))),
    ]);
}

#[test]
fn test_a_sentence_over_the_cap_is_cut_at_a_word_and_marked() {
    let sentence = format!("{}.", "ñandú palabra ".repeat(45).trim_end());
    assert!(sentence.chars().count() > 600 - 20);
    let (text, _) = of(&sentence, Es).unwrap();
    assert!(text.ends_with('…'), "{text}");
    assert!(text.chars().count() <= CHARS + 1, "{}", text.chars().count());
    let body = text.trim_end_matches('…');
    assert!(sentence.starts_with(body), "not a prefix: {body}");
    assert!(sentence[body.len()..].starts_with(' '), "not cut at a space");
    assert!(!body.ends_with(' '));
}

#[test]
fn test_a_text_within_the_cap_is_not_marked_and_a_word_longer_than_the_cap_still_returns() {
    let (text, _) = of("Una frase corta.", Es).unwrap();
    assert!(!text.ends_with('…'));
    let word = "x".repeat(CHARS * 2);
    let (cut, _) = of(&word, Es).unwrap();
    assert!(
        cut.ends_with('…') && cut.chars().count() <= CHARS + 1,
        "{}",
        cut.chars().count()
    );
}

#[test]
fn test_text_with_nothing_to_say_is_none_not_a_stray_mark() {
    check(&[
        (".", Es, None),
        ("https://a.b/c.", Es, None),
        ("✅ ✅ ✅", Es, None),
        ("https://a.b/c", Es, None),
        ("![alt](http://x.y/i.png)", Es, None),
        ("<div>html only</div>", Es, None),
        ("---", Es, None),
        (".", Es, None),
        ("https://a.b/c.", Es, None),
    ]);
}

#[test]
fn test_punctuation_alone_is_not_something_to_say() {
    check(&[
        ("...", Es, None),
        (".", Es, None),
        ("https://a.b/c.", Es, None),
        (".", Es, None),
        ("https://a.b/c.", Es, None),
        (".", Es, None),
        ("https://a.b/c.", Es, None),
        ("@@@ $$$", Es, None),
        ("()", Es, None),
        ("[]", Es, None),
        ("«»", Es, None),
        ("¿?", Es, None),
        ("¡!", Es, None),
        ("~~~", Es, None),
        ("***", Es, None),
        ("\\", Es, None),
        (".", Es, None),
        ("https://a.b/c.", Es, None),
        ("a", Es, Some(("a", Es))),
        ("1", Es, Some(("1", Es))),
    ]);
}

#[test]
fn test_hostile_input_neither_panics_nor_takes_long() {
    let started = Instant::now();
    let inputs = [
        "a ".repeat(500_000),
        "> ".repeat(100_000),
        "[".repeat(200_000),
        "*".repeat(200_000),
        "- ".repeat(200_000),
        "ñ".repeat(1_000_000),
        "\u{0}\u{1}\u{7f}\u{200d}".repeat(50_000),
        format!("{}.", "palabra ".repeat(130_000)),
        "| a |\n|---|\n".repeat(50_000),
        "<".repeat(300_000),
        "`".repeat(300_000),
    ];
    for input in &inputs {
        let _ = of(input, Es);
    }
    assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
}

#[test]
fn test_one_megabyte_of_prose_is_digested_in_well_under_a_second_release_budget_ten_debug() {
    let prose = "Esto es una frase de prueba. ".repeat(36_000);
    assert!(prose.len() > 1_000_000);
    let started = Instant::now();
    let (text, lang) = of(&prose, En).unwrap();
    assert_eq!(text, "Esto es una frase de prueba. Esto es una frase de prueba.");
    assert_eq!(lang, Es);
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

#[test]
fn test_the_digest_is_deterministic_and_its_text_never_exceeds_the_cap_for_any_cut() {
    for size in [1usize, 2, 5, 299, 300, 301, 302, 450, 1000] {
        let input = format!("{}.", "áé ".repeat(size));
        let first = of(&input, Es);
        assert_eq!(first, of(&input, Es));
        if let Some((text, _)) = first {
            assert!(text.chars().count() <= CHARS + 1, "{size}: {}", text.chars().count());
        }
    }
}

#[test]
fn test_a_punctuation_only_sentence_does_not_use_up_one_of_the_two_slots() {
    check(&[
        ("Done. ... Next.", En, Some(("Done. Next.", En))),
        ("Hecho. — Listo. Tercero.", Es, Some(("Hecho. Listo.", Es))),
        ("Done. https://a.b/c. Next. Last.", En, Some(("Done. Next.", En))),
        ("...", En, None),
    ]);
}
