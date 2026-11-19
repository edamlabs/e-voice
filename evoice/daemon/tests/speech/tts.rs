use std::net::TcpListener;

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::speech::base::{Frame, Speech, SpeechError, SpeechRequest, Utterance};
use e_voice_daemon::speech::tts::TtsSpeech;

use crate::scripted::{Scripted, Step, floats, text};

fn request(text: &str) -> SpeechRequest {
    SpeechRequest {
        text: text.to_owned(),
        voice: None,
        lang: None,
    }
}

const READY: &str = r#"{"type":"ready","rate":24000}"#;
const CLOSED: &str = r#"{"type":"closed"}"#;

/// Frames until `closed`, or the first error.
async fn drain(url: &str, request: SpeechRequest) -> (Vec<Frame>, Option<SpeechError>) {
    let mut utterance = TtsSpeech::new(url).say(request).await.unwrap();
    let mut frames = Vec::new();
    while let Some(next) = utterance.frame().await {
        match next {
            Ok(frame) => frames.push(frame),
            Err(error) => return (frames, Some(error)),
        }
    }
    (frames, None)
}

async fn fail(steps: Vec<Step>) -> SpeechError {
    let server = Scripted::start(steps).await;
    drain(&server.url(), request("hola")).await.1.expect("an error")
}

fn query(target: &str) -> Vec<(String, String)> {
    let (_, query) = target.split_once('?').unwrap_or((target, ""));
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_owned(), decode(value))
        })
        .collect()
}

