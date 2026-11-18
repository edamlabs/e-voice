use std::time::{Duration, Instant};

use e_voice_daemon::api::state::AppState;
use e_voice_daemon::workflow::queue::SpeechState;

use crate::fake::{Device, Ev, FakeSpeech, events, long, queue};
use crate::running::Running;

async fn daemon(speech: FakeSpeech, device: &Device) -> (Running, AppState) {
    let state = AppState {
        speech: queue(speech, device),
    };
    (Running::start(state.clone()).await, state)
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_idle_answers_null_and_zero_every_time() {
    let (daemon, _) = daemon(FakeSpeech::marked(), &Device::new(0, 0)).await;
    for _ in 0..2 {
        let (status, body) = daemon.send("POST", "/stop").await;
        assert_eq!((status, body.as_str()), (200, r#"{"stopped":null,"dropped":0}"#));
    }
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_cuts_a_speak_in_flight_and_that_speak_answers_cancelled() {
    let device = Device::new(0, 0);
    let (daemon, state) = daemon(long(50, 2.0), &device).await;
    let speaking = {
        let port = daemon.port;
        tokio::task::spawn_blocking(move || crate::http::post_json(port, "/speak", r#"{"text":"hola"}"#))
    };
    until("speaking", || state.speech.state() == (SpeechState::Speaking { id: 0 })).await;
    until("a frame", || {
        events(&device.log).iter().any(|event| matches!(event, Ev::Push { .. }))
    })
    .await;
    let (status, body) = daemon.send("POST", "/stop").await;
    assert_eq!((status, body.as_str()), (200, r#"{"stopped":0,"dropped":0}"#));
    let (status, body) = speaking.await.unwrap();
    assert_eq!((status, body.as_str()), (200, r#"{"id":0,"outcome":"cancelled"}"#));
    assert!(events(&device.log).contains(&Ev::Clear));
    let (status, body) = daemon.speak(r#"{"text":"1"}"#).await;
    assert_eq!(status, 200);
    assert!(
        body.contains(r#""outcome":"done""#) || body.contains(r#""outcome":"cancelled""#),
        "{body}"
    );
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_answers_the_waiting_speaks_cancelled_with_their_own_ids() {
    let device = Device::new(0, 0);
    let (daemon, state) = daemon(long(50, 2.0), &device).await;
    let port = daemon.port;
    let mut calls = Vec::new();
    for text in ["a", "b", "c"] {
        calls.push(tokio::task::spawn_blocking(move || {
            crate::http::post_json(port, "/speak", &format!(r#"{{"text":"{text}"}}"#))
        }));
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    until("speaking", || state.speech.state() == (SpeechState::Speaking { id: 0 })).await;
    let (status, body) = daemon.send("POST", "/stop").await;
    assert_eq!((status, body.as_str()), (200, r#"{"stopped":0,"dropped":2}"#));
    let mut answers = Vec::new();
    for call in calls {
        answers.push(call.await.unwrap());
    }
    assert!(answers.iter().all(|(status, _)| *status == 200), "{answers:?}");
    let mut bodies: Vec<String> = answers.into_iter().map(|(_, body)| body).collect();
    bodies.sort();
    assert_eq!(
        bodies,
        [
            r#"{"id":0,"outcome":"cancelled"}"#,
            r#"{"id":1,"outcome":"cancelled"}"#,
            r#"{"id":2,"outcome":"cancelled"}"#
        ]
    );
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_on_a_dead_queue_is_a_500_with_the_reason() {
    let mut fake = FakeSpeech::marked();
    fake.panic_say = true;
    let (daemon, _) = daemon(fake, &Device::new(0, 0)).await;
    assert_eq!(daemon.speak(r#"{"text":"1"}"#).await.0, 500);
    tokio::time::sleep(Duration::from_millis(60)).await;
    let (status, body) = daemon.send("POST", "/stop").await;
    daemon.stop().await;
    assert_eq!((status, body.as_str()), (500, r#"{"error":"speech queue stopped"}"#));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_takes_post_only() {
    let (daemon, _) = daemon(FakeSpeech::marked(), &Device::new(0, 0)).await;
    let (get, delete) = (daemon.get("/stop").await.0, daemon.send("DELETE", "/stop").await.0);
    daemon.stop().await;
    assert_eq!((get, delete), (405, 405));
}
