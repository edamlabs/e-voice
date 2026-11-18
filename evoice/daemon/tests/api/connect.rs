use std::time::{Duration, Instant};

use e_voice_daemon::api::state::AppState;
use e_voice_daemon::speech::tts::TtsSpeech;
use e_voice_daemon::workflow::queue::SpeechQueue;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::fake::{Device, Ev, events};
use crate::running::Running;

/// A TTS service whose first connection is accepted at TCP level and never answered, and whose later ones work:
/// ready at 24 kHz, one frame, closed.
async fn stalling_then_working() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (silent, _) = listener.accept().await.unwrap();
        let mut held = vec![silent];
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(message)) = socket.next().await {
                    if matches!(&message, Message::Text(text) if text.contains("close")) {
                        break;
                    }
                }
                let bytes: Vec<u8> = [0.5f32, 0.5].iter().flat_map(|sample| sample.to_le_bytes()).collect();
                socket
                    .send(Message::text(r#"{"type":"ready","rate":24000}"#))
                    .await
                    .unwrap();
                socket.send(Message::Binary(bytes.into())).await.unwrap();
                socket.send(Message::text(r#"{"type":"closed"}"#)).await.unwrap();
                let _ = socket.next().await;
            });
            held.retain(|_| true);
        }
    });
    url
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_service_that_never_answers_the_handshake_is_a_502_within_the_bound_and_the_queue_moves_on() {
    let url = stalling_then_working().await;
    let device = Device::new(0, 0);
    let state = AppState {
        speech: SpeechQueue::start(TtsSpeech::new(url), device.thread()),
    };
    let daemon = Running::start(state).await;
    let started = Instant::now();
    let (status, body) = daemon.speak(r#"{"text":"hola"}"#).await;
    let took = started.elapsed();
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("no handshake within 5 s"), "{body}");
    assert!(took >= Duration::from_secs(4), "answered before the bound: {took:?}");
    assert!(took < Duration::from_secs(6), "answered after the bound: {took:?}");
    let (status, body) = daemon.speak(r#"{"text":"otra"}"#).await;
    assert_eq!((status, body.as_str()), (200, r#"{"id":1,"outcome":"done"}"#));
    assert!(
        events(&device.log)
            .iter()
            .any(|event| matches!(event, Ev::Push { len: 2, .. }))
    );
    daemon.stop().await;
}
