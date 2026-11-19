use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use e_voice_daemon::hearing::base::{HearingError, Recognized};
use e_voice_daemon::workflow::listen::{ListenError, Listener};
use e_voice_device::error::DeviceError;
use serde_json::{Value, json};

use crate::fake::{Device, FakeHearing, FakeListen, FakeSpeech, Mic, heard, queue};
use crate::running::Running;

type Case = (fn() -> ListenError, u16, &'static str);

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("{error}: {body}"))
}

async fn with(listen: FakeListen) -> (Running, Arc<std::sync::Mutex<Vec<Option<Duration>>>>) {
    let calls = Arc::clone(&listen.calls);
    let state = crate::fake::state(
        queue(FakeSpeech::marked(), &Device::new(0, 0)),
        Arc::new(listen),
        crate::fake::bus(),
    );
    (Running::start(state).await, calls)
}

async fn real() -> (Running, FakeHearing, Mic) {
    let (hearing, mic) = (FakeHearing::default(), Mic::default());
    let events = crate::fake::bus();
    let state = crate::fake::state(
        queue(FakeSpeech::marked(), &Device::new(0, 0)),
        Arc::new(Listener::new(
            hearing.clone(),
            mic.thread(),
            Duration::from_millis(300),
            events.clone(),
        )),
        events,
    );
    (Running::start(state).await, hearing, mic)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listen_answers_the_heard_text_lang_and_emotion() {
    let (daemon, _) = with(FakeListen::new(|_| Ok(heard("hola")))).await;
    let (status, body) = daemon.listen(None, None).await;
    daemon.stop().await;
    assert_eq!(status, 200);
    assert_eq!(
        json(&body),
        json!({"text": "hola", "lang": "es", "emotion": {"label": "unknown", "scores": {}, "model": null}})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_body_decides_the_timeout_whatever_the_content_type() {
    let (daemon, calls) = with(FakeListen::new(|_| Ok(heard("x")))).await;
    let cases: [(Option<&'static str>, Option<&str>); 9] = [
        (None, None),
        (Some("application/json"), Some("")),
        (None, Some("")),
        (Some("application/json"), Some("{}")),
        (Some("application/json"), Some(r#"{"timeout":"2s"}"#)),
        (None, Some(r#"{"timeout":"2s"}"#)),
        (Some("text/plain"), Some(r#"{"timeout":"500ms"}"#)),
        (Some("application/x-www-form-urlencoded"), Some(r#"{"timeout":"1m"}"#)),
        (Some("application/json"), Some(r#"{"timeout":null}"#)),
    ];
    for (kind, body) in cases {
        let (status, answer) = daemon.listen(kind, body).await;
        assert_eq!(status, 200, "{kind:?} {body:?}: {answer}");
    }
    daemon.stop().await;
    let seconds = Duration::from_secs;
    assert_eq!(
        *calls.lock().unwrap(),
        [
            None,
            None,
            None,
            None,
            Some(seconds(2)),
            Some(seconds(2)),
            Some(Duration::from_millis(500)),
            Some(seconds(60)),
            None
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_body_that_is_not_a_listen_is_a_400_in_the_daemons_shape_and_nothing_is_called() {
    let (daemon, calls) = with(FakeListen::new(|_| Ok(heard("x")))).await;
    let bad = [
        r#"{"bogus":1}"#,
        "not json",
        r#"{"timeout":5}"#,
        r#"{"timeout":"soon"}"#,
        r#"{"timeout":"-1s"}"#,
        r#"{"timeout":"1s","bogus":1}"#,
        "null5",
    ];
    for body in bad {
        for kind in [Some("application/json"), None, Some("text/plain")] {
            let (status, answer) = daemon.listen(kind, Some(body)).await;
            assert_eq!(status, 400, "{kind:?} {body}: {answer}");
            let parsed = json(&answer);
            assert!(
                parsed["error"].as_str().is_some_and(|text| !text.is_empty()),
                "{answer}"
            );
            assert_eq!(parsed.as_object().unwrap().len(), 1, "{answer}");
        }
    }
    daemon.stop().await;
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_json_array_is_not_a_listen_body() {
    let (daemon, calls) = with(FakeListen::new(|_| Ok(heard("x")))).await;
    let mut seen = Vec::new();
    for body in ["[]", r#"["2s"]"#, "[null]"] {
        seen.push((body, daemon.listen(Some("application/json"), Some(body)).await.0));
    }
    daemon.stop().await;
    assert_eq!(
        seen,
        [("[]", 400), (r#"["2s"]"#, 400), ("[null]", 400)],
        "calls {:?}",
        calls.lock().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_each_listen_error_has_its_status_and_the_error_text() {
    let cases: [Case; 6] = [
        (
            || ListenError::Invalid(Duration::ZERO),
            400,
            "a listen timeout is above zero and at most 300 s, not 0 s",
        ),
        (
            || ListenError::Timeout(Duration::from_secs(8)),
            408,
            "nobody started speaking within 8 s",
        ),
        (|| ListenError::Busy, 409, "someone is already listening"),
        (
            || ListenError::Hearing(HearingError::Server("no final within 40 s of speech".to_owned())),
            502,
            "speech recognizer: no final within 40 s of speech",
        ),
        (
            || ListenError::Hearing(HearingError::Connect("refused".to_owned())),
            502,
            "cannot reach the speech recognizer: refused",
        ),
        (|| ListenError::Device(DeviceError::Input(None)), 503, "no input device"),
    ];
    for (error, status, text) in cases {
        let (daemon, _) = with(FakeListen::new(move |_| Err(error()))).await;
        let (got, body) = daemon.listen(None, None).await;
        daemon.stop().await;
        assert_eq!(got, status, "{text}: {body}");
        assert_eq!(json(&body), json!({"error": text}));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_zero_and_over_the_longest_are_400_before_the_microphone_opens() {
    let (daemon, _hearing, mic) = real().await;
    for timeout in ["0s", "0ms", "301s", "6m", "1h"] {
        let (status, body) = daemon
            .listen(Some("application/json"), Some(&format!(r#"{{"timeout":"{timeout}"}}"#)))
            .await;
        assert_eq!(status, 400, "{timeout}: {body}");
        assert!(
            json(&body)["error"]
                .as_str()
                .unwrap()
                .starts_with("a listen timeout is above zero and at most 300 s")
        );
    }
    daemon.stop().await;
    assert_eq!(mic.opened(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_silence_is_a_408_with_the_default_and_with_the_timeout_asked_and_the_microphone_is_released() {
    let (daemon, hearing, mic) = real().await;
    let _a = hearing.session();
    let at = Instant::now();
    let (status, body) = daemon.listen(None, None).await;
    assert_eq!(status, 408, "{body}");
    assert_eq!(json(&body)["error"], "nobody started speaking within 0.3 s");
    assert!(at.elapsed() >= Duration::from_millis(300));
    assert!(mic.freed().await);
    let _b = hearing.session();
    let (status, body) = daemon
        .listen(Some("application/json"), Some(r#"{"timeout":"700ms"}"#))
        .await;
    daemon.stop().await;
    assert_eq!(status, 408, "{body}");
    assert_eq!(json(&body)["error"], "nobody started speaking within 0.7 s");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_speaker_past_the_timeout_gets_a_200_over_http() {
    let (daemon, hearing, mic) = real().await;
    let feed = hearing.session();
    let daemon = Arc::new(daemon);
    let asking = Arc::clone(&daemon);
    let call = tokio::spawn(async move {
        asking
            .listen(Some("application/json"), Some(r#"{"timeout":"200ms"}"#))
            .await
    });
    assert!(mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    tokio::time::sleep(Duration::from_millis(600)).await;
    feed.say(Recognized::Final(heard("tarde"), None));
    let (status, body) = call.await.unwrap();
    Arc::into_inner(daemon).unwrap().stop().await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json(&body)["text"], "tarde");
    assert!(mic.freed().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_second_listen_while_one_runs_is_a_409_and_the_first_is_unharmed() {
    let (daemon, hearing, mic) = real().await;
    let feed = hearing.session();
    let daemon = Arc::new(daemon);
    let asking = Arc::clone(&daemon);
    let first = tokio::spawn(async move {
        asking
            .listen(Some("application/json"), Some(r#"{"timeout":"10s"}"#))
            .await
    });
    assert!(mic.live().await);
    let (status, body) = daemon.listen(None, None).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(json(&body)["error"], "someone is already listening");
    let (status, body) = daemon
        .listen(Some("application/json"), Some(r#"{"timeout":"0s"}"#))
        .await;
    assert_eq!(status, 400, "invalid is judged before busy: {body}");
    assert_eq!(mic.opened(), 1);
    feed.say(Recognized::Final(heard("uno"), None));
    let (status, body) = first.await.unwrap();
    Arc::into_inner(daemon).unwrap().stop().await;
    assert_eq!((status, json(&body)["text"].as_str().unwrap()), (200, "uno"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_no_microphone_is_a_503_and_a_dead_recognizer_a_502_and_both_are_retried_clean() {
    let (daemon, hearing, mic) = real().await;
    mic.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let (status, body) = daemon.listen(None, None).await;
    assert_eq!(
        (status, json(&body)["error"].as_str().unwrap()),
        (503, "no input device")
    );
    mic.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    *hearing.fail_open.lock().unwrap() = Some(HearingError::Connect("refused".to_owned()));
    let (status, body) = daemon.listen(None, None).await;
    assert_eq!(
        (status, json(&body)["error"].as_str().unwrap()),
        (502, "cannot reach the speech recognizer: refused")
    );
    assert!(mic.freed().await);
    *hearing.fail_open.lock().unwrap() = None;
    hearing.session().say(Recognized::Final(heard("ya"), None));
    let (status, _) = daemon.listen(None, None).await;
    daemon.stop().await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_unspoken_final_over_forty_seconds_is_not_waited_for_a_502_when_the_recognizer_fails_midway() {
    let (daemon, hearing, mic) = real().await;
    let feed = hearing.session();
    let daemon = Arc::new(daemon);
    let asking = Arc::clone(&daemon);
    let call = tokio::spawn(async move { asking.listen(None, None).await });
    assert!(mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.fail(HearingError::Server("closed with 1011".to_owned()));
    let (status, body) = call.await.unwrap();
    Arc::into_inner(daemon).unwrap().stop().await;
    assert_eq!(status, 502, "{body}");
    assert!(mic.freed().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_disconnects_releases_the_microphone() {
    let (daemon, hearing, mic) = real().await;
    let _feed = hearing.session();
    let port = daemon.port;
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let body = r#"{"timeout":"60s"}"#;
        write!(
            stream,
            "POST /listen HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
    })
    .await
    .unwrap();
    assert!(mic.freed().await, "the microphone stayed open after the client left");
    let again = hearing.session();
    again.say(Recognized::Final(heard("libre"), None));
    let (status, _) = daemon.listen(None, None).await;
    daemon.stop().await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listen_is_post_only_and_health_still_answers_while_one_waits() {
    let (daemon, hearing, mic) = real().await;
    let _feed = hearing.session();
    let daemon = Arc::new(daemon);
    assert_eq!(daemon.get("/listen").await.0, 405);
    let asking = Arc::clone(&daemon);
    let call = tokio::spawn(async move {
        asking
            .listen(Some("application/json"), Some(r#"{"timeout":"500ms"}"#))
            .await
    });
    assert!(mic.live().await);
    assert_eq!(daemon.get("/health").await.0, 200);
    assert_eq!(call.await.unwrap().0, 408);
    Arc::into_inner(daemon).unwrap().stop().await;
}
