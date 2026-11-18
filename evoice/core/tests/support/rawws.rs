#![allow(
    clippy::cast_possible_truncation,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic
)]
//! A WebSocket client written by hand, so a test can send what a library client will not: one message in
//! several frames, a frame header announcing more than follows, and read the server's frames as they are.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;

pub const TEXT: u8 = 0x1;
pub const BINARY: u8 = 0x2;
pub const CONTINUATION: u8 = 0x0;
pub const CLOSE: u8 = 0x8;

const MASK: [u8; 4] = [0x11, 0x22, 0x33, 0x44];

pub struct RawWs {
    lazy: Option<Box<dyn FnOnce() + Send>>,
    writer: OwnedWriteHalf,
    frames: mpsc::UnboundedReceiver<Frame>,
    ended: mpsc::UnboundedReceiver<String>,
}

/// One frame as the server sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub opcode: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    /// A close frame's code and reason.
    pub fn closing(&self) -> Option<(u16, String)> {
        (self.opcode == CLOSE && self.payload.len() >= 2).then(|| {
            (
                u16::from_be_bytes([self.payload[0], self.payload[1]]),
                String::from_utf8_lossy(&self.payload[2..]).into_owned(),
            )
        })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.payload).into_owned()
    }
}

impl RawWs {
    /// The handshake's 16-byte nonce, base64, made fresh from the process id, a counter and the clock.
    fn nonce() -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&std::process::id().to_be_bytes());
        bytes[4..8].copy_from_slice(&nanos.to_be_bytes());
        bytes[8..].copy_from_slice(&count.to_be_bytes());
        let alphabet: Vec<char> = ('A'..='Z')
            .chain('a'..='z')
            .chain('0'..='9')
            .chain(['+', '/'])
            .collect();
        let mut text = String::new();
        for group in bytes.chunks(3) {
            let word = group.iter().fold(0u32, |word, byte| (word << 8) | u32::from(*byte)) << (8 * (3 - group.len()));
            for index in 0..=group.len() {
                text.push(alphabet[((word >> (18 - 6 * index)) & 0x3F) as usize]);
            }
        }
        text.push_str("==");
        text
    }

    /// A client that reads while it writes.
    pub async fn connect(address: &str, path: &str) -> Self {
        Self::open(address, path, true).await
    }

    /// A client that writes everything it means to send before it reads anything.
    pub async fn connect_writing_first(address: &str, path: &str) -> Self {
        Self::open(address, path, false).await
    }

    async fn open(address: &str, path: &str, eager: bool) -> Self {
        let mut stream = TcpStream::connect(address).await.unwrap();
        let key = Self::nonce();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let mut byte = [0u8; 1];
            assert_eq!(
                stream.read(&mut byte).await.unwrap(),
                1,
                "the server hung up during the handshake"
            );
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let (reader, writer) = stream.into_split();
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let (ended_tx, ended) = mpsc::unbounded_channel();
        let pump = move |mut reader: tokio::net::tcp::OwnedReadHalf| async move {
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = vec![0u8; 65_536];
            loop {
                while let Some(frame) = Self::parse(&mut buffer) {
                    if frames_tx.send(frame).is_err() {
                        return;
                    }
                }
                match reader.read(&mut chunk).await {
                    Ok(0) => {
                        ended_tx.send("EOF".to_owned()).ok();
                        break;
                    }
                    Err(error) => {
                        ended_tx.send(format!("{:?}: {error}", error.kind())).ok();
                        break;
                    }
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                }
            }
        };
        let start: Box<dyn FnOnce() + Send> = Box::new(move || {
            tokio::spawn(pump(reader));
        });
        let lazy = if eager {
            start();
            None
        } else {
            Some(start)
        };
        Self {
            lazy,
            writer,
            frames,
            ended,
        }
    }

    /// One masked frame; an error means the server already hung up.
    pub async fn send(&mut self, opcode: u8, fin: bool, payload: &[u8]) -> io::Result<()> {
        let mut frame = vec![(u8::from(fin) << 7) | opcode];
        match payload.len() {
            0..=125 => frame.push(0x80 | payload.len() as u8),
            126..=65_535 => {
                frame.push(0x80 | 0x7E);
                frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            _ => {
                frame.push(0x80 | 0x7F);
                frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&MASK);
        frame.extend(payload.iter().enumerate().map(|(index, byte)| byte ^ MASK[index % 4]));
        self.writer.write_all(&frame).await
    }

    /// Only the header of a frame announcing `length` bytes of payload, none of which follows.
    pub async fn header_only(&mut self, opcode: u8, length: usize) -> io::Result<()> {
        let mut frame = vec![0x80 | opcode, 0x80 | 0x7F];
        frame.extend_from_slice(&(length as u64).to_be_bytes());
        frame.extend_from_slice(&MASK);
        self.writer.write_all(&frame).await
    }

    /// How the server's end of the connection finished, once it has: `EOF` or the read error (a reset).
    pub async fn ending(&mut self) -> Option<String> {
        self.begin_reading();
        tokio::time::timeout(Duration::from_secs(5), self.ended.recv())
            .await
            .ok()
            .flatten()
    }

    /// One message as a single frame.
    pub async fn message(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        self.send(opcode, true, payload).await
    }

    /// One message in frames of at most `piece` bytes.
    pub async fn fragmented(&mut self, opcode: u8, payload: &[u8], piece: usize) -> io::Result<()> {
        let pieces: Vec<&[u8]> = payload.chunks(piece).collect();
        let last = pieces.len() - 1;
        for (index, chunk) in pieces.into_iter().enumerate() {
            self.send(if index == 0 { opcode } else { CONTINUATION }, index == last, chunk)
                .await?;
        }
        Ok(())
    }

    /// The next frame the server sends; `None` once it has hung up (an error counts as hanging up).
    pub async fn frame(&mut self) -> Option<Frame> {
        self.begin_reading();
        tokio::select! {
            biased;
            frame = self.frames.recv() => frame,
            () = async { self.ended.recv().await; } => self.frames.try_recv().ok(),
            () = tokio::time::sleep(Duration::from_secs(10)) => None,
        }
    }

    fn begin_reading(&mut self) {
        if let Some(start) = self.lazy.take() {
            start();
        }
    }

    fn parse(buffer: &mut Vec<u8>) -> Option<Frame> {
        if buffer.len() < 2 {
            return None;
        }
        let (length, header) = match buffer[1] & 0x7F {
            126 if buffer.len() >= 4 => (usize::from(u16::from_be_bytes([buffer[2], buffer[3]])), 4),
            127 if buffer.len() >= 10 => (u64::from_be_bytes(buffer[2..10].try_into().unwrap()) as usize, 10),
            126 | 127 => return None,
            short => (usize::from(short), 2),
        };
        if buffer.len() < header + length {
            return None;
        }
        let frame = Frame {
            opcode: buffer[0] & 0x0F,
            payload: buffer[header..header + length].to_vec(),
        };
        buffer.drain(..header + length);
        Some(frame)
    }

    /// Every frame up to and including the first Close, or until the server hangs up.
    pub async fn until_close(&mut self) -> (Vec<Frame>, bool) {
        let mut seen = Vec::new();
        while let Some(frame) = self.frame().await {
            let closing = frame.opcode == CLOSE;
            seen.push(frame);
            if closing {
                return (seen, true);
            }
        }
        (seen, false)
    }

    /// Answers a close, then reports whether the server ends the TCP connection cleanly (EOF, not a reset).
    pub async fn finish_handshake(&mut self, code: u16) -> bool {
        self.begin_reading();
        let _ = self.send(CLOSE, true, &code.to_be_bytes()).await;
        matches!(tokio::time::timeout(Duration::from_secs(5), self.ended.recv()).await, Ok(Some(reason)) if reason == "EOF")
    }
}
