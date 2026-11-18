use reqwest::multipart::{Form, Part};
use serde_json::Value;

use crate::app::{serve, wav};

#[tokio::test]
async fn test_voices_learn_list_and_forget() {
    let app = serve(1).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/voices", app.addr);
    let form = Form::new()
        .text("voice_id", "damien")
        .part("file", Part::bytes(wav()).file_name("a.wav"))
        .part("file", Part::bytes(wav()).file_name("b.wav"));
    let created = client.post(&url).multipart(form).send().await.unwrap();
    assert_eq!(created.status(), 201);
    let body: Value = created.json().await.unwrap();
    assert_eq!(body["voice_id"], "damien");
    assert!((body["seconds"].as_f64().unwrap() - 2.0).abs() < 0.05);
    let listed: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed["voices"], serde_json::json!(["damien"]));
    let deleted = client.delete(format!("{url}/damien")).send().await.unwrap();
    assert_eq!(deleted.status(), 204);
    let missing = client.delete(format!("{url}/damien")).send().await.unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn test_voices_reject_bad_ids_and_missing_audio() {
    let app = serve(1).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/voices", app.addr);
    let bad = Form::new()
        .text("voice_id", "Bad Id!")
        .part("file", Part::bytes(wav()).file_name("a.wav"));
    assert_eq!(client.post(&url).multipart(bad).send().await.unwrap().status(), 400);
    let empty = Form::new().text("voice_id", "ok");
    assert_eq!(client.post(&url).multipart(empty).send().await.unwrap().status(), 400);
}

fn wav_claiming(rate: u32) -> Vec<u8> {
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
        &vec![0u8; data as usize],
    ] {
        bytes.extend_from_slice(part);
    }
    bytes
}

#[tokio::test]
async fn test_voices_refuse_a_wav_with_an_absurd_rate_and_the_service_lives() {
    let app = serve(1).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/voices", app.addr);
    for rate in [u32::MAX, 384_001, 0] {
        let form = Form::new()
            .text("voice_id", "absurd")
            .part("file", Part::bytes(wav_claiming(rate)).file_name("a.wav"));
        let status = client.post(&url).multipart(form).send().await.unwrap().status();
        assert!(status.is_client_error(), "rate {rate}: {status}");
    }
    let listed = client.get(&url).send().await.unwrap();
    assert_eq!(listed.status(), 200);
    let kept: Value = listed.json().await.unwrap();
    assert_eq!(kept["voices"], serde_json::json!([]));
}

/// A mono s16 WAV of `seconds` of a steady tone at 8 kHz, so a minute fits the 1 MiB upload.
fn tone_wav(seconds: u32) -> Vec<u8> {
    let samples: Vec<i16> = (0..seconds * 8_000)
        .map(|n| ((n as f32 * 0.05).sin() * 9_000.0) as i16)
        .collect();
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    for part in [
        b"RIFF".as_slice(),
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &8_000u32.to_le_bytes(),
        &16_000u32.to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
        &samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<u8>>(),
    ] {
        bytes.extend_from_slice(part);
    }
    bytes
}

async fn learn(files: Vec<Vec<u8>>) -> (u16, Value) {
    let app = serve(1).await;
    let mut form = Form::new().text("voice_id", "long");
    for (n, bytes) in files.into_iter().enumerate() {
        form = form.part("file", Part::bytes(bytes).file_name(format!("{n}.wav")));
    }
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/voices", app.addr))
        .multipart(form)
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn test_seconds_is_what_was_learned_under_the_limit_it_is_the_sum() {
    let (status, body) = learn(vec![tone_wav(5), tone_wav(7)]).await;
    assert_eq!(status, 201, "{body}");
    assert!((body["seconds"].as_f64().unwrap() - 12.0).abs() < 0.25, "{body}");
}

#[tokio::test]
async fn test_seconds_is_what_was_learned_over_the_limit_it_is_thirty() {
    for files in [
        vec![tone_wav(45)],
        vec![tone_wav(20), tone_wav(20)],
        vec![tone_wav(20), tone_wav(20), tone_wav(20)],
    ] {
        let (status, body) = learn(files).await;
        assert_eq!(status, 201, "{body}");
        let seconds = body["seconds"].as_f64().unwrap();
        assert!(seconds <= 30.0 + 1e-9 && seconds > 29.9, "learned {seconds} s");
    }
}

#[tokio::test]
async fn test_a_clip_past_the_budget_that_is_not_audio_is_still_a_400() {
    let (status, body) = learn(vec![tone_wav(45), vec![0u8; 64]]).await;
    assert_eq!(status, 400, "{body}");
}
