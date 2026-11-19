use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use e_voice_daemon::hearing::base::Recognized;
use e_voice_daemon::schema::event::DaemonEvent;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;

use crate::fake::{Device, FakeSpeech, Wired, heard, long, wired};
use crate::rawws::{BINARY, CLOSE, Frame, RawWs, TEXT};
use crate::running::Running;

const PING: u8 = 0x9;
const PONG: u8 = 0xA;
const LINGER: Duration = Duration::from_secs(1);
const SUBSCRIBERS: usize = e_voice_daemon::api::events::SUBSCRIBERS;

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Upgraded {
    status: u16,
    body: String,
    _stream: TcpStream,
}

fn upgrade(port: u16) -> Upgraded {
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(10)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let key = format!("{}==", "A".repeat(22));
    write!(
        stream,
        "GET /events HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        assert_eq!(stream.read(&mut byte).unwrap(), 1, "hung up during the handshake");
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let length: usize = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0u8; if status == 101 { 0 } else { length }];
    stream.read_exact(&mut body).unwrap();
    Upgraded {
        status,
        body: String::from_utf8(body).unwrap(),
        _stream: stream,
    }
}

async fn client(port: u16) -> Client {
    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/events"))
        .await
        .unwrap()
        .0
}

async fn texts(client: &mut Client, count: usize) -> Vec<String> {
    let mut seen = Vec::new();
    while seen.len() < count {
        let next = tokio::time::timeout(Duration::from_secs(5), client.next()).await;
        match next {
            Ok(Some(Ok(Message::Text(text)))) => seen.push(text.to_string()),
            Ok(Some(Ok(_))) => {}
            other => panic!("after {seen:?}: {other:?}"),
        }
    }
    seen
}

fn address(daemon: &Running) -> String {
    format!("127.0.0.1:{}", daemon.port)
}