fn decode(raw: &str) -> String {
    let bytes = raw.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            out.push(u8::from_str_radix(std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap(), 16).unwrap());
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_audio_frames_arrive_at_the_rate_the_service_announced() {
    let server = Scripted::start(vec![
        text(READY),
        text(r#"{"type":"start"}"#),
        floats(&[0.5, -0.25, 0.125]),
        floats(&[1.0]),
        text(r#"{"type":"end","error":null}"#),
        text(CLOSED),
    ])
    .await;
    let (frames, error) = drain(&server.url(), request("hola")).await;
    assert_eq!(error, None);
    let rates: Vec<u32> = frames.iter().map(|frame| frame.rate.get()).collect();
    assert_eq!(rates, [24_000, 24_000]);
    assert_eq!(frames[0].samples, [0.5, -0.25, 0.125]);
    assert_eq!(frames[1].samples, [1.0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_text_then_close_are_what_the_service_receives() {
    let server = Scripted::start(vec![text(READY), text(CLOSED)]).await;
    drain(&server.url(), request("Hola, mundo")).await;
    let seen = server.seen();
    assert_eq!(
        seen.messages,
        [r#"{"text":"Hola, mundo","type":"text"}"#, r#"{"type":"close"}"#]
    );
    assert_eq!(query(&seen.target), [("format".to_owned(), "f32".to_owned())]);
    assert!(seen.target.starts_with("/v1/stream?"), "{}", seen.target);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_base_url_with_a_trailing_slash_is_trimmed() {
    let server = Scripted::start(vec![text(READY), text(CLOSED)]).await;
    drain(&format!("{}/", server.url()), request("x")).await;
    assert!(
        server.seen().target.starts_with("/v1/stream?format=f32"),
        "{}",
        server.seen().target
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_lang_and_voice_reach_the_service_as_their_own_parameters() {
    for (lang, code) in [(Lang::Es, "es"), (Lang::En, "en")] {
        let server = Scripted::start(vec![text(READY), text(CLOSED)]).await;
        let request = SpeechRequest {
            text: "x".to_owned(),
            voice: Some("ef_dora".to_owned()),
            lang: Some(lang),
        };
        drain(&server.url(), request).await;
        let pairs = query(&server.seen().target);
        assert_eq!(
            pairs,
            [
                ("format".into(), "f32".into()),
                ("lang".into(), code.into()),
                ("voice".into(), "ef_dora".into())
            ]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_voice_full_of_query_syntax_arrives_as_exactly_one_value() {
    let nasty = "a&format=s16 #ñ";
    let server = Scripted::start(vec![text(READY), text(CLOSED)]).await;
    let request = SpeechRequest {
        text: "x".to_owned(),
        voice: Some(nasty.to_owned()),
        lang: Some(Lang::Es),
    };
    drain(&server.url(), request).await;
    let seen = server.seen();
    let pairs = query(&seen.target);
    assert_eq!(
        pairs,
        [
            ("format".to_owned(), "f32".to_owned()),
            ("lang".to_owned(), "es".to_owned()),
            ("voice".to_owned(), nasty.to_owned())
        ],
        "{}",
        seen.target
    );
    assert!(
        !seen.target.contains('#') && !seen.target.contains(' '),
        "{}",
        seen.target
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pings_and_unknown_events_are_skipped() {
    let server = Scripted::start(vec![
        Step::Ping,
        text(READY),
        text(r#"{"type":"something-new","x":1}"#),
        floats(&[0.1]),
        text(CLOSED),
    ])
    .await;
    let (frames, error) = drain(&server.url(), request("x")).await;
    assert_eq!((frames.len(), error), (1, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_end_with_an_error_is_a_server_error_carrying_it() {
    let error = fail(vec![text(READY), text(r#"{"type":"end","error":{"kind":"timeout"}}"#)]).await;
    assert_eq!(error, SpeechError::Server(r#"{"kind":"timeout"}"#.to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_error_event_is_a_server_error_with_its_message() {
    assert_eq!(
        fail(vec![text(r#"{"type":"error","message":"no voice 'x'"}"#)]).await,
        SpeechError::Server("no voice 'x'".to_owned())
    );
    assert_eq!(
        fail(vec![text(r#"{"type":"error"}"#)]).await,
        SpeechError::Server(String::new())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_audio_before_ready_is_a_protocol_error() {
    let error = fail(vec![floats(&[0.1])]).await;
    assert!(
        matches!(&error, SpeechError::Protocol(why) if why.contains("before `ready`")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_ready_with_rate_zero_is_a_protocol_error() {
    let error = fail(vec![text(r#"{"type":"ready","rate":0}"#)]).await;
    assert!(
        matches!(&error, SpeechError::Protocol(why) if why.contains("rate 0")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_frame_that_is_not_whole_f32s_is_a_protocol_error() {
    let error = fail(vec![text(READY), Step::Binary(vec![0, 0, 0, 0, 0, 0])]).await;
    assert!(
        matches!(&error, SpeechError::Protocol(why) if why.contains("6 bytes")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_malformed_json_is_a_protocol_error() {
    let error = fail(vec![text(READY), text("{not json")]).await;
    assert!(matches!(error, SpeechError::Protocol(_)), "{error:?}");
    let untyped = fail(vec![text(r#"{"rate":1}"#)]).await;
    assert!(matches!(untyped, SpeechError::Protocol(_)), "{untyped:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_close_before_closed_is_a_protocol_error() {
    let error = fail(vec![text(READY), floats(&[0.1]), Step::Close]).await;
    assert!(
        matches!(&error, SpeechError::Protocol(why) if why.contains("before `closed`")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_connection_dropped_before_closed_is_a_protocol_error() {
    let error = fail(vec![text(READY), Step::Drop]).await;
    assert!(matches!(error, SpeechError::Protocol(_)), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_refused_connection_is_a_connect_error() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let outcome = TtsSpeech::new(format!("ws://127.0.0.1:{port}")).say(request("x")).await;
    assert!(
        matches!(outcome, Err(SpeechError::Connect(_))),
        "{:?}",
        outcome.map(|_| ())
    );
}

/// Control: the checker is not blind. The URL a naive concatenation would send reads as extra parameters.
#[test]
fn test_control_a_naive_url_would_split_the_voice() {
    let naive = "/v1/stream?format=f32&lang=es&voice=a&format=s16 #ñ";
    let pairs = query(naive);
    assert_ne!(pairs.len(), 3);
    assert_eq!(pairs.iter().filter(|(key, _)| key == "format").count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_base_url_that_is_not_a_url_is_a_connect_error() {
    let outcome = TtsSpeech::new("not a url").say(request("x")).await;
    assert!(
        matches!(outcome, Err(SpeechError::Connect(_))),
        "{:?}",
        outcome.map(|_| ())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asking_again_after_a_close_is_still_a_protocol_error() {
    let server = Scripted::start(vec![text(READY), Step::Close]).await;
    let mut utterance = TtsSpeech::new(server.url()).say(request("x")).await.unwrap();
    for _ in 0..3 {
        let next = utterance.frame().await;
        assert!(matches!(next, Some(Err(SpeechError::Protocol(_)))), "{next:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_utterance_prints_its_rate_without_its_socket() {
    let server = Scripted::start(vec![text(READY), floats(&[0.1]), text(CLOSED)]).await;
    let mut utterance = TtsSpeech::new(server.url()).say(request("x")).await.unwrap();
    assert!(format!("{utterance:?}").contains("rate: None"));
    utterance.frame().await;
    let shown = format!("{utterance:?}");
    assert!(shown.starts_with("TtsUtterance") && shown.contains("24000"), "{shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancel_sends_the_cancel_message_after_the_text_and_close() {
    let server = Scripted::start(vec![text(READY)]).await;
    let mut utterance = TtsSpeech::new(server.url()).say(request("hola")).await.unwrap();
    utterance.cancel().await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server.seen().messages.len() < 3 {
        assert!(std::time::Instant::now() < deadline, "{:?}", server.seen().messages);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        server.seen().messages,
        [
            r#"{"text":"hola","type":"text"}"#,
            r#"{"type":"close"}"#,
            r#"{"type":"cancel"}"#
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancel_on_a_dead_connection_is_a_protocol_error() {
    let server = Scripted::start(vec![Step::Drop]).await;
    let mut utterance = TtsSpeech::new(server.url()).say(request("hola")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut failed = false;
    for _ in 0..50 {
        if let Err(error) = utterance.cancel().await {
            assert!(matches!(error, SpeechError::Protocol(_)), "{error:?}");
            failed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(failed, "cancelling on a closed socket never failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_ready_rate_outside_the_bound_is_a_protocol_error_naming_it() {
    for rate in [4_294_967_295u64, 384_001, 0] {
        let error = fail(vec![text(&format!(r#"{{"type":"ready","rate":{rate}}}"#))]).await;
        assert!(
            matches!(&error, SpeechError::Protocol(why) if why.contains(&format!("rate {rate}"))),
            "{rate}: {error:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_ready_rate_at_the_bound_is_accepted() {
    let server = Scripted::start(vec![
        text(r#"{"type":"ready","rate":384000}"#),
        floats(&[0.1, 0.2]),
        text(CLOSED),
    ])
    .await;
    let (frames, error) = drain(&server.url(), request("x")).await;
    assert_eq!(error, None);
    assert_eq!(frames[0].rate.get(), 384_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_binary_message_over_256_kib_is_a_protocol_error_and_one_just_under_is_a_frame() {
    let flat = |samples: usize| floats(&vec![0.25; samples]);
    let server = Scripted::start(vec![text(READY), flat(60_000), text(CLOSED)]).await;
    let (frames, error) = drain(&server.url(), request("x")).await;
    assert_eq!((frames.len(), frames[0].samples.len(), error), (1, 60_000, None));
    let error = fail(vec![text(READY), flat(70_000)]).await;
    assert!(matches!(error, SpeechError::Protocol(_)), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_text_message_over_256_kib_is_a_protocol_error() {
    let big = format!(r#"{{"type":"something-new","pad":"{}"}}"#, "a".repeat(270_000));
    let error = fail(vec![text(READY), text(&big)]).await;
    assert!(matches!(error, SpeechError::Protocol(_)), "{error:?}");
}
