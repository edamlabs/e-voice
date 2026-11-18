use std::net::TcpListener;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use e_voice_core::schema::emotion::{Emotion, EmotionLabel};
use e_voice_core::schema::error::NodeError;
use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::{Heard, Hearing, HearingError, Recognized, Transcription};
use e_voice_daemon::hearing::stt::{SttEvent, SttHearing, SttVoice};

use crate::scripted::{Scripted, Step, text};

fn rate() -> NonZeroU32 {
    NonZeroU32::new(16_000).unwrap()
}

const FINAL: &str = r#"{"type":"final","text":"hola","lang":"es","emotion":{"label":"happy","scores":{"happy":0.9},"model":"ser"},"error":null}"#;

async fn events(steps: Vec<Step>, count: usize) -> Vec<Option<Result<Recognized, HearingError>>> {
    let server = Scripted::start(steps).await;
    let mut session = SttHearing::new(server.url()).open(rate()).await.unwrap();
    let mut got = Vec::new();
    for _ in 0..count {
        got.push(
            tokio::time::timeout(Duration::from_secs(5), session.next())
                .await
                .unwrap(),
        );
    }
    got
}

#[test]
fn test_url_asks_for_mono_f32le_struct_events_with_the_emotion_as_a_field() {
    let url = SttHearing::new("ws://127.0.0.1:5500").url(rate()).unwrap();
    assert_eq!(url.path(), "/v1/stream");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    assert_eq!(
        pairs,
        [
            pair("rate", "16000"),
            pair("encoding", "f32le"),
            pair("view", "struct"),
            pair("emotion", "field")
        ]
    );
}

#[test]
fn test_url_carries_the_given_rate_and_tolerates_trailing_slashes() {
    for base in ["ws://h:1", "ws://h:1/", "ws://h:1//"] {
        let url = SttHearing::new(base).url(NonZeroU32::new(48_000).unwrap()).unwrap();
        assert_eq!(url.path(), "/v1/stream", "{base}");
        assert!(url.query().unwrap().starts_with("rate=48000&"), "{base}");
    }
}

