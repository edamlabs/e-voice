use std::fmt::{self, Debug};
use std::num::NonZeroU32;
use std::time::Duration;

use e_voice_core::schema::emotion::Emotion;
use e_voice_core::schema::error::NodeError;
use e_voice_core::schema::lang::Lang;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::hearing::base::{Heard, Hearing, HearingError, Recognized, Transcription};

/// The longest a connect plus WebSocket handshake may take; the recognizer is local, so past it the host
/// is dead or black-holed.
const CONNECT: Duration = Duration::from_secs(5);

/// The largest message or frame read from the recognizer: 64 KiB. It sends JSON events only, a few hundred
/// bytes each, so a message near the cap is a broken peer, refused before it is buffered.
const MESSAGE: usize = 65_536;

/// One sample of the recognizer's own stream in nanoseconds: it counts `speech` offsets at 16 kHz,
/// whatever rate the session was opened at.
const SAMPLE: u64 = 62_500;

/// Whether a `speech` event opens or closes a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttVoice {
    Started,
    Stopped,
}

/// What the recognizer's native stream (`view=struct`) sends, read as far as the daemon needs: `wake`,
/// `speakers` and fields such as `segment` are skipped; `at` is the 16 kHz stream sample where speech
/// started (estimated) or stopped (exact). Public so the recognizer's own tests can prove this mirror
/// parses what it really emits.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SttEvent {
    Speech {
        state: SttVoice,
        at: u64,
    },
    Partial {
        text: String,
    },
    Final {
        text: String,
        lang: Lang,
        emotion: Emotion,
        error: Option<NodeError>,
    },
    Closed {},
    #[serde(other)]
    Other,
}

/// e-voice's STT service over its native stream (`/v1/stream`, f32le binary frames in, JSON events out).
#[derive(Debug, Clone)]
pub struct SttHearing {
    url: String,
}

impl SttHearing {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    /// The stream's URL for mono f32le at `rate`, every event as JSON, the emotion as a field. Public so the
    /// recognizer's own tests can prove it is a query the service accepts.
    ///
    /// # Errors
    /// The configured base is not a URL.
    pub fn url(&self, rate: NonZeroU32) -> Result<Url, HearingError> {
        let mut url = Url::parse(&format!("{}/v1/stream", self.url.trim_end_matches('/')))
            .map_err(|error| HearingError::Connect(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("rate", &rate.to_string())
            .append_pair("encoding", "f32le")
            .append_pair("view", "struct")
            .append_pair("emotion", "field");
        Ok(url)
    }
}

impl Hearing for SttHearing {
    type Transcription = SttTranscription;

    /// Nagle is off: the session is a steady flow of small audio blocks, and batching them only delays
    /// what the recognizer hears.
    async fn open(&self, rate: NonZeroU32) -> Result<SttTranscription, HearingError> {
        let url = self.url(rate)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MESSAGE))
            .max_frame_size(Some(MESSAGE));
        let connect = tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), true);
        let (socket, _) = tokio::time::timeout(CONNECT, connect)
            .await
            .map_err(|_| HearingError::Connect(format!("no handshake within {} s", CONNECT.as_secs())))?
            .map_err(|error| HearingError::Connect(error.to_string()))?;
        Ok(SttTranscription { socket })
    }
}

/// One live session on its own socket; dropping it drops the socket, and the recognizer discards what it
/// had not finished.
pub struct SttTranscription {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Debug for SttTranscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SttTranscription").finish_non_exhaustive()
    }
}

impl Transcription for SttTranscription {
    async fn send(&mut self, samples: &[f32]) -> Result<(), HearingError> {
        let bytes: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        self.socket
            .send(Message::binary(bytes))
            .await
            .map_err(|error| HearingError::Protocol(error.to_string()))
    }

    /// A normal close ends the session like `closed`; any other close carries the recognizer's reason
    /// (1003 for audio it refuses, 1011 for its own failure).
    async fn next(&mut self) -> Option<Result<Recognized, HearingError>> {
        while let Some(message) = self.socket.next().await {
            let event = match message {
                Ok(Message::Text(text)) => serde_json::from_str::<SttEvent>(&text),
                Ok(Message::Close(Some(frame))) if frame.code != CloseCode::Normal => {
                    let reason = format!("closed with {}: {}", frame.code, frame.reason.as_str());
                    return Some(Err(HearingError::Server(reason)));
                }
                Ok(Message::Close(_)) => return None,
                Ok(Message::Binary(_)) => return Some(Err(HearingError::Protocol("a binary frame".to_owned()))),
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => continue,
                Err(error) => return Some(Err(HearingError::Protocol(error.to_string()))),
            };
            match event {
                Ok(SttEvent::Speech { state, at }) => {
                    let at = Duration::from_nanos(at.saturating_mul(SAMPLE));
                    return Some(Ok(match state {
                        SttVoice::Started => Recognized::Started(at),
                        SttVoice::Stopped => Recognized::Stopped(at),
                    }));
                }
                Ok(SttEvent::Partial { text }) => return Some(Ok(Recognized::Partial(text))),
                Ok(SttEvent::Final {
                    text,
                    lang,
                    emotion,
                    error,
                }) => {
                    let heard = Heard { text, lang, emotion };
                    return Some(Ok(Recognized::Final(heard, error)));
                }
                Ok(SttEvent::Closed {}) => return None,
                Ok(SttEvent::Other) => {}
                Err(error) => return Some(Err(HearingError::Protocol(error.to_string()))),
            }
        }
        Some(Err(HearingError::Protocol(
            "the recognizer dropped the connection".to_owned(),
        )))
    }
}
