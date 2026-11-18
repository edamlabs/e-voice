use serde_json::json;

use crate::app::serve;
use crate::fake::{FRAME, PACE};

#[tokio::test]
async fn test_speech_streams_a_wav_by_default() {
    let app = serve(1).await;
    let input = "Hola mundo, esto es una prueba.";
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/audio/speech", app.addr))
        .json(&json!({ "model": "tts-1", "input": input, "voice": "alloy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "audio/wav");
    let body = response.bytes().await.unwrap();
    assert_eq!(&body[..4], b"RIFF");
    let frames = input.chars().count().div_ceil(PACE);
    assert_eq!(body.len(), 44 + frames * FRAME * 2);
}

#[tokio::test]
async fn test_speech_sse_sends_deltas_then_done() {
    let app = serve(1).await;
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/audio/speech", app.addr))
        .json(&json!({ "model": "tts-1", "input": "Hola.", "voice": "alloy", "response_format": "pcm", "stream_format": "sse" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = response.text().await.unwrap();
    let events: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert!(events.len() >= 2);
    assert!(
        events[..events.len() - 1]
            .iter()
            .all(|event| event["type"] == "speech.audio.delta")
    );
    assert_eq!(events.last().unwrap()["type"], "speech.audio.done");
}

#[tokio::test]
async fn test_speech_rejects_compressed_formats_and_empty_input() {
    let app = serve(1).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", app.addr);
    let mp3 = client
        .post(&url)
        .json(&json!({ "model": "tts-1", "input": "Hola.", "voice": "alloy", "response_format": "mp3" }))
        .send()
        .await
        .unwrap();
    assert_eq!(mp3.status(), 400);
    let body: serde_json::Value = mp3.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    let empty = client
        .post(&url)
        .json(&json!({ "model": "tts-1", "input": "  ", "voice": "alloy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), 400);
}

#[tokio::test]
async fn test_health_and_models_describe_the_service() {
    let app = serve(1).await;
    let client = reqwest::Client::new();
    let health: serde_json::Value = client
        .get(format!("http://{}/health", app.addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (health["status"].as_str(), health["rate"].as_u64()),
        (Some("ok"), Some(24_000))
    );
    let models: serde_json::Value = client
        .get(format!("http://{}/v1/models", app.addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "fake-es");
    let docs = client
        .get(format!("http://{}/openapi.json", app.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(docs.status(), 200);
}

const SPEECH_LIMIT: usize = 4_096;
const BODY_LIMIT: usize = 65_536;

async fn speech(body: Vec<u8>, content_type: &str) -> (u16, Vec<u8>, Option<String>) {
    let app = serve(1).await;
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/audio/speech", app.addr))
        .header("content-type", content_type)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (status, response.bytes().await.unwrap().to_vec(), kind)
}

fn input_of(text: &str) -> Vec<u8> {
    json!({ "model": "tts-1", "input": text, "voice": "alloy" })
        .to_string()
        .into_bytes()
}

/// The door's own error body: `{"error":{"message":…,"type":…}}` as JSON, whatever the status.
fn enveloped(status: u16, body: &[u8], kind: Option<&str>) -> serde_json::Value {
    assert!(
        kind.is_some_and(|kind| kind.starts_with("application/json")),
        "{status}: content-type {kind:?}, body {}",
        String::from_utf8_lossy(body)
    );
    let parsed: serde_json::Value = serde_json::from_slice(body)
        .unwrap_or_else(|error| panic!("{status}: not JSON ({error}): {}", String::from_utf8_lossy(body)));
    assert!(
        parsed["error"]["message"].is_string() && parsed["error"]["type"].is_string(),
        "{status}: {parsed}"
    );
    parsed
}

#[tokio::test]
async fn test_speech_takes_4096_characters_not_bytes_and_refuses_one_more() {
    for letter in ["a", "ñ", "語"] {
        let (status, body, _) = speech(input_of(&letter.repeat(SPEECH_LIMIT)), "application/json").await;
        assert_eq!(
            status,
            200,
            "{SPEECH_LIMIT} × {letter:?}: {}",
            String::from_utf8_lossy(&body[..body.len().min(200)])
        );
        let (status, body, kind) = speech(input_of(&letter.repeat(SPEECH_LIMIT + 1)), "application/json").await;
        assert_eq!(status, 400, "{} × {letter:?}", SPEECH_LIMIT + 1);
        let parsed = enveloped(status, &body, kind.as_deref());
        assert_eq!(parsed["error"]["type"], "invalid_request_error");
        assert!(
            parsed["error"]["message"].as_str().unwrap().contains("4096"),
            "{parsed}"
        );
    }
}

#[tokio::test]
async fn test_speech_body_over_64_kib_is_a_413_in_the_doors_envelope() {
    let mut body = input_of("Hola.");
    body.pop();
    body.extend(std::iter::repeat_n(b' ', BODY_LIMIT - body.len() - 1));
    body.push(b'}');
    assert_eq!(body.len(), BODY_LIMIT);
    let (status, answer, _) = speech(body.clone(), "application/json").await;
    assert_eq!(
        status,
        200,
        "exactly 64 KiB is taken: {}",
        String::from_utf8_lossy(&answer[..answer.len().min(200)])
    );
    body.insert(body.len() - 1, b' ');
    let (status, answer, kind) = speech(body, "application/json").await;
    assert_eq!(status, 413);
    let parsed = enveloped(status, &answer, kind.as_deref());
    assert_eq!(parsed["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn test_speech_malformed_and_invalid_json_come_back_in_the_same_envelope_with_their_own_status() {
    let cases: [(&[u8], &str, u16); 5] = [
        (b"{not json", "application/json", 400),
        (br#"{"model":"tts-1","voice":"alloy"}"#, "application/json", 422),
        (
            br#"{"model":"tts-1","input":7,"voice":"alloy"}"#,
            "application/json",
            422,
        ),
        (
            br#"{"model":"tts-1","input":"Hola.","voice":"alloy"}"#,
            "text/plain",
            415,
        ),
        (b"", "application/json", 400),
    ];
    for (body, content_type, expected) in cases {
        let (status, answer, kind) = speech(body.to_vec(), content_type).await;
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(body));
        let parsed = enveloped(status, &answer, kind.as_deref());
        assert_eq!(parsed["error"]["type"], "invalid_request_error", "{parsed}");
    }
}

#[tokio::test]
async fn test_speech_errors_that_were_already_enveloped_are_byte_identical_to_main() {
    let (status, body, _) = speech(
        json!({ "model": "tts-1", "input": "Hola.", "voice": "alloy", "response_format": "mp3" })
            .to_string()
            .into_bytes(),
        "application/json",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"{"error":{"message":"unsupported response_format \"mp3\"; use wav, pcm or f32","type":"invalid_request_error"}}"#
    );
    let (status, body, _) = speech(input_of("  "), "application/json").await;
    assert_eq!(status, 400);
    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"{"error":{"message":"input is empty","type":"invalid_request_error"}}"#
    );
}

#[tokio::test]
async fn test_speech_errors_of_the_language_and_of_a_busy_backend_keep_their_exact_bytes() {
    let (status, body, _) = speech(
        json!({ "model": "tts-1", "input": "Hola.", "voice": "alloy", "language": "fr" })
            .to_string()
            .into_bytes(),
        "application/json",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"{"error":{"message":"unsupported language \"fr\"; supported: es, en","type":"invalid_request_error"}}"#
    );
    let app = crate::app::serve_synth(std::sync::Arc::new(crate::fake::FakeBusy)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/audio/speech", app.addr))
        .json(&json!({ "model": "tts-1", "input": "Hola.", "voice": "alloy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.text().await.unwrap(),
        r#"{"error":{"message":"every fake worker is busy","type":"server_error"}}"#
    );
}