async fn permits(w: &Wired, expected: usize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if w.state.subscribers.available_permits() == expected {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

fn speaking() -> Wired {
    wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_secs(5))
}

fn padded(index: usize, size: usize) -> DaemonEvent {
    DaemonEvent::Partial {
        text: format!("{index:08}{}", "x".repeat(size - 8)),
    }
}

fn index_of(text: &str) -> Option<usize> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value["text"].as_str()?.get(..8)?.parse().ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_subscriber_gets_the_speech_and_listen_sequences_as_the_pinned_json_text_messages() {
    let w = speaking();
    let daemon = Arc::new(Running::start(w.state.clone()).await);
    let mut socket = client(daemon.port).await;
    let (status, body) = daemon.speak(r#"{"text":"4"}"#).await;
    assert_eq!((status, body.as_str()), (200, r#"{"id":0,"outcome":"done"}"#));
    assert_eq!(
        texts(&mut socket, 3).await,
        [
            r#"{"type":"speech","state":"speaking","id":0}"#,
            r#"{"type":"speech","state":"draining","id":0}"#,
            r#"{"type":"speech","state":"idle"}"#,
        ]
    );
    let feed = w.hearing.session();
    let asking = Arc::clone(&daemon);
    let call = tokio::spawn(async move { asking.listen(None, None).await });
    assert!(w.mic.live().await);
    feed.say(Recognized::Started);
    feed.say(Recognized::Partial("ho".to_owned()));
    feed.say(Recognized::Final(heard("hola"), None));
    let (status, _) = call.await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        texts(&mut socket, 4).await,
        [
            r#"{"type":"listening","state":"open"}"#,
            r#"{"type":"partial","text":"ho"}"#,
            r#"{"type":"heard","text":"hola","lang":"es","emotion":{"label":"unknown","scores":{},"model":null}}"#,
            r#"{"type":"listening","state":"closed"}"#,
        ]
    );
    Arc::into_inner(daemon).unwrap().stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_over_http_is_seen_as_cancelling_with_the_id_speak_answered() {
    let w = wired(long(300, 1.0), Device::new(0, 0), Duration::from_secs(5));
    let daemon = Arc::new(Running::start(w.state.clone()).await);
    let mut socket = client(daemon.port).await;
    let asking = Arc::clone(&daemon);
    let speak = tokio::spawn(async move { asking.speak(r#"{"text":"x"}"#).await });
    assert_eq!(
        texts(&mut socket, 1).await,
        [r#"{"type":"speech","state":"speaking","id":0}"#]
    );
    let (status, _) = daemon.send("POST", "/stop").await;
    assert_eq!(status, 200);
    assert_eq!(
        texts(&mut socket, 2).await,
        [
            r#"{"type":"speech","state":"cancelling","id":0}"#,
            r#"{"type":"speech","state":"idle"}"#
        ]
    );
    let (_, answer) = speak.await.unwrap();
    assert!(answer.contains(r#""id":0"#) && answer.contains("cancelled"), "{answer}");
    Arc::into_inner(daemon).unwrap().stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_two_subscribers_see_the_same_sequence_and_each_alone_sees_what_happens_after_it_joins() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let mut first = client(daemon.port).await;
    daemon.speak(r#"{"text":"1"}"#).await;
    let one = texts(&mut first, 3).await;
    let mut second = client(daemon.port).await;
    daemon.speak(r#"{"text":"2"}"#).await;
    let (a, b) = (texts(&mut first, 3).await, texts(&mut second, 3).await);
    assert_eq!(a, b);
    assert_eq!(one[0], r#"{"type":"speech","state":"speaking","id":0}"#);
    assert_eq!(a[0], r#"{"type":"speech","state":"speaking","id":1}"#);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_events_from_before_the_upgrade_are_not_replayed() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    for index in 0..5 {
        w.events.send(padded(index, 16)).ok();
    }
    let mut socket = client(daemon.port).await;
    w.events.send(padded(99, 16)).unwrap();
    let first = texts(&mut socket, 1).await;
    assert_eq!(index_of(&first[0]), Some(99), "{first:?}");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_subscriber_that_falls_behind_gets_lagged_with_the_exact_gap_and_the_other_none() {
    const TOTAL: usize = 1_500;
    const SIZE: usize = 32 * 1024;
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let mut quick = client(daemon.port).await;
    let mut away = RawWs::connect_writing_first(&address(&daemon), "/events").await;
    assert!(permits(&w, SUBSCRIBERS - 2).await);
    let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress = Arc::clone(&received);
    let reading = tokio::spawn(async move {
        let mut seen = Vec::new();
        while seen.len() < TOTAL {
            match tokio::time::timeout(Duration::from_secs(30), quick.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    seen.push(text.to_string());
                    progress.store(seen.len(), std::sync::atomic::Ordering::SeqCst);
                }
                other => panic!("quick subscriber after {} events: {other:?}", seen.len()),
            }
        }
        seen
    });
    for index in 0..TOTAL {
        while index.saturating_sub(received.load(std::sync::atomic::Ordering::SeqCst)) >= 128 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        w.events.send(padded(index, SIZE)).unwrap();
    }
    let quick_seen = reading.await.unwrap();
    assert!(quick_seen.iter().all(|text| !text.contains("lagged")));
    assert_eq!(
        quick_seen
            .iter()
            .map(|text| index_of(text).unwrap())
            .collect::<Vec<_>>(),
        (0..TOTAL).collect::<Vec<_>>()
    );
    let mut previous: Option<usize> = None;
    let mut gaps = Vec::new();
    let mut last = None;
    while last != Some(TOTAL - 1) {
        let Some(frame) = away.frame_within(Duration::from_secs(4)).await else {
            break;
        };
        if frame.opcode != TEXT {
            continue;
        }
        let text = frame.text();
        if text.contains(r#""type":"lagged""#) {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 2, "{text}");
            gaps.push((previous, usize::try_from(value["missed"].as_u64().unwrap()).unwrap()));
        } else {
            let index = index_of(&text).unwrap();
            if let Some((_, missed)) = gaps.last().filter(|_| previous.is_some_and(|p| index > p + 1)) {
                assert_eq!(
                    index,
                    previous.unwrap() + 1 + missed,
                    "gap after {previous:?} claimed {missed}"
                );
            }
            previous = Some(index);
            last = Some(index);
        }
    }
    assert!(!gaps.is_empty(), "the slow subscriber never lagged");
    assert_eq!(last, Some(TOTAL - 1), "the newest event never arrived");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_seventeenth_subscriber_is_a_503_before_the_upgrade_and_a_freed_permit_is_reusable() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let mut held: Vec<Upgraded> = (0..SUBSCRIBERS).map(|_| upgrade(daemon.port)).collect();
    assert!(held.iter().all(|socket| socket.status == 101));
    assert_eq!(w.state.subscribers.available_permits(), 0);
    let refused = upgrade(daemon.port);
    assert_eq!(refused.status, 503);
    assert_eq!(refused.body, r#"{"error":"16 subscribers already"}"#);
    let again = upgrade(daemon.port);
    assert_eq!(again.status, 503);
    drop(held.pop());
    assert!(permits(&w, 1).await, "the permit of a closed socket never came back");
    let accepted = upgrade(daemon.port);
    assert_eq!(accepted.status, 101);
    assert_eq!(upgrade(daemon.port).status, 503);
    drop(held);
    drop(accepted);
    assert!(permits(&w, SUBSCRIBERS).await);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_text_or_binary_frame_from_the_client_closes_1003() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    for (opcode, payload) in [
        (TEXT, &b"hola"[..]),
        (BINARY, &[1u8, 2, 3][..]),
        (TEXT, &b""[..]),
        (BINARY, &[0u8; 1024][..]),
    ] {
        let mut socket = RawWs::connect(&address(&daemon), "/events").await;
        socket.message(opcode, payload).await.unwrap();
        let (frames, closed) = socket.until_close_within(Duration::from_secs(5)).await;
        assert!(closed, "{opcode} x {}: no close", payload.len());
        let reason = frames.last().unwrap().closing();
        assert_eq!(
            reason,
            Some((1003, "this socket only sends".to_owned())),
            "{opcode} x {}",
            payload.len()
        );
    }
    assert!(permits(&w, SUBSCRIBERS).await);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_ping_is_answered_a_pong_is_ignored_and_the_socket_stays_open() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let mut socket = RawWs::connect(&address(&daemon), "/events").await;
    socket.send(PING, true, b"p1").await.unwrap();
    socket.send(PONG, true, b"unasked").await.unwrap();
    w.events.send(padded(7, 16)).unwrap();
    let mut seen: Vec<Frame> = Vec::new();
    while seen.len() < 2 {
        let Some(frame) = socket.frame_within(Duration::from_secs(5)).await else {
            break;
        };
        seen.push(frame);
    }
    let kinds: Vec<u8> = seen.iter().map(|frame| frame.opcode).collect();
    assert!(kinds.contains(&PONG), "no pong for the ping: {kinds:?}");
    assert!(seen.iter().any(|frame| frame.opcode == PONG && frame.payload == b"p1"));
    assert!(kinds.contains(&TEXT), "the socket stopped delivering events: {kinds:?}");
    assert!(!kinds.contains(&CLOSE));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_frame_over_a_kibibyte_closes_1009_within_the_linger_for_every_shape() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let shapes: [&str; 4] = ["one frame", "fragmented", "header only", "text one frame"];
    for shape in shapes {
        for round in 0..3 {
            let mut socket = RawWs::connect_writing_first(&address(&daemon), "/events").await;
            let started = Instant::now();
            match shape {
                "one frame" => socket.message(BINARY, &[7u8; 1025]).await.unwrap(),
                "fragmented" => socket.fragmented(BINARY, &[7u8; 1200], 600).await.unwrap(),
                "header only" => socket.header_only(BINARY, 100_000).await.unwrap(),
                _ => socket.message(TEXT, &[b'a'; 2048]).await.unwrap(),
            }
            let (frames, closed) = socket.until_close_within(LINGER + Duration::from_secs(4)).await;
            assert!(closed, "{shape} round {round}: no Close arrived");
            assert_eq!(
                frames.last().unwrap().closing(),
                Some((1009, "message over 1024 bytes".to_owned())),
                "{shape} round {round}"
            );
            let waited = started.elapsed();
            assert!(waited < LINGER + Duration::from_secs(3), "{shape}: {waited:?}");
        }
    }
    assert!(permits(&w, SUBSCRIBERS).await);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_stops_reading_is_dropped_after_about_five_seconds_and_its_permit_returns() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let mut away = RawWs::connect_writing_first(&address(&daemon), "/events").await;
    let mut steady = client(daemon.port).await;
    assert!(permits(&w, SUBSCRIBERS - 2).await);
    let draining = tokio::spawn(async move {
        let mut count = 0usize;
        while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_secs(15), steady.next()).await {
            count += 1;
        }
        count
    });
    for index in 0..300 {
        w.events.send(padded(index, 64 * 1024)).unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let flooded = Instant::now();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        w.state.subscribers.available_permits(),
        SUBSCRIBERS - 2,
        "dropped before the five seconds"
    );
    assert!(permits(&w, SUBSCRIBERS - 1).await || flooded.elapsed() < Duration::from_secs(14));
    let deadline = Instant::now() + Duration::from_secs(14);
    while w.state.subscribers.available_permits() < SUBSCRIBERS - 1 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let dropped_after = flooded.elapsed();
    assert_eq!(
        w.state.subscribers.available_permits(),
        SUBSCRIBERS - 1,
        "the stalled subscriber was never dropped"
    );
    assert!(dropped_after > Duration::from_secs(3), "{dropped_after:?}");
    assert!(dropped_after < Duration::from_secs(13), "{dropped_after:?}");
    let mut last_close = None;
    while let Some(frame) = away.frame_within(Duration::from_secs(3)).await {
        if frame.opcode == CLOSE {
            last_close = frame.closing();
        }
    }
    if let Some(close) = last_close {
        assert_eq!(close, (1008, "an event waited over 5 s".to_owned()));
    }
    w.events.send(padded(999_999, 16)).ok();
    daemon.stop().await;
    drop(draining);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_closes_frees_its_permit_and_is_sent_nothing_after() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let mut socket = RawWs::connect(&address(&daemon), "/events").await;
    assert!(permits(&w, SUBSCRIBERS - 1).await);
    socket.send(CLOSE, true, &1000u16.to_be_bytes()).await.unwrap();
    assert!(
        permits(&w, SUBSCRIBERS).await,
        "the permit of a closed subscriber never came back"
    );
    for index in 0..5 {
        w.events.send(padded(index, 16)).ok();
    }
    let mut after = Vec::new();
    while let Some(frame) = socket.frame_within(Duration::from_millis(500)).await {
        after.push(frame.opcode);
    }
    assert!(!after.contains(&TEXT), "events kept coming after the Close: {after:?}");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_vanishes_without_a_close_frees_its_permit() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    let socket = upgrade(daemon.port);
    assert_eq!(socket.status, 101);
    assert!(permits(&w, SUBSCRIBERS - 1).await);
    drop(socket);
    w.events.send(padded(1, 16)).ok();
    assert!(permits(&w, SUBSCRIBERS).await);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_events_is_get_only_and_a_plain_get_is_not_an_upgrade() {
    let w = speaking();
    let daemon = Running::start(w.state.clone()).await;
    assert_eq!(daemon.send("POST", "/events").await.0, 405);
    let (status, _) = daemon.get("/events").await;
    assert!((400..500).contains(&status), "{status}");
    assert_eq!(w.state.subscribers.available_permits(), SUBSCRIBERS);
    daemon.stop().await;
}
