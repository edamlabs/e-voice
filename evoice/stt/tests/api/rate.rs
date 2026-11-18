use reqwest::multipart::{Form, Part};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use crate::app::{self, exchange, pcm};
use crate::fake::{batch, nodes, ser};

/// Sample rates the services must refuse: beyond the bound, and zero.
const ABSURD: [u32; 3] = [u32::MAX, 384_001, 0];

async fn address() -> String {
    app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 28, false).await.0
}

async fn alive(address: &str) {
    let health = reqwest::get(format!("http://{address}/health")).await.unwrap();
    assert_eq!(health.status(), 200, "the service died");
}

/// A mono s16 WAV of 1,600 loud samples whose header claims `rate` Hz, whatever the audio really is.
fn wav(rate: u32, _parts: &[(f32, u32)]) -> Vec<u8> {
    let data = 3_200u32;
    let mut bytes = Vec::new();
    for part in [
        b"RIFF".as_slice(),
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &rate.wrapping_mul(2).to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
        &pcm(0.9, 1_600),
    ] {
        bytes.extend_from_slice(part);
    }
    bytes
}

fn flat() -> Vec<Message> {
    vec![
        Message::Binary(pcm(0.9, 1_600).into()),
        Message::Text(r#"{"type":"end"}"#.into()),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn test_native_stream_refuses_an_absurd_rate_with_1003_and_lives() {
    let address = address().await;
    for rate in ABSURD {
        let (texts, code) = exchange(&format!("ws://{address}/v1/stream?rate={rate}"), flat()).await;
        assert_eq!((texts.len(), code), (0, Some(1003)), "rate {rate}");
    }
    alive(&address).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_native_stream_accepts_the_bound() {
    let address = address().await;
    let (_, code) = exchange(&format!("ws://{address}/v1/stream?rate=384000"), flat()).await;
    assert_eq!(code, Some(1000));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_live_refuses_an_absurd_rate_with_1003_and_lives() {
    let address = address().await;
    for rate in ABSURD {
        let url = format!("ws://{address}/v1/listen?encoding=linear16&sample_rate={rate}&language=en");
        let (_, code) = exchange(&url, vec![Message::Binary(pcm(0.9, 1_600).into())]).await;
        assert_eq!(code, Some(1003), "rate {rate}");
    }
    alive(&address).await;
}

async fn batch_status(address: &str, door: &str, rate: u32) -> (u16, Value) {
    let file = wav(rate, &[(0.9, 1_600)]);
    let client = reqwest::Client::new();
    let response = match door {
        "openai" => {
            let form = Form::new()
                .text("model", "whisper-1")
                .part("file", Part::bytes(file).file_name("a.wav"));
            client
                .post(format!("http://{address}/v1/audio/transcriptions"))
                .multipart(form)
                .send()
                .await
        }
        "deepgram" => {
            client
                .post(format!("http://{address}/v1/listen?language=es"))
                .header("content-type", "audio/wav")
                .body(file)
                .send()
                .await
        }
        _ => {
            let form = Form::new()
                .text("model_id", "scribe_v1")
                .part("file", Part::bytes(file).file_name("a.wav"));
            client
                .post(format!("http://{address}/v1/speech-to-text"))
                .multipart(form)
                .send()
                .await
        }
    }
    .unwrap();
    (response.status().as_u16(), response.json().await.unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_every_batch_door_refuses_a_wav_header_with_an_absurd_rate_in_its_own_shape() {
    let address = address().await;
    for rate in [u32::MAX, 384_001, 0] {
        let (status, body) = batch_status(&address, "openai", rate).await;
        assert_eq!(status, 400, "openai {rate}: {body}");
        assert_eq!(body["error"]["type"], "invalid_request_error", "{body}");
        assert_eq!(body["error"]["param"], "file", "{body}");
        let (status, body) = batch_status(&address, "deepgram", rate).await;
        assert_eq!(status, 400, "deepgram {rate}: {body}");
        assert_eq!(body["err_code"], "Bad Request", "{body}");
        let (status, body) = batch_status(&address, "scribe", rate).await;
        assert_eq!(status, 400, "scribe {rate}: {body}");
        assert_eq!(body["detail"]["status"], "invalid_request", "{body}");
    }
    alive(&address).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_every_batch_door_takes_a_wav_at_the_bound() {
    let address = address().await;
    for door in ["openai", "deepgram", "scribe"] {
        let (status, body) = batch_status(&address, door, 384_000).await;
        assert_eq!(status, 200, "{door}: {body}");
    }
}
