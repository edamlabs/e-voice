use std::time::Duration;

use e_voice_daemon::hearing::base::Recognized;
use e_voice_daemon::schema::event::SpeechState;
use futures_util::StreamExt;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use crate::client::{Mcp, failure};
use crate::fake::{Device, FakeSpeech, Wired, heard, long, wired};
use crate::running::Running;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn subscribe(port: u16) -> Socket {
    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/events"))
        .await
        .unwrap()
        .0
}

async fn texts(socket: &mut Socket, count: usize) -> Vec<String> {
    let mut seen = Vec::new();
    while seen.len() < count {
        match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => seen.push(text.to_string()),
            Ok(Some(Ok(_))) => {}
            other => panic!("after {seen:?}: {other:?}"),
        }
    }
    seen
}

fn plain() -> Wired {
    wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_secs(5))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_http_then_mcp_speak_share_one_queue_contiguous_ids_and_one_event_stream() {
    let w = plain();
    let daemon = Running::start(w.state.clone()).await;
    let mcp = Mcp::on(daemon.port);
    let mut events = subscribe(daemon.port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (status, body) = daemon.speak(r#"{"text":"1"}"#).await;
    assert_eq!((status, body.as_str()), (200, r#"{"id":0,"outcome":"done"}"#));
    let via = mcp.call("speak", &json!({"text": "2"})).await;
    assert_eq!(via["structuredContent"], json!({"id": 1, "outcome": "done"}));
    let (_, again) = daemon.speak(r#"{"text":"3"}"#).await;
    assert_eq!(again, r#"{"id":2,"outcome":"done"}"#);
    let seen = texts(&mut events, 9).await;
    let expected: Vec<String> = (0..3)
        .flat_map(|id| {
            [
                format!(r#"{{"type":"speech","state":"speaking","id":{id}}}"#),
                format!(r#"{{"type":"speech","state":"draining","id":{id}}}"#),
                r#"{"type":"speech","state":"idle"}"#.to_owned(),
            ]
        })
        .collect();
    assert_eq!(seen, expected);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_mcp_listen_makes_http_listen_busy_and_the_reverse() {
    let w = plain();
    let daemon = Running::start(w.state.clone()).await;
    let mcp = Mcp::on(daemon.port);
    let feed = w.hearing.session();
    let asking = mcp.clone();
    let via_mcp = tokio::spawn(async move { asking.call("listen", &json!({"timeout_seconds": 10})).await });
    assert!(w.mic.live().await);
    let (status, body) = daemon.listen(Some("application/json"), Some("{}")).await;
    assert_eq!(
        (status, body.as_str()),
        (409, r#"{"error":"someone is already listening"}"#)
    );
    feed.say(Recognized::Final(heard("uno"), None));
    assert_eq!(via_mcp.await.unwrap()["structuredContent"]["text"], "uno");
    assert!(w.mic.freed().await);

    let second = w.hearing.session();
    let daemon = std::sync::Arc::new(daemon);
    let asking = std::sync::Arc::clone(&daemon);
    let via_http = tokio::spawn(async move {
        asking
            .listen(Some("application/json"), Some(r#"{"timeout":"10s"}"#))
            .await
    });
    while w.mic.opened() < 2 || !w.mic.is_open() {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(w.mic.live().await);
    let busy = mcp.call("listen", &json!({})).await;
    assert_eq!(failure(&busy), ("someone is already listening".to_owned(), 409));
    second.say(Recognized::Final(heard("dos"), None));
    let (status, _) = via_http.await.unwrap();
    assert_eq!(status, 200);
    std::sync::Arc::into_inner(daemon).unwrap().stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_over_http_cuts_an_mcp_speak_and_an_mcp_stop_cuts_an_http_speak() {
    let w = wired(long(300, 1.0), Device::new(0, 0), Duration::from_secs(5));
    let daemon = std::sync::Arc::new(Running::start(w.state.clone()).await);
    let mcp = Mcp::on(daemon.port);
    let speaker = mcp.clone();
    let spoken = tokio::spawn(async move { speaker.call("speak", &json!({"text": "x"})).await });
    while w.state.speech.state() != (SpeechState::Speaking { id: 0 }) {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(daemon.send("POST", "/stop").await.0, 200);
    assert_eq!(
        spoken.await.unwrap()["structuredContent"],
        json!({"id": 0, "outcome": "cancelled"})
    );

    let asking = std::sync::Arc::clone(&daemon);
    let http = tokio::spawn(async move { asking.speak(r#"{"text":"y"}"#).await });
    while w.state.speech.state() != (SpeechState::Speaking { id: 1 }) {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    mcp.call("stop", &json!({})).await;
    let (status, body) = http.await.unwrap();
    assert_eq!((status, body.as_str()), (200, r#"{"id":1,"outcome":"cancelled"}"#));
    std::sync::Arc::into_inner(daemon).unwrap().stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_listen_is_announced_on_the_event_stream_like_the_http_one() {
    let w = plain();
    let daemon = Running::start(w.state.clone()).await;
    let mcp = Mcp::on(daemon.port);
    let mut events = subscribe(daemon.port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let feed = w.hearing.session();
    let asking = mcp.clone();
    let call = tokio::spawn(async move { asking.call("listen", &json!({"timeout_seconds": 10})).await });
    assert!(w.mic.live().await);
    feed.say(Recognized::Partial("ho".to_owned()));
    feed.say(Recognized::Final(heard("hola"), None));
    call.await.unwrap();
    assert_eq!(
        texts(&mut events, 4).await,
        [
            r#"{"type":"listening","state":"open"}"#,
            r#"{"type":"partial","text":"ho"}"#,
            r#"{"type":"heard","text":"hola","lang":"es","emotion":{"label":"unknown","scores":{},"model":null}}"#,
            r#"{"type":"listening","state":"closed"}"#,
        ]
    );
    daemon.stop().await;
}
