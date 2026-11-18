use std::fmt::Write;

use axum::Json;
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::extract::multipart::MultipartError;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::sse::Event as SseEvent;
use axum::response::{IntoResponse, Response};
use e_voice_core::schema::lang::Lang;
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::api::batch::BatchError;
use crate::config::api::EmotionMode;
use crate::schema::emotion::Emotion;
use crate::schema::event::FinalEvent;
use crate::schema::speaker::{Diarization, SpeakerId};
use crate::schema::transcript::Transcript;

/// Error body in the shape OpenAI clients parse: `{"error": {message, type, param, code}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenaiError {
    pub status: StatusCode,
    pub message: String,
    pub kind: &'static str,
    pub param: Option<&'static str>,
}

impl OpenaiError {
    #[must_use]
    pub fn invalid(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            kind: "invalid_request_error",
            param,
        }
    }

    #[must_use]
    pub fn server(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            kind: "server_error",
            param: None,
        }
    }
}

impl From<MultipartError> for OpenaiError {
    fn from(error: MultipartError) -> Self {
        Self {
            status: error.status(),
            message: error.body_text(),
            kind: "invalid_request_error",
            param: None,
        }
    }
}

impl From<BatchError> for OpenaiError {
    fn from(error: BatchError) -> Self {
        match error {
            BatchError::Decode(message) => Self::invalid(message, Some("file")),
            BatchError::Model(message) => Self::invalid(message, Some("model")),
            BatchError::Failed(message) => Self::server(message),
            BatchError::TooLong(message) => Self {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                ..Self::invalid(message, Some("file"))
            },
        }
    }
}

impl IntoResponse for OpenaiError {
    fn into_response(self) -> Response {
        let body = json!({"error": {"message": self.message, "type": self.kind, "param": self.param, "code": null}});
        (self.status, Json(body)).into_response()
    }
}

/// `response_format` of `/v1/audio/transcriptions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenaiFormat {
    #[default]
    Json,
    Text,
    Srt,
    Vtt,
    VerboseJson,
    DiarizedJson,
}

/// `json` response: OpenAI's `text` and `usage`, plus `emotion` unless `emotion=off`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranscriptionJson {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<Emotion>,
    #[schema(value_type = Object)]
    pub usage: Value,
}

/// One `verbose_json` segment; the OpenAI decoding fields are fixed placeholders.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerboseSegment {
    pub id: u64,
    pub seek: u64,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub tokens: Vec<u32>,
    pub temperature: f64,
    pub avg_logprob: f64,
    pub compression_ratio: f64,
    pub no_speech_prob: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<Emotion>,
}

/// One `diarized_json` segment, streamed alike as a `transcript.text.segment` event. `speaker` is
/// absent only where no voice overlaps the segment.
#[derive(Debug, Serialize, ToSchema)]
pub struct DiarizedSegment {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: String,
    pub start: f64,
    pub end: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
}

/// `diarized_json` response: OpenAI's shape, plus `emotion` unless `emotion=off`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranscriptionDiarized {
    pub task: &'static str,
    pub duration: f64,
    pub text: String,
    pub segments: Vec<DiarizedSegment>,
    #[schema(value_type = Object)]
    pub usage: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<Emotion>,
}

/// `verbose_json` response.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranscriptionVerbose {
    pub task: &'static str,
    pub language: &'static str,
    pub duration: f64,
    pub text: String,
    pub segments: Vec<VerboseSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<Emotion>,
}

impl OpenaiFormat {
    // ##### PRIVATE #####

    fn render_clock(samples: u64, separator: char) -> String {
        let millis = samples / 16;
        let (hours, minutes, seconds, rest) = (
            millis / 3_600_000,
            (millis / 60_000) % 60,
            (millis / 1_000) % 60,
            millis % 1_000,
        );
        format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{rest:03}")
    }

    fn render_cues(transcript: &Transcript, separator: char, numbered: bool, tags: bool) -> String {
        let mut cues = String::new();
        let spoken = transcript.segments.iter().filter(|segment| !segment.text.is_empty());
        for (index, segment) in (1u64..).zip(spoken) {
            let (start, end) = (
                Self::render_clock(segment.span.start, separator),
                Self::render_clock(segment.span.end, separator),
            );
            let number = if numbered { format!("{index}\n") } else { String::new() };
            writeln!(cues, "{number}{start} --> {end}\n{}\n", Transcript::line(segment, tags)).ok();
        }
        cues
    }

