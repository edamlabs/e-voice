use std::collections::BTreeMap;

use e_voice_core::schema::emotion::{Emotion, EmotionLabel};
use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::Heard;
use e_voice_daemon::schema::event::{DaemonEvent, ListenState, SpeechState};

fn wire(event: &DaemonEvent) -> String {
    serde_json::to_string(event).unwrap()
}

#[test]
fn test_speech_events_are_pinned_byte_for_byte() {
    let speech = |state| wire(&DaemonEvent::Speech(state));
    assert_eq!(speech(SpeechState::Idle), r#"{"type":"speech","state":"idle"}"#);
    assert_eq!(
        speech(SpeechState::Speaking { id: 3 }),
        r#"{"type":"speech","state":"speaking","id":3}"#
    );
    assert_eq!(
        speech(SpeechState::Draining { id: 3 }),
        r#"{"type":"speech","state":"draining","id":3}"#
    );
    assert_eq!(
        speech(SpeechState::Cancelling { id: 3 }),
        r#"{"type":"speech","state":"cancelling","id":3}"#
    );
    assert_eq!(
        speech(SpeechState::Speaking { id: u64::MAX }),
        format!(r#"{{"type":"speech","state":"speaking","id":{}}}"#, u64::MAX)
    );
}

#[test]
fn test_listening_events_are_pinned_byte_for_byte() {
    let listening = |state| wire(&DaemonEvent::Listening { state });
    assert_eq!(listening(ListenState::Open), r#"{"type":"listening","state":"open"}"#);
    assert_eq!(
        listening(ListenState::Closed),
        r#"{"type":"listening","state":"closed"}"#
    );
}

#[test]
fn test_partial_and_lagged_are_pinned_byte_for_byte() {
    assert_eq!(
        wire(&DaemonEvent::Partial {
            text: "hola".to_owned()
        }),
        r#"{"type":"partial","text":"hola"}"#
    );
    assert_eq!(
        wire(&DaemonEvent::Lagged { missed: 344 }),
        r#"{"type":"lagged","missed":344}"#
    );
}

#[test]
fn test_text_is_escaped_as_json_and_unicode_is_kept() {
    let partial = |text: &str| wire(&DaemonEvent::Partial { text: text.to_owned() });
    assert_eq!(partial("a\"b\\c\nd"), r#"{"type":"partial","text":"a\"b\\c\nd"}"#);
    assert_eq!(
        partial("¿Qué tal, señor? 日本"),
        "{\"type\":\"partial\",\"text\":\"¿Qué tal, señor? 日本\"}"
    );
    assert_eq!(partial(""), r#"{"type":"partial","text":""}"#);
}

#[test]
fn test_heard_is_pinned_with_its_lang_and_emotion() {
    let plain = DaemonEvent::Heard(Heard {
        text: "hola".to_owned(),
        lang: Lang::Es,
        emotion: Emotion::default(),
    });
    assert_eq!(
        wire(&plain),
        r#"{"type":"heard","text":"hola","lang":"es","emotion":{"label":"unknown","scores":{},"model":null}}"#
    );
    let weights = BTreeMap::from([(EmotionLabel::Happy, 0.75), (EmotionLabel::Neutral, 0.25)]);
    let scored = DaemonEvent::Heard(Heard {
        text: "hi".to_owned(),
        lang: Lang::En,
        emotion: Emotion {
            label: EmotionLabel::Happy,
            scores: weights,
            model: Some("ser".to_owned()),
        },
    });
    assert_eq!(
        wire(&scored),
        r#"{"type":"heard","text":"hi","lang":"en","emotion":{"label":"happy","scores":{"happy":0.75,"neutral":0.25},"model":"ser"}}"#
    );
}

#[test]
fn test_every_kind_is_a_flat_object_with_its_type_first() {
    let all = [
        DaemonEvent::Speech(SpeechState::Idle),
        DaemonEvent::Listening {
            state: ListenState::Open,
        },
        DaemonEvent::Partial { text: String::new() },
        DaemonEvent::Heard(Heard {
            text: String::new(),
            lang: Lang::Es,
            emotion: Emotion::default(),
        }),
        DaemonEvent::Lagged { missed: 1 },
    ];
    let kinds: Vec<String> = all
        .iter()
        .map(|event| {
            let text = wire(event);
            assert!(text.starts_with(r#"{"type":""#), "{text}");
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["speech", "listening", "partial", "heard", "lagged"]);
}
