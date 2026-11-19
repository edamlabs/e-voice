use e_voice_core::schema::lang::Lang;
use e_voice_daemon::talk::answer::Answer;

use Answer::{No, Yes};
use Lang::{En, Es};

type Case = (&'static str, Lang, Option<Answer>);

fn check(cases: &[Case]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(heard, lang, expected)| {
            let seen = Answer::of(heard, *lang);
            (seen != *expected).then(|| format!("{heard:?} ({lang:?}): expected {expected:?}, got {seen:?}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} case(s) differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

const YES_ES: &[&str] = &[
    "sí",
    "si",
    "vale",
    "adelante",
    "hazlo",
    "claro",
    "dale",
    "venga",
    "ok",
    "okay",
    "de acuerdo",
];
const NO_ES: &[&str] = &["no", "nunca", "para", "cancela", "espera", "negativo"];
const YES_EN: &[&str] = &["yes", "yeah", "yep", "sure", "ok", "okay", "go ahead", "do it"];
const NO_EN: &[&str] = &["no", "nope", "never", "stop", "cancel", "wait", "don't"];

#[test]
fn test_the_banks_are_exactly_the_specified_phrases() {
    for (phrases, lang, answer) in [(YES_ES, Es, Yes), (NO_ES, Es, No), (YES_EN, En, Yes), (NO_EN, En, No)] {
        for phrase in phrases {
            assert_eq!(Answer::of(phrase, lang), Some(answer), "{phrase:?} {lang:?}");
        }
    }
}

#[test]
fn test_spanish_answers_with_accents_case_and_punctuation() {
    check(&[
        ("Sí.", Es, Some(Yes)),
        ("SÍ", Es, Some(Yes)),
        ("si", Es, Some(Yes)),
        ("¡Vale!", Es, Some(Yes)),
        ("De acuerdo, hazlo", Es, Some(Yes)),
        ("de acuerdo", Es, Some(Yes)),
        ("  Adelante...  ", Es, Some(Yes)),
        ("OK", Es, Some(Yes)),
        ("No.", Es, No.into()),
        ("¡No!", Es, No.into()),
        ("no sé", Es, No.into()),
        ("No, gracias", Es, No.into()),
        ("Espera un momento", Es, No.into()),
        ("Cancela eso", Es, No.into()),
        ("sí, pero no", Es, No.into()),
        ("no, sí", Es, No.into()),
        ("no no no", Es, No.into()),
    ]);
}

#[test]
fn test_english_answers_and_do_it_against_dont() {
    check(&[
        ("Yes.", En, Some(Yes)),
        ("YEAH!", En, Some(Yes)),
        ("go ahead", En, Some(Yes)),
        ("Go ahead, please", En, Some(Yes)),
        ("do it", En, Some(Yes)),
        ("Do it.", En, Some(Yes)),
        ("don't", En, No.into()),
        ("Don't.", En, No.into()),
        ("don't do it", En, No.into()),
        ("No way", En, No.into()),
        ("Stop!", En, No.into()),
        ("never mind", En, No.into()),
        ("okay", En, Some(Yes)),
    ]);
}

#[test]
fn test_anything_else_is_none() {
    check(&[
        ("quizá", Es, None),
        ("quizás", Es, None),
        ("", Es, None),
        ("   ", Es, None),
        ("...", Es, None),
        ("¿?", Es, None),
        ("maybe", En, None),
        ("what", En, None),
        ("go", En, None),
        ("do", En, None),
        ("de", Es, None),
        ("pues sí", Es, None),
        ("eh, vale", Es, None),
        ("nope", Es, None),
        ("yes", Es, None),
        ("sí", En, None),
        ("nothing", En, None),
        ("noise", En, None),
        ("nonsense", En, None),
        ("nadie", Es, None),
        ("sigue", Es, None),
        ("okey dokey", En, None),
    ]);
}

#[test]
fn test_a_word_that_only_starts_like_a_phrase_is_not_the_phrase() {
    check(&[
        ("notable", Es, None),
        ("yesterday", En, None),
        ("nobody", En, None),
        ("sigo", Es, None),
        ("claramente", Es, None),
        ("parar", Es, None),
        ("stopping", En, None),
    ]);
}

#[test]
fn test_a_curly_apostrophe_is_the_same_dont() {
    check(&[("Don’t", En, No.into()), ("don’t do it", En, No.into())]);
}

#[test]
fn test_a_negation_anywhere_in_the_answer_makes_it_a_no_the_safe_side() {
    check(&[
        ("claro que no", Es, No.into()),
        ("sí, no hay problema", Es, No.into()),
        ("vale, nunca", Es, No.into()),
        ("de acuerdo, jamás", Es, No.into()),
        ("sure, never", En, No.into()),
        ("yes, not now", En, No.into()),
        ("yeah nope", En, No.into()),
        ("do it, don't wait", En, No.into()),
        ("do it, don’t wait", En, No.into()),
    ]);
}

#[test]
fn test_first_word_noes_stay_first_word_only_since_para_is_also_a_preposition() {
    check(&[
        ("para", Es, No.into()),
        ("cancela", Es, No.into()),
        ("espera", Es, No.into()),
        ("negativo", Es, No.into()),
        ("stop", En, No.into()),
        ("cancel", En, No.into()),
        ("wait", En, No.into()),
        ("vale, hazlo para mí", Es, Some(Yes)),
        ("sí, espera", Es, Some(Yes)),
        ("yes, wait for it", En, Some(Yes)),
        ("go ahead and stop", En, Some(Yes)),
    ]);
}

#[test]
fn test_unicode_and_noise_never_panic_and_never_answer() {
    for heard in [
        "🎉",
        "\0",
        "日本語",
        "ñ",
        "¿¿¿",
        "sí\u{200b}",
        "\u{feff}no",
        &"x".repeat(100_000),
        &"no ".repeat(50_000),
    ] {
        let _ = Answer::of(heard, Es);
        let _ = Answer::of(heard, En);
    }
    assert_eq!(Answer::of(&"x".repeat(100_000), Es), None);
    assert_eq!(Answer::of(&"no ".repeat(50_000), Es), Some(No));
}
