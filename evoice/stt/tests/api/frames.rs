use serde_json::Value;

use crate::app;
use crate::fake::{FakeStreaming, batch, nodes, ser};
use crate::rawws::{BINARY, Frame, RawWs, TEXT};

const AUDIO_CAP: usize = 2_097_152;
const REALTIME_CAP: usize = 16_777_216;
const MIB: usize = 1 << 20;

/// How long a server may take to answer a message it must consume whole. Measured on a debug build: a legal 16 MiB
/// realtime append takes 7.2 to 8.1 s to consume whether the process runs on 32 cores, on two, or on one under
/// `nice` (it is one thread's work), so a slower runner takes longer in proportion to its core speed, not its
/// core count. A fixed silence bound of 10 s sat just above the fast box's time; this is a total of 120 s, 15 times
/// the measurement, and a server that is stuck still fails after it.
const SERVICE: std::time::Duration = std::time::Duration::from_secs(120);

async fn batch_server() -> String {
    app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 20, false).await.0
}

async fn streaming_server() -> String {
    let streaming = e_voice_stt::workflow::asr::registry::AsrBackend::Streaming(std::sync::Arc::new(FakeStreaming));
    app::serve(nodes(streaming, Some(ser(0)), None), 1 << 20, false).await.0
}

/// A JSON message of exactly `size` bytes: `body` padded with spaces before its closing brace.
fn json_of(body: &str, size: usize) -> Vec<u8> {
    let mut text = body.trim_end_matches('}').to_owned();
    text.push_str(&" ".repeat(size - text.len() - 1));
    text.push('}');
    assert_eq!(text.len(), size);
    text.into_bytes()
}

fn append_of(size: usize) -> Vec<u8> {
    let head = r#"{"type":"input_audio_buffer.append","audio":""#;
    let audio = "A".repeat((size - head.len() - 2) / 4 * 4);
    json_of(&format!("{head}{audio}\"}}"), size)
}

/// Frames without their payloads: opcode and size, and the text of the short ones.
fn brief(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .map(|frame| {
            if frame.payload.len() < 160 {
                format!("op{} {:?}", frame.opcode, frame.text())
            } else {
                format!("op{} {} bytes", frame.opcode, frame.payload.len())
            }
        })
        .collect()
}

fn closing(frames: &[Frame]) -> (u16, String) {
    frames
        .last()
        .and_then(Frame::closing)
        .unwrap_or_else(|| panic!("no Close frame in {:?}", brief(frames)))
}

fn too_big(cap: usize) -> (u16, String) {
    (1009, format!("message over {cap} bytes"))
}

fn kinds(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter(|frame| frame.opcode == TEXT)
        .filter_map(|frame| serde_json::from_str::<Value>(&frame.text()).ok())
        .filter_map(|event| event["type"].as_str().map(str::to_owned))
        .collect()
}

async fn send(socket: &mut RawWs, opcode: u8, payload: &[u8], pieces: Option<usize>) {
    let _ = match pieces {
        Some(piece) => socket.fragmented(opcode, payload, piece).await,
        None => socket.message(opcode, payload).await,
    };
}

