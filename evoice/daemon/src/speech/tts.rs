use std::fmt::{self, Debug};
use std::num::NonZeroU32;
use std::time::Duration;

use e_voice_core::audio::MAX_RATE;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::speech::base::{Frame, Speech, SpeechError, SpeechRequest, Utterance, Voices};

/// The longest a connect plus WebSocket handshake may take. The service is local, so seconds is already
/// generous; past it the host is dead or black-holed, and the queue must move on rather than wait out the
/// OS connect timeout.
const CONNECT: Duration = Duration::from_secs(5);

/// The largest message or frame read from the service: 256 KiB. Audio comes one 80 ms frame at a time, at
/// most 120 KiB at the highest rate accepted (`MAX_RATE`, f32); events are JSON of one sentence at most.
const MESSAGE: usize = 262_144;

/// The largest voice list read from the service: 64 KiB, thousands of ids. Read chunk by chunk, so a
/// broken service cannot make the daemon buffer more.
const LIST: usize = 65_536;

/// The service's voice list: `{"voices": ["id", …]}`.
#[derive(Debug, Deserialize)]
struct VoiceList {
    voices: Vec<String>,
}

/// What the TTS stream sends besides audio; anything else (`start`, …) is skipped.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum TtsEvent {
    Ready {
        rate: u32,
    },
    End {
        error: Option<Value>,
    },
    Error {
        message: Option<String>,
    },
    Closed {},
    #[serde(other)]
    Other,
}

/// e-voice's TTS service over its streaming WebSocket (`/v1/stream`, f32 frames): the whole text is
/// sent at once, then `close`, and audio is read until `closed`. The query is built by a URL encoder:
/// `voice` comes from any local client and must never rewrite the request.
#[derive(Debug, Clone)]
pub struct TtsSpeech {
    url: String,
}

impl TtsSpeech {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

impl TtsSpeech {
    // ##### PRIVATE #####

    /// `GET /v1/voices` on the service's HTTP side: the same host and port as its WebSocket base. Connect
    /// and the whole answer are bounded by [`CONNECT`]; the body by [`LIST`].
    async fn list_fetch(&self) -> Result<Vec<String>, SpeechError> {
        let connect = |error: reqwest::Error| SpeechError::Connect(error.to_string());
        let mut url = Url::parse(&format!("{}/v1/voices", self.url.trim_end_matches('/')))
            .map_err(|error| SpeechError::Connect(error.to_string()))?;
        url.set_scheme("http")
            .map_err(|()| SpeechError::Connect(format!("{} has no HTTP form", self.url)))?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT)
            .build()
            .map_err(connect)?;
        let mut response = http.get(url).timeout(CONNECT).send().await.map_err(connect)?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(connect)? {
            let size = body.len().saturating_add(chunk.len());
            (size <= LIST)
                .then_some(())
                .ok_or_else(|| SpeechError::Protocol(format!("a voice list over {LIST} bytes")))?;
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&body);
        status
            .is_success()
            .then_some(())
            .ok_or_else(|| SpeechError::Server(format!("{status}: {text}")))?;
        serde_json::from_str::<VoiceList>(&text)
            .map(|list| list.voices)
            .map_err(|error| SpeechError::Protocol(error.to_string()))
    }
}

impl Voices for TtsSpeech {
    fn list(&self) -> BoxFuture<'_, Result<Vec<String>, SpeechError>> {
        Box::pin(self.list_fetch())
    }
}

impl Speech for TtsSpeech {
    type Utterance = TtsUtterance;

    async fn say(&self, request: SpeechRequest) -> Result<TtsUtterance, SpeechError> {
        let mut url = Url::parse(&format!("{}/v1/stream", self.url.trim_end_matches('/')))
            .map_err(|error| SpeechError::Connect(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("format", "f32")
            .extend_pairs(request.lang.map(|lang| ("lang", lang.code())))
            .extend_pairs(request.voice.as_deref().map(|voice| ("voice", voice)));
        let config = WebSocketConfig::default()
            .max_message_size(Some(MESSAGE))
            .max_frame_size(Some(MESSAGE));
        let connect = tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), false);
        let (mut socket, _) = tokio::time::timeout(CONNECT, connect)
            .await
            .map_err(|_| SpeechError::Connect(format!("no handshake within {} s", CONNECT.as_secs())))?
            .map_err(|error| SpeechError::Connect(error.to_string()))?;
        let lost = |error: SocketError| SpeechError::Protocol(error.to_string());
        let text = json!({"type": "text", "text": request.text}).to_string();
        socket.send(Message::text(text)).await.map_err(lost)?;
        let close = json!({"type": "close"}).to_string();
        socket.send(Message::text(close)).await.map_err(lost)?;
        Ok(TtsUtterance { socket, rate: None })
    }
}

/// One text on its own socket: audio frames until the service sends `closed`.
pub struct TtsUtterance {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    rate: Option<NonZeroU32>,
}

impl Debug for TtsUtterance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtsUtterance")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl TtsUtterance {
    // ##### PRIVATE #####

    fn frame_audio(&self, bytes: &[u8]) -> Result<Frame, SpeechError> {
        let rate = self
            .rate
            .ok_or_else(|| SpeechError::Protocol("audio before `ready`".to_owned()))?;
        let (chunks, rest) = bytes.as_chunks::<4>();
        let samples: Vec<f32> = rest
            .is_empty()
            .then(|| chunks.iter().map(|chunk| f32::from_le_bytes(*chunk)).collect())
            .ok_or_else(|| SpeechError::Protocol(format!("a frame of {} bytes is not f32", bytes.len())))?;
        Ok(Frame { rate, samples })
    }

    fn frame_early() -> SpeechError {
        SpeechError::Protocol("the service closed before `closed`".to_owned())
    }
}

impl Utterance for TtsUtterance {
    async fn frame(&mut self) -> Option<Result<Frame, SpeechError>> {
        while let Some(message) = self.socket.next().await {
            let event = match message {
                Ok(Message::Binary(bytes)) => return Some(self.frame_audio(&bytes)),
                Ok(Message::Text(text)) => serde_json::from_str::<TtsEvent>(&text),
                Ok(Message::Close(_)) => return Some(Err(Self::frame_early())),
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => continue,
                Err(error) => return Some(Err(SpeechError::Protocol(error.to_string()))),
            };
            match event {
                Ok(TtsEvent::Ready { rate }) => match NonZeroU32::new(rate).filter(|rate| rate.get() <= MAX_RATE) {
                    Some(rate) => self.rate = Some(rate),
                    None => return Some(Err(SpeechError::Protocol(format!("`ready` with rate {rate}")))),
                },
                Ok(TtsEvent::End { error: Some(error) }) => return Some(Err(SpeechError::Server(error.to_string()))),
                Ok(TtsEvent::Error { message }) => return Some(Err(SpeechError::Server(message.unwrap_or_default()))),
                Ok(TtsEvent::Closed {}) => return None,
                Ok(TtsEvent::End { error: None } | TtsEvent::Other) => {}
                Err(error) => return Some(Err(SpeechError::Protocol(error.to_string()))),
            }
        }
        Some(Err(Self::frame_early()))
    }

    async fn cancel(&mut self) -> Result<(), SpeechError> {
        let cancel = json!({"type": "cancel"}).to_string();
        self.socket
            .send(Message::text(cancel))
            .await
            .map_err(|error| SpeechError::Protocol(error.to_string()))
    }
}
