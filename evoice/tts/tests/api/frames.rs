use crate::app::serve;
use crate::rawws::{BINARY, Frame, RawWs, TEXT};

const CAP: usize = 65_536;
const PATH: &str = "/v1/stream?lang=es";

/// A JSON message of exactly `size` bytes: `{"type":"close"}` padded with spaces before its closing brace.
fn close_of(size: usize) -> Vec<u8> {
    let mut text = r#"{"type":"close""#.to_owned();
    text.push_str(&" ".repeat(size - text.len() - 1));
    text.push('}');
    assert_eq!(text.len(), size);
    text.into_bytes()
}

fn brief(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .map(|frame| {
            if frame.payload.len() < 120 {
                format!("op{} {:?}", frame.opcode, frame.text())
            } else {
                format!("op{} {} bytes", frame.opcode, frame.payload.len())
            }
        })
        .collect()
}

async fn send(socket: &mut RawWs, payload: &[u8], pieces: Option<usize>) {
    let _ = match pieces {
        Some(piece) => socket.fragmented(TEXT, payload, piece).await,
        None => socket.message(TEXT, payload).await,
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn test_one_byte_over_the_cap_closes_1009_with_the_reason() {
    let app = serve(1).await;
    for pieces in [None, Some(CAP / 2)] {
        let mut socket = RawWs::connect(&app.addr.to_string(), PATH).await;
        send(&mut socket, &close_of(CAP + 1), pieces).await;
        let (frames, closed) = socket.until_close().await;
        assert!(closed, "no Close frame ({pieces:?}): {:?}", brief(&frames));
        let closing = frames.last().and_then(Frame::closing).unwrap();
        assert_eq!(closing, (1009, format!("message over {CAP} bytes")), "{pieces:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_exactly_at_the_cap_is_accepted_whole_or_fragmented() {
    let app = serve(1).await;
    for pieces in [None, Some(CAP / 2)] {
        let mut socket = RawWs::connect(&app.addr.to_string(), PATH).await;
        send(&mut socket, &close_of(CAP), pieces).await;
        let (frames, closed) = socket.until_close().await;
        assert!(closed, "{pieces:?}: {:?}", brief(&frames));
        assert_eq!(
            frames.last().and_then(Frame::closing).unwrap().0,
            1000,
            "{pieces:?}: {:?}",
            brief(&frames)
        );
        assert!(
            frames.iter().any(|frame| frame.text().contains(r#""type":"closed""#)),
            "{:?}",
            brief(&frames)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_binary_frame_is_still_refused_with_1003() {
    let app = serve(1).await;
    let mut socket = RawWs::connect(&app.addr.to_string(), PATH).await;
    socket.message(BINARY, &[0u8; 16]).await.unwrap();
    let (frames, closed) = socket.until_close().await;
    assert!(closed, "{:?}", brief(&frames));
    assert_eq!(frames.last().and_then(Frame::closing).unwrap().0, 1003);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_service_survives_an_oversize_frame() {
    let app = serve(1).await;
    let mut socket = RawWs::connect(&app.addr.to_string(), PATH).await;
    send(&mut socket, &close_of(8 << 20), None).await;
    let _ = socket.until_close().await;
    let health = reqwest::get(format!("http://{}/health", app.addr)).await.unwrap();
    assert_eq!(health.status(), 200);
}
