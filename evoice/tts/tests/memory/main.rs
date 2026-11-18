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

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn peak_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .unwrap()
}

/// A request announcing `announced` bytes of JSON body and sending up to `sent` of them as fast as it can.
async fn flood(address: std::net::SocketAddr, announced: usize, sent: usize) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let head = format!(
        "POST /v1/audio/speech HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {announced}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let answer = tokio::spawn(async move {
        let mut raw = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(20), reader.read_to_end(&mut raw)).await;
        String::from_utf8_lossy(&raw).into_owned()
    });
    let chunk = vec![b' '; 1 << 20];
    let mut written = 0;
    while written < sent {
        if writer.write_all(&chunk).await.is_err() {
            break;
        }
        written += chunk.len();
    }
    let _ = writer.shutdown().await;
    answer.await.unwrap()
}

/// The same with no announced length: a chunked body that never stops.
async fn endless(address: std::net::SocketAddr, sent: usize) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let head = "POST /v1/audio/speech HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
    stream.write_all(head.as_bytes()).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let answer = tokio::spawn(async move {
        let mut raw = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(20), reader.read_to_end(&mut raw)).await;
        String::from_utf8_lossy(&raw).into_owned()
    });
    let mut chunk = b"100000\r\n".to_vec();
    chunk.extend(std::iter::repeat_n(b' ', 1 << 20));
    chunk.extend_from_slice(b"\r\n");
    let mut written = 0;
    while written < sent {
        if writer.write_all(&chunk).await.is_err() {
            break;
        }
        written += 1 << 20;
    }
    let _ = writer.shutdown().await;
    answer.await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_body_far_over_the_limit_is_refused_before_it_is_buffered() {
    let server = app::serve(1).await;
    let warm = reqwest::get(format!("http://{}/health", server.addr)).await.unwrap();
    assert_eq!(warm.status(), 200);
    let before = peak_kib();
    let answer = flood(server.addr, 1 << 30, 256 << 20).await;
    let grown = peak_kib().saturating_sub(before);
    println!(
        "BODY_FLOOD announced 1 GiB, sent up to 256 MiB: peak grew {grown} KiB; answer starts {:?}",
        answer.lines().next()
    );
    assert!(
        answer.starts_with("HTTP/1.1 413"),
        "{}",
        answer.chars().take(200).collect::<String>()
    );
    assert!(answer.contains(r#""type":"invalid_request_error""#), "{answer}");
    assert!(grown < 50_000, "a flood of oversize body grew the peak by {grown} KiB");
    let alive = reqwest::get(format!("http://{}/health", server.addr)).await.unwrap();
    assert_eq!(alive.status(), 200);
    let before = peak_kib();
    let answer = endless(server.addr, 256 << 20).await;
    let grown = peak_kib().saturating_sub(before);
    println!(
        "BODY_ENDLESS chunked, up to 256 MiB: peak grew {grown} KiB; answer starts {:?}",
        answer.lines().next()
    );
    assert!(
        answer.starts_with("HTTP/1.1 413"),
        "{}",
        answer.chars().take(200).collect::<String>()
    );
    assert!(grown < 50_000, "an endless chunked body grew the peak by {grown} KiB");
}
