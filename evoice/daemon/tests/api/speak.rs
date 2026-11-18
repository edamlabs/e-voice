use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::api::state::AppState;
use e_voice_daemon::speech::base::SpeechError;
use e_voice_daemon::workflow::queue::{BACKLOG, SpeechState};
use serde_json::Value;

use crate::fake::{Device, Ev, FakeSpeech, events, frame, queue, same, say};
use crate::running::Running;

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body}"))
}

async fn daemon(speech: FakeSpeech, device: &Device) -> (Running, AppState) {
    let state = AppState {
        speech: queue(speech, device),
        listen: crate::fake::idle(),
    };
    (Running::start(state.clone()).await, state)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_answers_done_with_an_id_once_the_device_has_played() {
    let device = Device::new(2, 0);
    let (daemon, _) = daemon(FakeSpeech::marked(), &device).await;
    let started = Instant::now();
    let (status, body) = daemon.speak(r#"{"text":"4"}"#).await;
    let took = started.elapsed();
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"id":0,"outcome":"done"}"#);
    let log = events(&device.log);
    let last_push = log.iter().rposition(|event| matches!(event, Ev::Push { .. })).unwrap();
    assert!(took >= Duration::from_millis(20), "{took:?}");
    assert!(
        log[last_push..].contains(&Ev::Playing),
        "the device was already empty when the say resolved: {log:?}"
    );
    assert_eq!(log.last(), Some(&Ev::Silent));
    let (_, second) = daemon.speak(r#"{"text":"5"}"#).await;
    assert_eq!(second, r#"{"id":1,"outcome":"done"}"#);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_forwards_voice_and_lang() {
    let device = Device::new(0, 0);
    let fake = FakeSpeech::marked();
    let seen = fake.requests.clone();
    let (daemon, _) = daemon(fake, &device).await;
    let (status, _) = daemon.speak(r#"{"text":"1","voice":"ef_dora","lang":"es"}"#).await;
    daemon.stop().await;
    assert_eq!(status, 200);
    let request = seen.lock().unwrap()[0].clone();
    assert_eq!(request.voice.as_deref(), Some("ef_dora"));
    assert_eq!(request.lang.map(Lang::code), Some("es"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_refuses_empty_text_with_400() {
    let (daemon, _) = daemon(FakeSpeech::marked(), &Device::new(0, 0)).await;
    for body in [r#"{"text":""}"#, r#"{"text":"   "}"#] {
        let (status, answer) = daemon.speak(body).await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(answer, r#"{"error":"text is empty","outcome":"failed"}"#);
    }
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_rejects_a_malformed_body_at_the_door() {
    let device = Device::new(0, 0);
    let fake = FakeSpeech::marked();
    let (daemon, _) = daemon(fake.clone(), &device).await;
    let cases = [
        ("{not json", 400),
        (r#"{"voice":"x"}"#, 422),
        (r#"{"text":"1","speed":2}"#, 422),
        (r#"{"text":"1","lang":"fr"}"#, 422),
        (r#"{"text":7}"#, 422),
        ("", 400),
    ];
    for (body, expected) in cases {
        let (status, _) = daemon.speak(body).await;
        assert_eq!(status, expected, "{body}");
    }
    daemon.stop().await;
    assert_eq!(fake.texts(), Vec::<String>::new());
    assert_eq!(device.opened(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_maps_a_speech_failure_to_502() {
    let mut fake = FakeSpeech::marked();
    fake.fail_say = Some(SpeechError::Connect("refused".to_owned()));
    let (daemon, _) = daemon(fake, &Device::new(0, 0)).await;
    let (status, body) = daemon.speak(r#"{"text":"1"}"#).await;
    daemon.stop().await;
    assert_eq!(status, 502);
    let body = json(&body);
    assert_eq!(body["outcome"], "failed");
    assert_eq!(body["error"], "cannot reach the speech service: refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_maps_a_failure_midway_to_502_and_the_audio_before_it_still_plays() {
    let device = Device::new(1, 0);
    let fake = FakeSpeech::new(|_| vec![Ok(frame(8.0, 4)), Err(SpeechError::Server("cut".to_owned()))]);
    let (daemon, _) = daemon(fake, &device).await;
    let (status, body) = daemon.speak(r#"{"text":"x"}"#).await;
    daemon.stop().await;
    assert_eq!(status, 502);
    assert_eq!(json(&body)["error"], "speech service: cut");
    let log = events(&device.log);
    let first_push = log.iter().find(|event| matches!(event, Ev::Push { .. }));
    assert!(
        matches!(first_push, Some(Ev::Push { first, .. }) if same(*first, 8.0)),
        "{log:?}"
    );
    assert_eq!(log.last(), Some(&Ev::Silent));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_maps_a_missing_device_to_503_and_the_next_call_retries() {
    let device = Device::new(0, 1);
    let (daemon, _) = daemon(FakeSpeech::marked(), &device).await;
    assert_eq!(daemon.get("/health").await.0, 200);
    assert_eq!(device.opened(), 0);
    let (status, body) = daemon.speak(r#"{"text":"1"}"#).await;
    assert_eq!(status, 503);
    assert_eq!(body, r#"{"error":"no output device","outcome":"failed"}"#);
    let (status, body) = daemon.speak(r#"{"text":"2"}"#).await;
    assert_eq!((status, body.as_str()), (200, r#"{"id":1,"outcome":"done"}"#));
    assert_eq!(device.opened(), 2);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_maps_a_dead_queue_to_500() {
    let mut fake = FakeSpeech::marked();
    fake.panic_say = true;
    let (daemon, _) = daemon(fake, &Device::new(0, 0)).await;
    let (status, body) = daemon.speak(r#"{"text":"1"}"#).await;
    let (again, _) = daemon.speak(r#"{"text":"2"}"#).await;
    daemon.stop().await;
    assert_eq!((status, again), (500, 500));
    assert_eq!(json(&body)["error"], "speech queue stopped");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_answers_429_when_the_queue_is_full_and_the_rest_finish() {
    let device = Device::new(0, 0);
    let (fake, gate) = FakeSpeech::marked().gated();
    let (daemon, state) = daemon(fake.clone(), &device).await;
    let mut held = Vec::new();
    for n in 0..=BACKLOG {
        let speech = state.speech.clone();
        held.push(tokio::spawn(async move { speech.say(say(&n.to_string())).await }));
        if n == 0 {
            while state.speech.state() != (SpeechState::Speaking { id: 0 }) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (status, body) = daemon.speak(r#"{"text":"late"}"#).await;
    assert_eq!(status, 429);
    assert_eq!(
        body,
        format!(r#"{{"error":"speech queue is full: {BACKLOG} requests already waiting","outcome":"failed"}}"#)
    );
    gate.add_permits(BACKLOG + 1);
    for task in held {
        task.await.unwrap().unwrap();
    }
    gate.add_permits(1);
    let (status, body) = daemon.speak(r#"{"text":"after"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body, format!(r#"{{"id":{},"outcome":"done"}}"#, BACKLOG + 1));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_hangs_up_once_accepted_does_not_stop_the_speech() {
    use std::io::Write;
    let device = Device::new(1, 0);
    let (fake, gate) = FakeSpeech::marked().gated();
    let (daemon, state) = daemon(fake, &device).await;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", daemon.port)).unwrap();
    let body = r#"{"text":"9"}"#;
    write!(
        stream,
        "POST /speak HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.speech.state() != (SpeechState::Speaking { id: 0 }) {
        assert!(Instant::now() < deadline, "the request was never accepted");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    drop(stream);
    tokio::time::sleep(Duration::from_millis(100)).await;
    gate.add_permits(1);
    while events(&device.log)
        .iter()
        .filter(|event| matches!(event, Ev::Push { .. }))
        .count()
        < 3
    {
        assert!(Instant::now() < deadline, "the speech never played");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_refuses_a_text_over_4096_characters_with_400_and_takes_4096() {
    let (daemon, _) = daemon(FakeSpeech::marked(), &Device::new(0, 0)).await;
    for letter in ["a", "ñ"] {
        let (status, body) = daemon.speak(&format!(r#"{{"text":"{}"}}"#, letter.repeat(4_096))).await;
        assert_eq!(status, 200, "4096 × {letter}: {body}");
        let (status, body) = daemon.speak(&format!(r#"{{"text":"{}"}}"#, letter.repeat(4_097))).await;
        assert_eq!(status, 400, "4097 × {letter}");
        assert_eq!(
            body,
            r#"{"error":"text is too long: 4097 characters, at most 4096","outcome":"failed"}"#
        );
    }
    daemon.stop().await;
}
