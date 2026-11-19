use std::convert::Infallible;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use e_voice_core::schema::lang::Lang;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use utoipa::ToSchema;

use crate::api::error::ApiError;
use crate::api::state::AppState;
use crate::core::encode::AudioFormat;
use crate::schema::event::Event;
use crate::workflow::runner::{INPUT, Request};

/// How the audio is delivered: raw bytes as they are generated, or server-sent events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum StreamFormat {
    #[default]
    Audio,
    Sse,
}

/// OpenAI `POST /v1/audio/speech`. `voice` is a learned voice id; any other name (OpenAI presets
/// such as `alloy`) speaks with the default voice. `response_format`: `wav` (default), `pcm`
/// (24 kHz s16le, like OpenAI) or `f32`; compressed formats are not offered. `model` and `speed`
/// are accepted and ignored; `language` (extension) overrides the default language.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SpeechRequest {
    #[allow(dead_code)]
    model: Option<String>,
    input: String,
    voice: Option<String>,
    response_format: Option<String>,
    stream_format: Option<StreamFormat>,
    #[allow(dead_code)]
    speed: Option<f32>,
    #[allow(dead_code)]
    instructions: Option<String>,
    language: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum SpeechEvent {
    #[serde(rename = "speech.audio.delta")]
    Delta { audio: String },
    #[serde(rename = "speech.audio.done")]
    Done,
}

impl SpeechEvent {
    fn frame(&self) -> Bytes {
        Bytes::from(format!("data: {}\n\n", serde_json::to_string(self).unwrap_or_default()))
    }
}

/// The largest body the speech door reads: 64 KiB, room for [`INPUT`] characters even with every one
/// JSON-escaped, so an oversized request is refused before it is buffered.
pub const BODY: usize = 65_536;

/// Speaks `input`, streaming audio while later sentences are still being generated. The answer is
/// back-pressured by the client's reading, so memory is bounded by the runner's channels, not by the
/// text; [`INPUT`] bounds the work one request can ask for.
///
/// # Errors
/// A body over [`BODY`] or not valid JSON, an unsupported format or language, empty input or input over
/// [`INPUT`] characters, an unreadable default voice, or no free worker.
#[utoipa::path(
    post,
    path = "/v1/audio/speech",
    tag = "openai",
    request_body = SpeechRequest,
    responses(
        (status = 200, description = "audio bytes (wav, pcm or f32) or SSE `speech.audio.delta` events"),
        (status = 400, body = crate::api::error::ErrorBody),
        (status = 413, description = "a body over 64 KiB", body = crate::api::error::ErrorBody),
        (status = 422, description = "JSON that is not a speech request", body = crate::api::error::ErrorBody),
        (status = 503, description = "every worker is busy", body = crate::api::error::ErrorBody)
    )
)]
pub async fn speech(
    State(state): State<AppState>,
    body: Result<Json<SpeechRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError::Rejected(rejection.status(), rejection.body_text()))?;
    let format = match request.response_format.as_deref().unwrap_or("wav") {
        "wav" => AudioFormat::Wav,
        "pcm" => AudioFormat::Pcm,
        "f32" => AudioFormat::F32,
        other => {
            return Err(ApiError::Invalid(format!(
                "unsupported response_format {other:?}; use wav, pcm or f32"
            )));
        }
    };
    let lang = request
        .language
        .as_deref()
        .map(str::parse::<Lang>)
        .transpose()
        .map_err(ApiError::Invalid)?
        .unwrap_or(state.lang);
    (!request.input.trim().is_empty())
        .then_some(())
        .ok_or_else(|| ApiError::Invalid("input is empty".to_owned()))?;
    let length = request.input.chars().count();
    (length <= INPUT)
        .then_some(())
        .ok_or_else(|| ApiError::Invalid(format!("input is too long: {length} characters, at most {INPUT}")))?;
    let voice = state.voice(request.voice).await?;
    let session = state.runner.open(lang, voice).await?;
    let (requests, inbox) = mpsc::channel(2);
    let (outbox, events) = mpsc::channel(64);
    requests.send(Request::Text(request.input)).await.ok();
    requests.send(Request::Close).await.ok();
    let (runner, cancel) = (state.runner.clone(), state.shutdown.child_token());
    tokio::spawn(async move { runner.run(session, inbox, outbox, cancel).await });
    let rate = state.runner.caps().rate;
    let sse = request.stream_format.unwrap_or_default() == StreamFormat::Sse;
    let encode = move |event: Event| match (event, sse) {
        (Event::Audio { audio, .. }, false) => Some(Bytes::from(format.samples(&audio))),
        (Event::Audio { audio, .. }, true) => Some(
            SpeechEvent::Delta {
                audio: STANDARD.encode(format.samples(&audio)),
            }
            .frame(),
        ),
        (Event::Closed, true) => Some(SpeechEvent::Done.frame()),
        _ => None,
    };
    let header = (!sse).then(|| Ok::<_, Infallible>(Bytes::from(format.header(rate))));
    let body =
        futures_util::stream::iter(header).chain(futures_util::stream::unfold(events, move |mut events| async move {
            loop {
                if let Some(bytes) = encode(events.recv().await?) {
                    return Some((Ok(bytes), events));
                }
            }
        }));
    let mime = if sse { "text/event-stream" } else { format.mime() };
    Ok(([(header::CONTENT_TYPE, mime)], Body::from_stream(body)).into_response())
}