#[test]
fn test_url_of_a_base_that_is_not_a_url_is_a_connect_error() {
    let error = SttHearing::new("not a url").url(rate()).unwrap_err();
    assert!(matches!(error, HearingError::Connect(_)), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_server_sees_exactly_the_query_the_driver_builds() {
    let server = Scripted::start(vec![Step::Wait(Duration::from_millis(50)), Step::Drop]).await;
    let hearing = SttHearing::new(server.url());
    let _session = hearing.open(rate()).await.unwrap();
    let wanted = hearing.url(rate()).unwrap();
    let expected = format!("{}?{}", wanted.path(), wanted.query().unwrap());
    assert_eq!(server.target(), expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_samples_go_out_as_one_binary_frame_each_little_endian_bit_exact() {
    let server = Scripted::start(vec![Step::Wait(Duration::from_secs(2))]).await;
    let mut session = SttHearing::new(server.url()).open(rate()).await.unwrap();
    let blocks: [Vec<f32>; 3] = [
        vec![0.5, -1.0, f32::MIN_POSITIVE, f32::MAX, -0.0],
        vec![f32::NAN, f32::INFINITY, f32::from_bits(0x0000_0001)],
        vec![0.25],
    ];
    for block in &blocks {
        session.send(block).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let expected: Vec<Vec<u8>> = blocks
        .iter()
        .map(|block| block.iter().flat_map(|sample| sample.to_le_bytes()).collect())
        .collect();
    assert_eq!(server.frames(), expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_event_stream_maps_to_recognized_and_skips_what_the_daemon_does_not_use() {
    let got = events(
        vec![
            text(r#"{"type":"speech","state":"started"}"#),
            text(r#"{"type":"speech","state":"stopped"}"#),
            text(r#"{"type":"wake","word":"hey"}"#),
            text(r#"{"type":"speakers","segments":[]}"#),
            text(r#"{"type":"something-new","x":1}"#),
            Step::Ping,
            text(r#"{"type":"partial","text":"ho"}"#),
            text(FINAL),
        ],
        3,
    )
    .await;
    let emotion = Emotion {
        label: EmotionLabel::Happy,
        scores: [(EmotionLabel::Happy, 0.9)].into(),
        model: Some("ser".to_owned()),
    };
    let heard = Heard {
        text: "hola".to_owned(),
        lang: Lang::Es,
        emotion,
    };
    assert_eq!(got[0], Some(Ok(Recognized::Started)));
    assert_eq!(got[1], Some(Ok(Recognized::Partial("ho".to_owned()))));
    assert_eq!(got[2], Some(Ok(Recognized::Final(heard, None))));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_final_carries_the_error_that_kept_it_from_finishing() {
    let json = r#"{"type":"final","text":"","lang":"en","emotion":{"label":"unknown","scores":{},"model":null},"error":{"kind":"timeout"}}"#;
    let got = events(vec![text(json)], 1).await;
    match &got[0] {
        Some(Ok(Recognized::Final(heard, Some(NodeError::Timeout)))) => {
            assert_eq!((heard.text.as_str(), heard.lang), ("", Lang::En));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_closed_and_a_normal_close_end_the_session_and_any_other_close_is_the_recognizers_reason() {
    assert_eq!(events(vec![text(r#"{"type":"closed"}"#)], 1).await, [None]);
    assert_eq!(events(vec![Step::Close(1000, "bye")], 1).await, [None]);
    let got = events(vec![Step::Close(1011, "model failed")], 1).await;
    match &got[0] {
        Some(Err(HearingError::Server(message))) => {
            assert!(
                message.contains("1011") && message.contains("model failed"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
    let got = events(vec![Step::Close(1003, "audio refused")], 1).await;
    assert!(
        matches!(&got[0], Some(Err(HearingError::Server(m))) if m.contains("1003")),
        "{got:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_binary_frame_malformed_json_or_a_dropped_connection_is_a_protocol_error() {
    for step in [
        Step::Binary(vec![1, 2, 3]),
        text("not json"),
        text(r#"{"type":"final"}"#),
        Step::Drop,
    ] {
        let got = events(vec![step.clone()], 1).await;
        assert!(
            matches!(&got[0], Some(Err(HearingError::Protocol(_)))),
            "{step:?}: {got:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_message_over_64_kib_is_refused_and_one_just_under_is_read() {
    let big = |size: usize| format!(r#"{{"type":"partial","text":"{}"}}"#, "a".repeat(size));
    let got = events(vec![text(&big(60_000))], 1).await;
    assert!(
        matches!(&got[0], Some(Ok(Recognized::Partial(t))) if t.len() == 60_000),
        "{:?}",
        got[0].as_ref().map(Result::is_ok)
    );
    let got = events(vec![text(&big(70_000))], 1).await;
    assert!(
        matches!(&got[0], Some(Err(HearingError::Protocol(_)))),
        "{:?}",
        got[0].as_ref().map(Result::is_ok)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_next_dropped_between_events_loses_none() {
    let steps: Vec<Step> = (0..6)
        .flat_map(|n| {
            [
                Step::Wait(Duration::from_millis(25)),
                text(&format!(r#"{{"type":"partial","text":"{n}"}}"#)),
            ]
        })
        .collect();
    let server = Scripted::start(steps).await;
    let mut session = SttHearing::new(server.url()).open(rate()).await.unwrap();
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while got.len() < 6 && Instant::now() < deadline {
        if let Ok(Some(Ok(Recognized::Partial(t)))) =
            tokio::time::timeout(Duration::from_millis(5), session.next()).await
        {
            got.push(t);
        }
        session.send(&[0.0; 4]).await.unwrap();
    }
    assert_eq!(got, ["0", "1", "2", "3", "4", "5"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_port_nobody_holds_is_a_connect_error_at_once() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let started = Instant::now();
    let error = SttHearing::new(format!("ws://127.0.0.1:{port}"))
        .open(rate())
        .await
        .unwrap_err();
    assert!(matches!(error, HearingError::Connect(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_recognizer_that_never_answers_the_handshake_is_a_connect_error_within_five_seconds() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let started = Instant::now();
    let error = SttHearing::new(url).open(rate()).await.unwrap_err();
    let took = started.elapsed();
    assert!(
        matches!(&error, HearingError::Connect(m) if m.contains("no handshake within 5 s")),
        "{error:?}"
    );
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(6),
        "{took:?}"
    );
}

#[test]
fn test_the_mirror_parses_the_voice_states_and_files_the_rest_under_other() {
    let parse = |json: &str| serde_json::from_str::<SttEvent>(json).unwrap();
    assert_eq!(
        parse(r#"{"type":"speech","state":"started","at":1.5}"#),
        SttEvent::Speech {
            state: SttVoice::Started
        }
    );
    assert_eq!(
        parse(r#"{"type":"speech","state":"stopped"}"#),
        SttEvent::Speech {
            state: SttVoice::Stopped
        }
    );
    assert_eq!(parse(r#"{"type":"wake","word":"x"}"#), SttEvent::Other);
    assert_eq!(parse(r#"{"type":"closed"}"#), SttEvent::Closed {});
    assert!(serde_json::from_str::<SttEvent>(r#"{"type":"speech","state":"paused"}"#).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_sending_after_the_recognizer_is_gone_is_a_protocol_error() {
    let server = Scripted::start(vec![Step::Drop]).await;
    let mut session = SttHearing::new(server.url()).open(rate()).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut failed = None;
    while failed.is_none() && Instant::now() < deadline {
        failed = session.send(&[0.0; 1_600]).await.err();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(matches!(failed, Some(HearingError::Protocol(_))), "{failed:?}");
    assert_eq!(format!("{session:?}"), "SttTranscription { .. }");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asking_again_after_a_normal_close_reads_as_a_dropped_connection() {
    let server = Scripted::start(vec![Step::Close(1000, "bye"), Step::Wait(Duration::from_millis(500))]).await;
    let mut session = SttHearing::new(server.url()).open(rate()).await.unwrap();
    assert_eq!(session.next().await, None);
    let again = tokio::time::timeout(Duration::from_secs(5), session.next())
        .await
        .unwrap();
    assert!(
        matches!(again, Some(Err(HearingError::Protocol(ref m))) if m.contains("dropped")),
        "{again:?}"
    );
}
