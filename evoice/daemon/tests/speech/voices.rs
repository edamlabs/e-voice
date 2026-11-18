use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use e_voice_daemon::speech::base::{SpeechError, Voices};
use e_voice_daemon::speech::tts::TtsSpeech;

/// A TTS service's HTTP side: answers every request with `reply`, remembers each request line.
fn service(reply: Vec<u8>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0u8; 1];
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            record.lock().unwrap().push(
                String::from_utf8_lossy(&head)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            );
            stream.write_all(&reply).ok();
        }
    });
    (format!("ws://127.0.0.1:{port}"), seen)
}

fn http(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn list(url: &str) -> Result<Vec<String>, SpeechError> {
    TtsSpeech::new(url).list().await
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_ids_come_back_exact_from_get_v1_voices_on_the_ws_bases_host_and_port() {
    let (url, seen) = service(http("200 OK", r#"{"voices":["ef_dora","em_alex","ñandú-1"]}"#));
    assert_eq!(list(&url).await.unwrap(), ["ef_dora", "em_alex", "ñandú-1"]);
    assert_eq!(list(&format!("{url}/")).await.unwrap().len(), 3);
    assert_eq!(
        *seen.lock().unwrap(),
        ["GET /v1/voices HTTP/1.1", "GET /v1/voices HTTP/1.1"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_empty_list_is_an_empty_list() {
    let (url, _) = service(http("200 OK", r#"{"voices":[]}"#));
    assert_eq!(list(&url).await.unwrap(), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_body_over_64_kib_is_a_protocol_error_and_one_at_the_cap_is_read() {
    let filler = |size: usize| {
        let head = r#"{"voices":["#;
        let tail = "]}";
        let ids = format!("\"{}\"", "v".repeat(size - head.len() - tail.len() - 2));
        format!("{head}{ids}{tail}")
    };
    let over = filler(65_537);
    assert_eq!(over.len(), 65_537);
    let (url, _) = service(http("200 OK", &over));
    let error = list(&url).await.unwrap_err();
    assert_eq!(error, SpeechError::Protocol("a voice list over 65536 bytes".to_owned()));
    let exact = filler(65_536);
    let (url, _) = service(http("200 OK", &exact));
    assert_eq!(list(&url).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_body_that_never_ends_is_cut_at_the_cap_without_buffering_it_all() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = [0u8; 512];
            let _read = stream.read(&mut head).unwrap_or(0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .ok();
            let chunk = format!("{:x}\r\n{}\r\n", 16_384, "a".repeat(16_384));
            while stream.write_all(chunk.as_bytes()).is_ok() {}
        }
    });
    let started = Instant::now();
    let error = list(&url).await.unwrap_err();
    assert_eq!(error, SpeechError::Protocol("a voice list over 65536 bytes".to_owned()));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_non_2xx_is_a_server_error_carrying_the_status_and_the_body() {
    let (url, _) = service(http("500 Internal Server Error", "boom"));
    assert_eq!(
        list(&url).await.unwrap_err(),
        SpeechError::Server("500 Internal Server Error: boom".to_owned())
    );
    let (url, _) = service(http("404 Not Found", r#"{"detail":"nope"}"#));
    assert_eq!(
        list(&url).await.unwrap_err(),
        SpeechError::Server(r#"404 Not Found: {"detail":"nope"}"#.to_owned())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_2xx_that_is_not_a_voice_list_is_a_protocol_error() {
    for body in [
        "",
        "not json",
        r#"{"voices":"ef_dora"}"#,
        r#"{"names":[]}"#,
        r#"["ef_dora"]"#,
        r#"{"voices":[1,2]}"#,
    ] {
        let (url, _) = service(http("200 OK", body));
        let error = list(&url).await.unwrap_err();
        assert!(matches!(error, SpeechError::Protocol(_)), "{body}: {error:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_service_that_never_answers_is_an_error_within_five_seconds() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().flatten().collect();
        std::thread::sleep(Duration::from_secs(30));
        drop(held);
    });
    let started = Instant::now();
    let error = list(&url).await.unwrap_err();
    let took = started.elapsed();
    assert!(matches!(error, SpeechError::Connect(_)), "{error:?}");
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(6),
        "{took:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_service_that_is_not_there_or_a_base_that_is_not_a_url_is_a_connect_error() {
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let started = Instant::now();
    let refused = list(&format!("ws://127.0.0.1:{port}")).await.unwrap_err();
    assert!(matches!(refused, SpeechError::Connect(_)), "{refused:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
    let bad = list("not a url").await.unwrap_err();
    assert!(matches!(bad, SpeechError::Connect(_)), "{bad:?}");
}
