use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use e_voice_daemon::schema::event::RecordState::{Failed, Heard, Started};
use e_voice_daemon::workflow::record::RecordError;
use serde_json::{Value, json};

use crate::client::Mcp;
use crate::http::post_json;
use crate::rig::Rig;
use crate::running::Running;

async fn post(port: u16, body: &str) -> (u16, String) {
    let body = body.to_owned();
    tokio::task::spawn_blocking(move || post_json(port, "/voices", &body))
        .await
        .unwrap()
}

/// A recording over HTTP that has heard the first speech.
async fn speaking(rig: &mut Rig, daemon: &Running) -> tokio::task::JoinHandle<(u16, String)> {
    let port = daemon.port;
    let request = tokio::spawn(post(port, r#"{"id":"ana","text":"hola"}"#));
    rig.live().await;
    rig.audio(0.5, 0.0).await;
    rig.started(0.5);
    rig.audio(0.5, 0.5).await;
    rig.until(Heard).await;
    request
}

fn assert_cancelled(status: u16, body: &str, rig: &Rig) {
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(body).unwrap(),
        json!({"error": "the recording was stopped"})
    );
    assert!(rig.clips().is_empty(), "a cancelled recording learned");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_post_stop_cancels_a_recording_learns_nothing_and_sends_failed() {
    let mut rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let request = speaking(&mut rig, &daemon).await;
    let (status, answer) = daemon.send("POST", "/stop").await;
    assert_eq!(status, 200, "{answer}");
    let (status, body) = request.await.unwrap();
    assert_cancelled(status, &body, &rig);
    assert_eq!(rig.until(Failed).await, [Failed]);
    assert!(rig.w.mic.freed().await);
    assert!(!rig.w.record.cancel(), "nothing runs now");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_mcp_stop_tool_cancels_a_recording_the_same_way() {
    let mut rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let request = speaking(&mut rig, &daemon).await;
    let stopped = Mcp::on(daemon.port).call("stop", &json!({})).await;
    assert_eq!(stopped["isError"], false, "{stopped}");
    let (status, body) = request.await.unwrap();
    assert_cancelled(status, &body, &rig);
    assert_eq!(rig.until(Failed).await, [Failed]);
    assert!(rig.w.mic.freed().await);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_drops_the_request_cancels_the_recording() {
    let mut rig = Rig::new(1);
    let daemon = Running::start(rig.w.state.clone()).await;
    let port = daemon.port;
    let body = r#"{"id":"ana","text":"hola"}"#;
    let socket = tokio::task::spawn_blocking(move || {
        let mut socket = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(5)).unwrap();
        write!(
            socket,
            "POST /voices HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        socket
    })
    .await
    .unwrap();
    rig.live().await;
    rig.audio(0.5, 0.0).await;
    rig.started(0.5);
    rig.audio(0.5, 0.5).await;
    rig.until(Heard).await;
    drop(socket);
    assert_eq!(
        rig.until(Failed).await,
        [Failed],
        "the dropped client left the recording running"
    );
    assert!(rig.w.mic.freed().await);
    assert!(rig.clips().is_empty());
    assert!(!rig.w.record.finish());
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancelling_a_recording_directly_is_cancelled_with_no_clip() {
    let mut rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.audio(0.5, 0.5).await;
    assert!(rig.w.record.cancel());
    assert!(matches!(rig.done(recording).await, Err(RecordError::Cancelled)));
    assert!(rig.clips().is_empty());
    assert_eq!(rig.states(), [Started, Heard, Failed]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_while_the_clip_is_drained_still_cancels() {
    let rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.audio(0.5, 0.5).await;
    assert!(rig.w.record.finish());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!recording.is_finished(), "waiting for the final");
    assert!(rig.w.record.cancel());
    assert!(matches!(rig.done(recording).await, Err(RecordError::Cancelled)));
    assert!(rig.clips().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_voice_conversations_stop_cancels_a_recording_through_the_one_stop() {
    let mut rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.audio(0.5, 0.5).await;
    rig.w.state.talk.stop().await.unwrap();
    assert!(matches!(rig.done(recording).await, Err(RecordError::Cancelled)));
    assert!(rig.clips().is_empty());
    rig.until(Failed).await;
}
