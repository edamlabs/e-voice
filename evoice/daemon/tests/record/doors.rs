use std::sync::atomic::Ordering;
use std::time::Duration;

use e_voice_daemon::schema::event::RecordState::{Failed, Finished, Heard, Learned, Started};
use e_voice_daemon::speech::base::SpeechError;
use serde_json::{Value, json};

use crate::http::{post_json, request};
use crate::rig::Rig;
use crate::running::Running;

async fn post(port: u16, path: &'static str, body: &str) -> (u16, Value) {
    let body = body.to_owned();
    let (status, text) = tokio::task::spawn_blocking(move || post_json(port, path, &body))
        .await
        .unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}")),
    )
}

fn error_of(answer: &Value) -> &str {
    answer["error"]
        .as_str()
        .unwrap_or_else(|| panic!("no error in {answer}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_post_voices_answers_the_id_the_seconds_the_text_and_the_transcript() {
    let mut rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let request = tokio::spawn(post(daemon.port, "/voices", r#"{"id":"ana","text":"lo que leo"}"#));
    rig.live().await;
    rig.audio(1.0, 0.0).await;
    rig.started(1.0);
    rig.audio(2.0, 0.5).await;
    rig.stopped(3.0);
    rig.final_of("lo que oigo");
    rig.audio(2.5, 0.0).await;
    let (status, answer) = request.await.unwrap();
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        answer,
        json!({"id": "ana", "seconds": 2.0, "text": "lo que leo", "transcript": "lo que oigo"})
    );
    assert_eq!(rig.states(), [Started, Heard, Finished, Learned]);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_post_voices_finish_ends_the_recording_and_says_whether_one_ran() {
    let rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let (status, answer) = post(daemon.port, "/voices/finish", "").await;
    assert_eq!((status, answer), (200, json!({"finished": false})));
    let request = tokio::spawn(post(daemon.port, "/voices", r#"{"id":"ana"}"#));
    rig.live().await;
    rig.started(0.0);
    rig.audio(1.0, 0.5).await;
    let (status, answer) = post(daemon.port, "/voices/finish", "").await;
    assert_eq!((status, answer), (200, json!({"finished": true})));
    rig.final_of("hola");
    let (status, answer) = request.await.unwrap();
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["seconds"], 1.0);
    assert_eq!(answer["text"], "hola", "no text given: the transcript");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_nobody_speaking_within_the_listen_bound_is_408_failed_and_learns_nothing() {
    let mut rig = Rig::waiting(1, Duration::from_millis(300));
    let daemon = Running::start(rig.w.state.clone()).await;
    let (status, answer) = post(daemon.port, "/voices", r#"{"id":"ana"}"#).await;
    assert_eq!(status, 408, "{answer}");
    assert!(error_of(&answer).contains("nobody started speaking"), "{answer}");
    assert!(rig.clips().is_empty());
    assert_eq!(rig.states(), [Started, Failed]);
    assert!(rig.w.mic.freed().await);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_finish_before_anyone_spoke_is_408_and_learns_nothing() {
    let mut rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let request = tokio::spawn(post(daemon.port, "/voices", r#"{"id":"ana"}"#));
    rig.live().await;
    rig.audio(0.5, 0.0).await;
    let (_, finished) = post(daemon.port, "/voices/finish", "").await;
    assert_eq!(finished, json!({"finished": true}));
    let (status, answer) = request.await.unwrap();
    assert_eq!(status, 408, "{answer}");
    assert!(error_of(&answer).contains("before anyone spoke"), "{answer}");
    assert!(rig.clips().is_empty());
    assert_eq!(rig.states(), [Started, Finished, Failed]);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_refusal_by_the_speech_service_is_502_with_its_reason_and_failed() {
    let mut rig = Rig::new(1);
    *rig.w.learning.refuse.lock().unwrap() = Some(SpeechError::Server("422: the clip is too short".to_owned()));
    let daemon = Running::start(rig.w.state.clone()).await;
    let request = tokio::spawn(post(daemon.port, "/voices", r#"{"id":"ana","max":"1s"}"#));
    rig.live().await;
    rig.started(0.0);
    rig.final_of("hola");
    rig.audio(1.0, 0.5).await;
    let (status, answer) = request.await.unwrap();
    assert_eq!(status, 502, "{answer}");
    assert!(error_of(&answer).contains("the clip is too short"), "{answer}");
    assert_eq!(rig.states(), [Started, Heard, Finished, Failed]);
    assert!(rig.clips().is_empty());
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_microphone_that_cannot_open_is_503() {
    let rig = Rig::new(1);
    rig.w.mic.fail.store(true, Ordering::SeqCst);
    let daemon = Running::start(rig.w.state.clone()).await;
    let (status, answer) = post(daemon.port, "/voices", r#"{"id":"ana"}"#).await;
    assert_eq!(status, 503, "{answer}");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_recognizer_that_cannot_open_is_502() {
    let rig = Rig::new(0);
    let daemon = Running::start(rig.w.state.clone()).await;
    let (status, answer) = post(daemon.port, "/voices", r#"{"id":"ana"}"#).await;
    assert_eq!(status, 502, "{answer}");
    assert!(rig.w.mic.freed().await, "the microphone stayed open");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_take_that_is_not_one_is_400_before_the_microphone_or_the_speech_service_are_touched() {
    let rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let bodies = [
        r#"{"id":""}"#.to_owned(),
        r#"{"id":"Ana"}"#.to_owned(),
        r#"{"id":"a b"}"#.to_owned(),
        r#"{"id":"../x"}"#.to_owned(),
        r#"{"id":"ñandú"}"#.to_owned(),
        format!(r#"{{"id":"{}"}}"#, "a".repeat(65)),
        r#"{"id":"ana","silence":"0s"}"#.to_owned(),
        r#"{"id":"ana","silence":"31s"}"#.to_owned(),
        r#"{"id":"ana","max":"0s"}"#.to_owned(),
        r#"{"id":"ana","max":"31s"}"#.to_owned(),
        r#"{"id":"ana","max":"abc"}"#.to_owned(),
        r#"{"id":"ana","lang":"xx"}"#.to_owned(),
        r#"{"id":"ana","bogus":1}"#.to_owned(),
        r#"{"id":7}"#.to_owned(),
        "{}".to_owned(),
        "[]".to_owned(),
        r#"["ana"]"#.to_owned(),
        r#""ana""#.to_owned(),
        "7".to_owned(),
        "null".to_owned(),
        "{".to_owned(),
        String::new(),
    ];
    for body in bodies {
        let (status, answer) = post(daemon.port, "/voices", &body).await;
        assert_eq!(status, 400, "{body}: {answer}");
        assert!(answer["error"].is_string(), "{body}: {answer}");
    }
    assert_eq!(rig.w.mic.opened(), 0);
    assert_eq!(rig.w.hearing.opened(), 0);
    assert_eq!(rig.w.voices.calls.load(Ordering::SeqCst), 0);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_longest_silence_and_max_and_the_longest_id_are_accepted() {
    let rig = Rig::waiting(1, Duration::from_millis(200));
    let daemon = Running::start(rig.w.state.clone()).await;
    let body = format!(r#"{{"id":"{}","silence":"30s","max":"30s"}}"#, "a".repeat(64));
    let (status, answer) = post(daemon.port, "/voices", &body).await;
    assert_eq!(status, 408, "valid, and nobody spoke: {answer}");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_voices_script_answers_the_default_language_an_explicit_one_and_refuses_an_unknown() {
    let rig = Rig::new(0);
    let daemon = Running::start(rig.w.state.clone()).await;
    let port = daemon.port;
    let get = |path: &'static str| async move {
        let (status, text) = tokio::task::spawn_blocking(move || request(port, "GET", path))
            .await
            .unwrap();
        (status, serde_json::from_str::<Value>(&text).unwrap())
    };
    let (status, es) = get("/voices/script").await;
    assert_eq!(status, 200, "{es}");
    assert_eq!(es["lang"], "es", "the daemon's language by default");
    assert!(
        es["text"]
            .as_str()
            .unwrap()
            .starts_with("Hoy he decidido grabar mi voz")
    );
    assert_eq!(es["sample"], "Hola, así suena mi voz nueva.");
    assert_eq!(es.as_object().unwrap().len(), 3);
    let (_, explicit) = get("/voices/script?lang=es").await;
    assert_eq!(explicit, es);
    let (status, en) = get("/voices/script?lang=en").await;
    assert_eq!(status, 200);
    assert_eq!(en["lang"], "en");
    assert!(
        en["text"]
            .as_str()
            .unwrap()
            .starts_with("Today I decided to record my voice")
    );
    assert_eq!(en["sample"], "Hello, this is how my new voice sounds.");
    for bad in [
        "/voices/script?lang=xx",
        "/voices/script?lang=",
        "/voices/script?lang=ES",
    ] {
        let (status, answer) = get(bad).await;
        assert_eq!(status, 400, "{bad}: {answer}");
        assert!(answer["error"].is_string());
    }
    daemon.stop().await;
}