    /// OpenAI's letters: `A` … `Z`, then `AA`, `AB`, … — past `Z` is ours, upstream documents none.
    fn render_speaker(speaker: SpeakerId) -> String {
        let next = |rest: &u32| Some(rest.saturating_sub(1) / 26).filter(|left| *left > 0);
        let offset = |rest: u32| u8::try_from(rest.saturating_sub(1) % 26).unwrap_or(0);
        let letter = |rest: u32| char::from(b'A'.saturating_add(offset(rest)));
        let letters: Vec<char> = std::iter::successors(Some(speaker.0.saturating_add(1)), next)
            .map(letter)
            .collect();
        letters.into_iter().rev().collect()
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// # Errors
    /// The value is not one of OpenAI's formats.
    pub fn parse(value: &str) -> Result<Self, OpenaiError> {
        match value {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            "srt" => Ok(Self::Srt),
            "vtt" => Ok(Self::Vtt),
            "verbose_json" => Ok(Self::VerboseJson),
            "diarized_json" => Ok(Self::DiarizedJson),
            other => Err(OpenaiError::invalid(
                format!("unsupported response_format {other:?}; use json, text, srt, vtt, verbose_json, diarized_json"),
                Some("response_format"),
            )),
        }
    }

    /// Speakers are asked of the pipeline by `diarized_json` alone.
    #[must_use]
    pub const fn diarization(self) -> Diarization {
        match self {
            Self::DiarizedJson => Diarization::Run,
            Self::Json | Self::Text | Self::Srt | Self::Vtt | Self::VerboseJson => Diarization::Skip,
        }
    }

    /// One spoken final in `diarized_json` shape, its id the pipeline's segment id.
    #[must_use]
    pub fn diarized(segment: &FinalEvent, speaker: Option<SpeakerId>, tags: bool) -> DiarizedSegment {
        DiarizedSegment {
            kind: "transcript.text.segment",
            id: segment.segment.0.to_string(),
            start: Transcript::seconds(segment.span.start),
            end: Transcript::seconds(segment.span.end),
            speaker: speaker.map(Self::render_speaker),
            text: Transcript::line(segment, tags),
        }
    }

    /// The transcript in this format; emotion appears as fields and/or inline tags per `emotion`.
    #[must_use]
    pub fn render(self, transcript: &Transcript, emotion: EmotionMode) -> Response {
        let text = transcript.text(emotion.tags());
        let summary = emotion.field().then(|| transcript.emotion());
        match self {
            Self::Json => Json(TranscriptionJson {
                text,
                emotion: summary,
                usage: json!({"type": "duration", "seconds": transcript.duration().ceil()}),
            })
            .into_response(),
            Self::Text => ([(CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
            Self::Srt => (
                [(CONTENT_TYPE, "text/plain; charset=utf-8")],
                Self::render_cues(transcript, ',', true, emotion.tags()),
            )
                .into_response(),
            Self::Vtt => {
                let cues = Self::render_cues(transcript, '.', false, emotion.tags());
                ([(CONTENT_TYPE, "text/vtt; charset=utf-8")], format!("WEBVTT\n\n{cues}")).into_response()
            }
            Self::VerboseJson => {
                let segments = transcript
                    .segments
                    .iter()
                    .filter(|segment| !segment.text.is_empty())
                    .zip(0u64..)
                    .map(|(segment, id)| VerboseSegment {
                        id,
                        seek: 0,
                        start: Transcript::seconds(segment.span.start),
                        end: Transcript::seconds(segment.span.end),
                        text: Transcript::line(segment, emotion.tags()),
                        tokens: Vec::new(),
                        temperature: 0.0,
                        avg_logprob: 0.0,
                        compression_ratio: 0.0,
                        no_speech_prob: 0.0,
                        emotion: emotion.field().then(|| segment.emotion.clone()),
                    })
                    .collect();
                Json(TranscriptionVerbose {
                    task: "transcribe",
                    language: transcript.lang.name(),
                    duration: transcript.duration(),
                    text,
                    segments,
                    emotion: summary,
                })
                .into_response()
            }
            Self::DiarizedJson => Json(TranscriptionDiarized {
                task: "transcribe",
                duration: transcript.duration(),
                text,
                segments: transcript
                    .segments
                    .iter()
                    .filter(|segment| !segment.text.is_empty())
                    .map(|segment| Self::diarized(segment, segment.speaker, emotion.tags()))
                    .collect(),
                usage: json!({"type": "duration", "seconds": transcript.duration().ceil()}),
                emotion: summary,
            })
            .into_response(),
        }
    }

    pub fn delta(text: &str) -> SseEvent {
        SseEvent::default().data(json!({"type": "transcript.text.delta", "delta": text}).to_string())
    }

    pub fn done(text: &str, seconds: f64) -> SseEvent {
        let usage = json!({"type": "duration", "seconds": seconds.ceil()});
        SseEvent::default().data(json!({"type": "transcript.text.done", "text": text, "usage": usage}).to_string())
    }

    pub fn segment(segment: &DiarizedSegment) -> SseEvent {
        SseEvent::default().data(json!(segment).to_string())
    }

    pub fn failure(message: &str) -> SseEvent {
        SseEvent::default()
            .data(json!({"type": "error", "error": {"message": message, "type": "server_error"}}).to_string())
    }
}

/// Multipart form of `/v1/audio/transcriptions`. `prompt`, `temperature` and
/// `timestamp_granularities[]` are accepted and ignored; `emotion` is an extension.
#[derive(Debug, ToSchema)]
pub struct TranscriptionForm {
    /// Audio: wav, mp3, m4a, flac, ogg, opus or webm.
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
    /// A loaded engine (`parakeet`, `canary`, `cohere`, `whisper`, or its model id) selects it; any
    /// other value (`whisper-1`, …) uses the default file engine.
    pub model: String,
    /// `es` or `en` (regions such as `es-ES` are accepted).
    pub language: Option<String>,
    /// `json` (default), `text`, `srt`, `vtt`, `verbose_json` or `diarized_json` (speakers; refused while
    /// diarization is off). `chunking_strategy` and `known_speaker_*` are accepted and ignored.
    pub response_format: Option<String>,
    /// `true` streams `transcript.text.delta` server-sent events; with `diarized_json`, one
    /// `transcript.text.segment` per spoken segment follows once speakers are known.
    pub stream: Option<bool>,
    /// `field`, `tag` or `off`; the server default when omitted.
    pub emotion: Option<String>,
}

/// The parsed form, with defaults resolved.
#[derive(Debug)]
pub struct OpenaiForm {
    pub file: Bytes,
    pub extension: Option<String>,
    pub model: Option<String>,
    pub lang: Lang,
    pub format: OpenaiFormat,
    pub stream: bool,
    pub emotion: EmotionMode,
}

impl OpenaiForm {
    /// # Errors
    /// Malformed multipart, an unsupported language, format or emotion mode, or a missing `file`.
    pub async fn parse(mut multipart: Multipart, lang: Lang, emotion: EmotionMode) -> Result<Self, OpenaiError> {
        let (mut file, mut extension, mut model, mut chosen) = (None, None, None, lang);
        let (mut format, mut stream, mut mode) = (OpenaiFormat::Json, false, emotion);
        while let Some(field) = multipart.next_field().await? {
            match field.name().unwrap_or_default() {
                "file" => {
                    extension = field
                        .file_name()
                        .and_then(|name| name.rsplit_once('.'))
                        .map(|(_, extension)| extension.to_ascii_lowercase());
                    file = Some(field.bytes().await?);
                }
                "model" => model = Some(field.text().await?),
                "language" => {
                    chosen = field
                        .text()
                        .await?
                        .parse()
                        .map_err(|error: String| OpenaiError::invalid(error, Some("language")))?;
                }
                "response_format" => format = OpenaiFormat::parse(field.text().await?.as_str())?,
                "stream" => stream = matches!(field.text().await?.as_str(), "true" | "1"),
                "emotion" => {
                    mode = field
                        .text()
                        .await?
                        .parse()
                        .map_err(|error: String| OpenaiError::invalid(error, Some("emotion")))?;
                }
                _ => {}
            }
        }
        let file = file
            .filter(|file| !file.is_empty())
            .ok_or_else(|| OpenaiError::invalid("missing required field: file", Some("file")))?;
        Ok(Self {
            file,
            extension,
            model,
            lang: chosen,
            format,
            stream,
            emotion: mode,
        })
    }
}