// ---- native /v1/stream ------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_native_one_byte_over_the_cap_closes_1009_with_the_reason() {
    let address = batch_server().await;
    for pieces in [None, Some(MIB)] {
        let mut socket = RawWs::connect(&address, "/v1/stream").await;
        send(&mut socket, BINARY, &vec![0u8; AUDIO_CAP + 1], pieces).await;
        let (frames, closed) = socket.until_close().await;
        assert!(closed, "no Close frame ({pieces:?}): {:?}", brief(&frames));
        assert_eq!(closing(&frames), too_big(AUDIO_CAP), "{pieces:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_native_exactly_at_the_cap_is_accepted_whole_or_fragmented() {
    let address = batch_server().await;
    for pieces in [None, Some(MIB)] {
        let mut socket = RawWs::connect(&address, "/v1/stream").await;
        send(&mut socket, BINARY, &vec![0u8; AUDIO_CAP], pieces).await;
        socket.message(TEXT, br#"{"type":"end"}"#).await.unwrap();
        let (frames, closed) = socket.until_close_within(SERVICE).await;
        assert!(closed, "{pieces:?}: {frames:?}");
        assert_eq!(closing(&frames).0, 1000, "{pieces:?}: {frames:?}");
        assert!(kinds(&frames).contains(&"closed".to_owned()), "{frames:?}");
    }
}

// ---- Deepgram live /v1/listen -------------------------------------------------------------------------

const LISTEN: &str = "/v1/listen?encoding=linear16&sample_rate=16000";

#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_one_byte_over_the_cap_sends_its_error_frame_then_1009() {
    let address = batch_server().await;
    for pieces in [None, Some(MIB)] {
        let mut socket = RawWs::connect(&address, LISTEN).await;
        send(&mut socket, BINARY, &vec![0u8; AUDIO_CAP + 1], pieces).await;
        let (frames, closed) = socket.until_close().await;
        assert!(closed, "no Close frame ({pieces:?}): {:?}", brief(&frames));
        assert_eq!(closing(&frames), too_big(AUDIO_CAP), "{pieces:?}");
        assert!(
            frames.len() >= 2 && frames[0].opcode == TEXT,
            "Deepgram's own text frame comes first: {frames:?}"
        );
    }
}

/// Past the cap the server sends its Close, then drops the connection once LINGER has passed, without waiting for
/// the client's reply: whatever the end (a clean EOF or a reset), it comes within LINGER and a margin.
#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_ends_the_connection_within_the_linger_after_its_close() {
    let address = batch_server().await;
    let mut socket = RawWs::connect(&address, LISTEN).await;
    socket.header_only(BINARY, AUDIO_CAP + 1).await.unwrap();
    let (frames, closed) = socket.until_close().await;
    assert!(
        closed && frames.len() >= 2 && frames[0].opcode == TEXT,
        "{:?}",
        brief(&frames)
    );
    assert_eq!(closing(&frames), too_big(AUDIO_CAP));
    let closed_at = std::time::Instant::now();
    let ending = socket.ending().await;
    let waited = closed_at.elapsed();
    assert!(ending.is_some(), "the connection never ended");
    assert!(
        waited < e_voice_stt::api::live::LINGER + std::time::Duration::from_millis(1_500),
        "ended after {waited:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_exactly_at_the_cap_is_accepted() {
    let address = batch_server().await;
    for pieces in [None, Some(MIB)] {
        let mut socket = RawWs::connect(&address, LISTEN).await;
        send(&mut socket, BINARY, &vec![0u8; AUDIO_CAP], pieces).await;
        socket.message(TEXT, br#"{"type":"CloseStream"}"#).await.unwrap();
        let (frames, closed) = socket.until_close_within(SERVICE).await;
        assert!(closed, "{pieces:?}: {frames:?}");
        assert_eq!(closing(&frames).0, 1000, "{pieces:?}: {frames:?}");
    }
}

// ---- OpenAI realtime ----------------------------------------------------------------------------------

const REALTIME: &str = "/v1/realtime?intent=transcription";

async fn realtime_over(pieces: Option<usize>) {
    let address = streaming_server().await;
    let mut socket = RawWs::connect(&address, REALTIME).await;
    send(&mut socket, TEXT, &append_of(REALTIME_CAP + 1), pieces).await;
    let (frames, closed) = socket.until_close().await;
    println!(
        "REALTIME {pieces:?}: {:?}, tcp ended: {:?}",
        brief(&frames),
        socket.ending().await
    );
    assert!(closed, "no Close frame ({pieces:?}): {:?}", brief(&frames));
    assert_eq!(closing(&frames), too_big(REALTIME_CAP), "{pieces:?}");
    let errors: Vec<String> = frames
        .iter()
        .filter(|frame| frame.opcode == TEXT)
        .map(Frame::text)
        .collect();
    assert!(
        errors.iter().any(|text| text.contains("stream closed with code 1009")),
        "{errors:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_realtime_one_byte_over_the_cap_in_one_frame_sends_its_error_event_then_1009() {
    realtime_over(None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_realtime_one_byte_over_the_cap_in_two_fragments_sends_its_error_event_then_1009() {
    realtime_over(Some(8 * MIB)).await;
}

/// Splits the TCP-reset hypothesis: only the header of the oversize frame is sent, so nothing is left unread.
#[tokio::test(flavor = "multi_thread")]
async fn test_realtime_a_header_announcing_too_much_gets_its_1009_with_nothing_unread() {
    let address = streaming_server().await;
    let mut socket = RawWs::connect(&address, REALTIME).await;
    socket.header_only(TEXT, REALTIME_CAP + 1).await.unwrap();
    let (frames, closed) = socket.until_close().await;
    println!(
        "REALTIME header-only: {:?}, tcp ended: {:?}",
        brief(&frames),
        socket.ending().await
    );
    assert!(closed, "no Close frame (header only): {:?}", brief(&frames));
    assert_eq!(closing(&frames), too_big(REALTIME_CAP));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_realtime_exactly_at_the_cap_is_accepted() {
    let address = streaming_server().await;
    for pieces in [None, Some(8 * MIB)] {
        let mut socket = RawWs::connect(&address, REALTIME).await;
        send(&mut socket, TEXT, &append_of(REALTIME_CAP), pieces).await;
        socket.message(TEXT, br#"{"type":"session.close"}"#).await.unwrap();
        let (frames, closed) = socket.until_close_within(SERVICE).await;
        assert!(closed, "{pieces:?}: {frames:?}");
        assert_eq!(closing(&frames).0, 1000, "{pieces:?}: {frames:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_services_survive_every_oversize_frame() {
    let address = batch_server().await;
    for path in ["/v1/stream", LISTEN] {
        let mut socket = RawWs::connect(&address, path).await;
        send(&mut socket, BINARY, &vec![0u8; 8 * MIB], None).await;
        let _ = socket.until_close().await;
    }
    let health = reqwest::get(format!("http://{address}/health")).await.unwrap();
    assert_eq!(health.status(), 200);
}

/// The linger belongs to 1009 alone: a socket closed normally does not wait.
#[tokio::test(flavor = "multi_thread")]
async fn test_a_normal_close_does_not_linger_on_any_socket() {
    let batch = batch_server().await;
    let streaming = streaming_server().await;
    for (name, address, path, end) in [
        ("native", &batch, "/v1/stream", r#"{"type":"end"}"#),
        ("deepgram", &batch, LISTEN, r#"{"type":"CloseStream"}"#),
        ("realtime", &streaming, REALTIME, r#"{"type":"session.close"}"#),
    ] {
        let started = std::time::Instant::now();
        let mut socket = RawWs::connect(address, path).await;
        socket.message(TEXT, end.as_bytes()).await.unwrap();
        let (frames, closed) = socket.until_close().await;
        let took = started.elapsed();
        assert!(closed && closing(&frames).0 == 1000, "{name}: {:?}", brief(&frames));
        assert!(
            took < std::time::Duration::from_millis(500),
            "{name} took {took:?} to close normally"
        );
    }
}
