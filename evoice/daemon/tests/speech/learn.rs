use std::io::{Cursor, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use e_voice_daemon::speech::base::{Learning, SpeechError, VoiceClip};
use e_voice_daemon::speech::tts::TtsSpeech;

/// One request as the TTS service's HTTP side received it.
struct Seen {
    line: String,
    boundary: String,
    body: Vec<u8>,
}

/// A TTS service that answers every request with `reply` and keeps the whole of each.
fn service(reply: Vec<u8>) -> (String, Arc<Mutex<Vec<Seen>>>) {
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
            let head = String::from_utf8_lossy(&head).into_owned();
            let header = |name: &str| {
                head.lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix(&format!("{name}: "))
                            .map(str::to_owned)
                    })
                    .unwrap_or_default()
            };
            let length: usize = header("content-length").trim().parse().unwrap_or(0);
            let boundary = header("content-type")
                .split("boundary=")
                .nth(1)
                .unwrap_or_default()
                .trim()
                .to_owned();
            let mut body = vec![0u8; length];
            stream.read_exact(&mut body).ok();
            record.lock().unwrap().push(Seen {
                line: head.lines().next().unwrap_or_default().to_owned(),
                boundary,
                body,
            });
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

/// One multipart part: its headers' text and its bytes.
struct Part {
    head: String,
    data: Vec<u8>,
}

impl Part {
    fn name(&self) -> String {
        self.head
            .split("name=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned()
    }
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

fn parts(seen: &Seen) -> Vec<Part> {
    let delimiter = format!("--{}", seen.boundary).into_bytes();
    let body = &seen.body;
    let mut parts = Vec::new();
    let mut at = find(body, &delimiter, 0).unwrap();
    loop {
        let start = at + delimiter.len();
        if body[start..].starts_with(b"--") {
            return parts;
        }
        let head_end = find(body, b"\r\n\r\n", start).unwrap();
        let next = find(body, &delimiter, head_end).unwrap();
        parts.push(Part {
            head: String::from_utf8_lossy(&body[start..head_end]).into_owned(),
            data: body[head_end + 4..next - 2].to_vec(),
        });
        at = next;
    }
}

fn clip(id: &str, text: Option<&str>, samples: Vec<f32>) -> VoiceClip {
    VoiceClip {
        id: id.to_owned(),
        rate: NonZeroU32::new(24_000).unwrap(),
        samples,
        text: text.map(str::to_owned),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_learning_posts_voice_id_text_and_a_24_khz_mono_16_bit_wav_to_v1_voices() {
    let (url, seen) = service(http("200 OK", r#"{"voice_id":"ana"}"#));
    let samples = vec![0.0, 0.5, -0.5, 1.0, -1.0, 2.0, -2.0, 0.25];
    TtsSpeech::new(&url)
        .learn(clip("ana", Some("hola «mundo» ñ"), samples.clone()))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /v1/voices HTTP/1.1");
    let parts = parts(&seen[0]);
    assert_eq!(
        parts.iter().map(Part::name).collect::<Vec<_>>(),
        ["voice_id", "text", "file"]
    );
    assert_eq!(parts[0].data, b"ana");
    assert_eq!(String::from_utf8(parts[1].data.clone()).unwrap(), "hola «mundo» ñ");
    assert!(parts[2].head.contains("filename=\"voice.wav\""), "{}", parts[2].head);
    assert!(
        parts[2].head.to_lowercase().contains("content-type: audio/wav"),
        "{}",
        parts[2].head
    );
    let wav = hound::WavReader::new(Cursor::new(parts[2].data.clone())).unwrap();
    let spec = wav.spec();
    assert_eq!(
        (
            spec.channels,
            spec.sample_rate,
            spec.bits_per_sample,
            spec.sample_format
        ),
        (1, 24_000, 16, hound::SampleFormat::Int)
    );
    assert_eq!(wav.len() as usize, samples.len());
    assert_eq!(
        parts[2].data.len(),
        44 + 2 * samples.len(),
        "a 44-byte header and two bytes a sample"
    );
    let decoded: Vec<i16> = wav.into_samples::<i16>().map(Result::unwrap).collect();
    assert_eq!(decoded, [0, 16_384, -16_384, 32_767, -32_767, 32_767, -32_767, 8_192]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_text_field_is_sent_only_when_the_clip_has_one() {
    let (url, seen) = service(http("200 OK", "{}"));
    TtsSpeech::new(&url)
        .learn(clip("ana", None, vec![0.1; 240]))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        parts(&seen[0]).iter().map(Part::name).collect::<Vec<_>>(),
        ["voice_id", "file"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_thirty_second_clip_is_a_wav_of_exactly_1_440_044_bytes() {
    let (url, seen) = service(http("200 OK", "{}"));
    TtsSpeech::new(&url)
        .learn(clip("ana", None, vec![0.1; 720_000]))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    let file = parts(&seen[0]).pop().unwrap();
    assert_eq!(file.data.len(), 44 + 2 * 720_000);
    assert_eq!(
        hound::WavReader::new(Cursor::new(file.data)).unwrap().duration(),
        720_000
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_refusal_is_the_services_status_and_reason() {
    for (status, reason) in [
        ("400 Bad Request", "voice_id exists"),
        ("422 Unprocessable Entity", "too short"),
        ("500 Internal Server Error", "boom"),
    ] {
        let (url, _) = service(http(status, &format!(r#"{{"detail":"{reason}"}}"#)));
        let error = TtsSpeech::new(&url)
            .learn(clip("ana", None, vec![0.1; 240]))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, SpeechError::Server(text) if text.contains(reason) && text.contains(&status[..3])),
            "{error:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_service_that_is_not_there_is_a_connect_error() {
    let port = {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let error = TtsSpeech::new(format!("ws://127.0.0.1:{port}"))
        .learn(clip("ana", None, vec![0.1; 240]))
        .await
        .unwrap_err();
    assert!(matches!(error, SpeechError::Connect(_)), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_answer_over_the_cap_is_a_protocol_error_not_buffered() {
    let (url, _) = service(http("200 OK", &"x".repeat(70_000)));
    let error = TtsSpeech::new(&url)
        .learn(clip("ana", None, vec![0.1; 240]))
        .await
        .unwrap_err();
    assert!(matches!(error, SpeechError::Protocol(_)), "{error:?}");
}
