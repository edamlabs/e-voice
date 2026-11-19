use std::fmt::{self, Debug};
use std::io::Cursor;
use std::num::NonZeroU32;
use std::time::Duration;

use e_voice_core::audio::MAX_RATE;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use hound::{SampleFormat, WavSpec, WavWriter};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::speech::base::{Frame, Learning, Speech, SpeechError, SpeechRequest, Utterance, VoiceClip, Voices};

/// The longest a connect plus WebSocket handshake may take. The service is local, so seconds is already
/// generous; past it the host is dead or black-holed, and the queue must move on rather than wait out the
/// OS connect timeout.
const CONNECT: Duration = Duration::from_secs(5);

/// The largest message or frame read from the service: 256 KiB. Audio comes one 80 ms frame at a time, at
/// most 120 KiB at the highest rate accepted (`MAX_RATE`, f32); events are JSON of one sentence at most.
const MESSAGE: usize = 262_144;

/// The largest answer read from the service's HTTP side: 64 KiB, a voice list of thousands of ids. Read
/// chunk by chunk, so a broken service cannot make the daemon buffer more.
const ANSWER: usize = 65_536;

/// The longest the service may take to learn a voice: it decodes and encodes at most 30 s of audio, on
/// the CPU for some backends.
const LEARN: Duration = Duration::from_secs(120);

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

    /// `GET /v1/voices`. Connect and the whole answer are bounded by [`CONNECT`].
    async fn list_fetch(&self) -> Result<Vec<String>, SpeechError> {
        let connect = |error: reqwest::Error| SpeechError::Connect(error.to_string());
        let url = self.common_url("/v1/voices")?;
        let response = Self::common_client()?
            .get(url)
            .timeout(CONNECT)
            .send()
            .await
            .map_err(connect)?;
        let text = Self::common_answer(response, "a voice list").await?;
        serde_json::from_str::<VoiceList>(&text)
            .map(|list| list.voices)
            .map_err(|error| SpeechError::Protocol(error.to_string()))
    }

    /// `POST /v1/voices` as multipart: `voice_id`, `text` when the clip has one, and the clip as a 16-bit
    /// WAV `file`. The service may take a while to learn, so the whole answer is bounded by [`LEARN`].
    async fn learn_send(&self, clip: VoiceClip) -> Result<(), SpeechError> {
        let connect = |error: reqwest::Error| SpeechError::Connect(error.to_string());
        let url = self.common_url("/v1/voices")?;
        let wav = Part::bytes(Self::learn_wav(clip.rate.get(), &clip.samples)?)
            .file_name("voice.wav")
            .mime_str("audio/wav")
            .map_err(connect)?;
        let form = Form::new().text("voice_id", clip.id);
        let form = clip.text.into_iter().fold(form, |form, text| form.text("text", text));
        let response = Self::common_client()?
            .post(url)
            .multipart(form.part("file", wav))
            .timeout(LEARN)
            .send()
            .await
            .map_err(connect)?;
        Self::common_answer(response, "an answer").await.map(drop)
    }

    /// Mono 16-bit PCM WAV at `rate`, samples clamped to [-1, 1].
    fn learn_wav(rate: u32, samples: &[f32]) -> Result<Vec<u8>, SpeechError> {
        let encode = |error: hound::Error| SpeechError::Protocol(format!("cannot encode the clip: {error}"));
        let spec = WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut wav = Vec::new();
        let mut writer = WavWriter::new(Cursor::new(&mut wav), spec).map_err(encode)?;
        samples
            .iter()
            .try_for_each(|sample| {
                #[allow(clippy::cast_possible_truncation)]
                let value = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
                writer.write_sample(value)
            })
            .map_err(encode)?;
        writer.finalize().map_err(encode)?;
        Ok(wav)
    }

    /// `path` on the service's HTTP side: the same host and port as its WebSocket base.
    fn common_url(&self, path: &str) -> Result<Url, SpeechError> {
        let mut url = Url::parse(&format!("{}{path}", self.url.trim_end_matches('/')))
            .map_err(|error| SpeechError::Connect(error.to_string()))?;
        url.set_scheme("http")
            .map_err(|()| SpeechError::Connect(format!("{} has no HTTP form", self.url)))?;
        Ok(url)
    }

    fn common_client() -> Result<reqwest::Client, SpeechError> {
        reqwest::Client::builder()
            .connect_timeout(CONNECT)
            .build()
            .map_err(|error| SpeechError::Connect(error.to_string()))
    }

    /// The answer's body, read chunk by chunk up to [`ANSWER`] bytes (`what` names it when it is over); a
    /// status other than success is the service's refusal, with the body as its reason.
    async fn common_answer(mut response: reqwest::Response, what: &str) -> Result<String, SpeechError> {
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| SpeechError::Connect(error.to_string()))?
        {
            let size = body.len().saturating_add(chunk.len());
            (size <= ANSWER)
                .then_some(())
                .ok_or_else(|| SpeechError::Protocol(format!("{what} over {ANSWER} bytes")))?;
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        status
            .is_success()
            .then_some(())
            .ok_or_else(|| SpeechError::Server(format!("{status}: {text}")))?;
        Ok(text)
    }
}

impl Voices for TtsSpeech {
    fn list(&self) -> BoxFuture<'_, Result<Vec<String>, SpeechError>> {
        Box::pin(self.list_fetch())
    }
}

impl Learning for TtsSpeech {
    fn learn(&self, clip: VoiceClip) -> BoxFuture<'_, Result<(), SpeechError>> {
        Box::pin(self.learn_send(clip))
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
