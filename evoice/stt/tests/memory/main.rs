#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
//! Its own test binary: the peak resident set is per process, so this must be the only test in it.

#[allow(dead_code)]
#[path = "../api/app.rs"]
mod app;
#[allow(dead_code)]
#[path = "../workflow/fake.rs"]
mod fake;
#[allow(dead_code)]
#[path = "../../../core/tests/support/flac.rs"]
mod flac;
#[allow(dead_code)]
#[path = "../../../core/tests/support/rawws.rs"]
mod rawws;

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use tokio_tungstenite::tungstenite::Message;

use crate::app::{exchange, pcm};
use crate::fake::{batch, nodes, ser};

fn peak_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .unwrap()
}

/// A mono s16 WAV of 1,600 loud samples whose header claims `rate` Hz.
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
        &pcm(0.9, 1_600),
    ] {
        bytes.extend_from_slice(part);
    }
    bytes
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn test_refusing_absurd_rates_on_every_path_keeps_the_process_small() {
    let (address, _) = app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 28, false).await;
    let warm = reqwest::get(format!("http://{address}/health")).await.unwrap();
    assert_eq!(warm.status(), 200);
    let before = peak_kib();
    for rate in [u32::MAX, 384_001, 0] {
        let frames = || vec![Message::Binary(pcm(0.9, 1_600).into())];
        let (_, code) = exchange(&format!("ws://{address}/v1/stream?rate={rate}"), frames()).await;
        assert_eq!(code, Some(1003), "native {rate}");
        let live = format!("ws://{address}/v1/listen?encoding=linear16&sample_rate={rate}&language=en");
        let (_, code) = exchange(&live, frames()).await;
        assert_eq!(code, Some(1003), "deepgram {rate}");
        let client = reqwest::Client::new();
        let form = Form::new()
            .text("model", "whisper-1")
            .part("file", Part::bytes(wav_claiming(rate)).file_name("a.wav"));
        let refused = client
            .post(format!("http://{address}/v1/audio/transcriptions"))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400, "openai {rate}");
        let refused = client
            .post(format!("http://{address}/v1/listen?language=es"))
            .header("content-type", "audio/wav")
            .body(wav_claiming(rate))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400, "deepgram batch {rate}");
    }
    let grown = peak_kib().saturating_sub(before);
    println!("ABSURD_RATES peak grew {grown} KiB");
    assert!(grown < 100_000, "refusing absurd rates grew the peak by {grown} KiB");
    let alive = reqwest::get(format!("http://{address}/health")).await.unwrap();
    assert_eq!(alive.status(), 200);

    let capped = app::serve_within(
        nodes(batch(0), Some(ser(0)), None),
        1 << 28,
        false,
        Duration::from_secs(10),
    )
    .await
    .0;
    let before = peak_kib();
    let bomb = flac::silent_flac(10 * 3_600, 16_000);
    let client = reqwest::Client::new();
    let form = Form::new()
        .text("model", "whisper-1")
        .part("file", Part::bytes(bomb.clone()).file_name("a.flac"));
    let refused = client
        .post(format!("http://{capped}/v1/audio/transcriptions"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 413, "openai bomb");
    let refused = client
        .post(format!("http://{capped}/v1/listen?language=es"))
        .header("content-type", "audio/flac")
        .body(bomb.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 413, "deepgram bomb");
    let form = Form::new()
        .text("model_id", "scribe_v1")
        .part("file", Part::bytes(bomb).file_name("a.flac"));
    let refused = client
        .post(format!("http://{capped}/v1/speech-to-text"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 413, "scribe bomb");
    let grown = peak_kib().saturating_sub(before);
    println!("BOMB_DOORS 10 h of silence, cap 10 s, three doors: peak grew {grown} KiB");
    assert!(
        grown < 150_000,
        "three 10 h bombs with a 10 s cap grew the peak by {grown} KiB"
    );

    let (address, _) = app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 20, false).await;
    let before = peak_kib();
    let mut socket = rawws::RawWs::connect(&address, "/v1/stream").await;
    socket.message(rawws::BINARY, &vec![0u8; 2_097_152]).await.unwrap();
    socket.message(rawws::TEXT, br#"{"type":"end"}"#).await.unwrap();
    let (frames, closed) = socket.until_close().await;
    assert!(closed, "{frames:?}");
    let one = peak_kib().saturating_sub(before);
    println!("LEGAL_FRAME one 2 MiB frame on the native socket: peak grew {one} KiB");
    assert!(one < 100_000, "one legal frame grew the peak by {one} KiB");
    let before = peak_kib();
    let mut socket = rawws::RawWs::connect(&address, "/v1/stream").await;
    let flood = vec![0u8; 2_097_152];
    for _ in 0..64 {
        if socket.message(rawws::BINARY, &flood).await.is_err() {
            break;
        }
    }
    let _ = socket.message(rawws::TEXT, br#"{"type":"end"}"#).await;
    let (frames, closed) = socket.until_close().await;
    let flooded = peak_kib().saturating_sub(before);
    println!(
        "LEGAL_FLOOD 64 legal 2 MiB frames as fast as possible (128 MiB): peak grew {flooded} KiB, closed {closed}, {} frames back",
        frames.len()
    );
    assert!(
        flooded < 600_000,
        "a flood of legal frames grew the peak by {flooded} KiB"
    );
}
