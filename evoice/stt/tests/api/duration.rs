use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde_json::Value;

use crate::app::{self, serve_within};
use crate::fake::{batch, nodes, ser};
use crate::flac::silent_flac;

const RATE: u32 = 16_000;
const DOORS: [&str; 3] = ["openai", "deepgram", "scribe"];

async fn start(upload: usize, duration: Duration) -> String {
    serve_within(nodes(batch(0), Some(ser(0)), None), upload, false, duration)
        .await
        .0
}

async fn post(address: &str, door: &str, bytes: Vec<u8>) -> (u16, Value) {
    let client = reqwest::Client::new();
    let response = match door {
        "openai" => {
            let form = Form::new()
                .text("model", "whisper-1")
                .part("file", Part::bytes(bytes).file_name("a.flac"));
            client
                .post(format!("http://{address}/v1/audio/transcriptions"))
                .multipart(form)
                .send()
                .await
        }
        "deepgram" => {
            client
                .post(format!("http://{address}/v1/listen?language=es"))
                .header("content-type", "audio/flac")
                .body(bytes)
                .send()
                .await
        }
        _ => {
            let form = Form::new()
                .text("model_id", "scribe_v1")
                .part("file", Part::bytes(bytes).file_name("a.flac"));
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
async fn test_audio_past_the_duration_is_a_413_in_each_vendors_envelope() {
    let address = start(1 << 28, Duration::from_secs(10)).await;
    let bomb = silent_flac(3_600, RATE);
    let (status, body) = post(&address, "openai", bomb.clone()).await;
    assert_eq!(status, 413, "openai: {body}");
    assert_eq!(body["error"]["type"], "invalid_request_error", "{body}");
    assert_eq!(body["error"]["param"], "file", "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("10 s"), "{body}");
    let (status, body) = post(&address, "deepgram", bomb.clone()).await;
    assert_eq!(status, 413, "deepgram: {body}");
    assert_eq!(body["err_code"], "Payload Too Large", "{body}");
    assert!(body["err_msg"].as_str().unwrap().contains("10 s"), "{body}");
    let (status, body) = post(&address, "scribe", bomb).await;
    assert_eq!(status, 413, "scribe: {body}");
    assert_eq!(body["detail"]["status"], "invalid_request", "{body}");
    let health = reqwest::get(format!("http://{address}/health")).await.unwrap();
    assert_eq!(health.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_audio_exactly_at_the_duration_is_accepted_and_one_second_less_is_refused() {
    let file = silent_flac(32, RATE);
    let exact = start(1 << 28, Duration::from_secs(32)).await;
    let short = start(1 << 28, Duration::from_secs(31)).await;
    for door in DOORS {
        assert_eq!(post(&exact, door, file.clone()).await.0, 200, "{door} at the cap");
        assert_eq!(
            post(&short, door, file.clone()).await.0,
            413,
            "{door} one second under the cap"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_default_duration_takes_a_meeting_of_an_hour_whole_on_every_door() {
    let address = app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 28, false).await.0;
    let meeting = silent_flac(3_700, RATE);
    for door in DOORS {
        let (status, body) = post(&address, door, meeting.clone()).await;
        assert_eq!(status, 200, "{door}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_bytes_past_the_upload_cap_are_a_413_in_deepgrams_envelope() {
    let address = start(1_024, Duration::from_secs(14_400)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/listen?language=es"))
        .header("content-type", "audio/wav")
        .body(vec![0u8; 4_096])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let body: Value = response
        .json()
        .await
        .expect("the 413 must be a Deepgram JSON envelope, not plain text");
    println!("DEEPGRAM_413 err_code = {:?}", body["err_code"]);
    assert_eq!(body["err_code"], "Payload Too Large", "{body}");
    assert!(body["err_msg"].is_string() && body["request_id"].is_string(), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_other_doors_still_answer_413_past_the_upload_cap() {
    let address = start(1_024, Duration::from_secs(14_400)).await;
    for door in ["openai", "scribe"] {
        assert_eq!(post(&address, door, vec![0u8; 4_096]).await.0, 413, "{door}");
    }
}
