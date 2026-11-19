use std::sync::atomic::Ordering;
use std::time::Duration;

use e_voice_daemon::schema::event::RecordState::{Failed, Started};
use e_voice_daemon::workflow::record::{RecordError, Take};
use serde_json::{Value, json};

use crate::http::post_json;
use crate::rig::Rig;
use crate::running::Running;

const ES: &str = "Hoy he decidido grabar mi voz con calma.";

/// A recording over HTTP of one short speech, answered whole.
async fn recorded(rig: &Rig, body: &str) -> (u16, Value) {
    let daemon = Running::start(rig.w.state.clone()).await;
    let (port, body) = (daemon.port, body.to_owned());
    let request = tokio::task::spawn_blocking(move || post_json(port, "/voices", &body));
    rig.live().await;
    rig.started(0.0);
    rig.final_of("lo oído");
    rig.audio(1.0, 0.5).await;
    assert!(rig.w.record.finish());
    let (status, text) = request.await.unwrap();
    daemon.stop().await;
    (status, serde_json::from_str(&text).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_given_text_wins_over_the_script_and_the_transcript() {
    let rig = Rig::new(1);
    let (status, answer) = recorded(&rig, r#"{"id":"ana","text":"mi texto","lang":"es"}"#).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["text"], "mi texto");
    assert_eq!(answer["transcript"], "lo oído");
    assert_eq!(rig.clips()[0].text.as_deref(), Some("mi texto"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_script_of_the_language_wins_over_the_transcript() {
    for (lang, start) in [
        ("es", ES),
        ("en", "Today I decided to record my voice slowly and clearly."),
    ] {
        let rig = Rig::new(1);
        let (status, answer) = recorded(&rig, &json!({"id": "ana", "lang": lang}).to_string()).await;
        assert_eq!(status, 200, "{answer}");
        assert!(answer["text"].as_str().unwrap().starts_with(start), "{lang}: {answer}");
        assert_eq!(rig.clips()[0].text, answer["text"].as_str().map(str::to_owned));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_blank_text_falls_to_the_script_and_without_a_language_to_the_transcript() {
    let rig = Rig::new(1);
    let (_, answer) = recorded(&rig, r#"{"id":"ana","text":"   ","lang":"es"}"#).await;
    assert!(answer["text"].as_str().unwrap().starts_with(ES), "{answer}");
    let rig = Rig::new(1);
    let (_, answer) = recorded(&rig, r#"{"id":"ana","text":"   "}"#).await;
    assert_eq!(answer["text"], "lo oído");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_no_text_no_language_the_transcript_is_sent() {
    let rig = Rig::new(1);
    let (_, answer) = recorded(&rig, r#"{"id":"ana"}"#).await;
    assert_eq!(answer["text"], "lo oído");
    assert_eq!(rig.clips()[0].text.as_deref(), Some("lo oído"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_nothing_given_and_nothing_heard_the_text_is_absent() {
    let rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.audio(1.0, 0.5).await;
    assert!(rig.w.record.finish());
    rig.final_of("   ");
    let learned = rig.done(recording).await.unwrap();
    assert_eq!((learned.text, learned.transcript.as_str()), (None, ""));
    assert_eq!(rig.clips()[0].text, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failed_final_is_no_part_of_the_transcript() {
    use e_voice_core::schema::error::NodeError;
    use e_voice_daemon::hearing::base::Recognized;
    let rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.audio(1.0, 0.5).await;
    rig.stopped(1.0);
    rig.says(Recognized::Final(
        crate::fake::heard("roto"),
        Some(NodeError::Backend("asr".to_owned())),
    ));
    rig.audio(2.5, 0.0).await;
    assert_eq!(rig.done(recording).await.unwrap().transcript, "");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_existing_voice_is_409_and_nothing_is_recorded() {
    let mut rig = Rig::new(1);
    *rig.w.voices.answer.lock().unwrap() = Ok(vec!["ana".to_owned()]);
    let refused = rig.done(rig.start(Rig::take("ana"))).await;
    assert!(
        matches!(refused, Err(RecordError::Exists(ref id)) if id == "ana"),
        "{refused:?}"
    );
    assert_eq!(
        rig.w.mic.opened(),
        0,
        "the microphone was opened for a voice that cannot be learned"
    );
    assert!(rig.states().is_empty());
    assert!(rig.clips().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_existing_voice_over_http_is_409_naming_replace() {
    let rig = Rig::new(1);
    *rig.w.voices.answer.lock().unwrap() = Ok(vec!["ana".to_owned()]);
    let daemon = Running::start(rig.w.state.clone()).await;
    let port = daemon.port;
    let (status, text) = tokio::task::spawn_blocking(move || post_json(port, "/voices", r#"{"id":"ana"}"#))
        .await
        .unwrap();
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("replace"), "{text}");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_replace_uploads_over_an_existing_voice_without_asking_for_the_list() {
    let rig = Rig::new(1);
    *rig.w.voices.answer.lock().unwrap() = Ok(vec!["ana".to_owned()]);
    let recording = rig.start(Take {
        replace: true,
        ..Rig::take("ana")
    });
    rig.live().await;
    rig.started(0.0);
    rig.audio(1.0, 0.5).await;
    assert!(rig.w.record.finish());
    rig.final_of("hola");
    assert_eq!(rig.done(recording).await.unwrap().id, "ana");
    assert_eq!(rig.clips().len(), 1);
    assert_eq!(rig.w.voices.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_voice_list_that_fails_is_a_failure_not_a_pass() {
    use e_voice_daemon::speech::base::SpeechError;
    let rig = Rig::new(1);
    *rig.w.voices.answer.lock().unwrap() = Err(SpeechError::Connect("down".to_owned()));
    let refused = rig.done(rig.start(Rig::take("ana"))).await;
    assert!(matches!(refused, Err(RecordError::Speech(_))), "{refused:?}");
    assert_eq!(rig.w.mic.opened(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_after_a_failed_recording_the_next_one_works() {
    let mut rig = Rig::waiting(2, Duration::from_millis(200));
    assert!(matches!(
        rig.done(rig.start(Rig::take("ana"))).await,
        Err(RecordError::Timeout(_))
    ));
    assert_eq!(rig.states(), [Started, Failed]);
    let recording = rig.start(Rig::take("ana"));
    while rig.w.mic.opened() < 2 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    rig.live().await;
    rig.w.mic.block(&vec![0.5; 16_000]);
    while rig.feeds[1].sent().len() < 16_000 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    rig.feeds[1].say(e_voice_daemon::hearing::base::Recognized::Started(Duration::ZERO));
    rig.feeds[1].say(e_voice_daemon::hearing::base::Recognized::Final(
        crate::fake::heard("hola"),
        None,
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rig.w.record.finish());
    assert_eq!(rig.done(recording).await.unwrap().id, "ana");
}
