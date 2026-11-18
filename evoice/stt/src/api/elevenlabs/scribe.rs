use axum::Json;
use axum::extract::multipart::MultipartError;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use e_voice_core::schema::lang::Lang;
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::api::batch::{Batch, BatchError};
use crate::api::state::AppState;
use crate::config::api::EmotionMode;
use crate::schema::emotion::Emotion;
use crate::schema::speaker::Diarization;
use crate::schema::transcript::Transcript;

/// Error body in the shape ElevenLabs clients parse: `{"detail": {"status", "message"}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScribeError {
    pub status: StatusCode,
    pub message: String,
}

impl IntoResponse for ScribeError {
    fn into_response(self) -> Response {
        let status = if self.status.is_client_error() {
            "invalid_request"
        } else {
            "internal_error"
        };
        (
            self.status,
            Json(json!({"detail": {"status": status, "message": self.message}})),
        )
            .into_response()
    }
}

impl From<BatchError> for ScribeError {
    fn from(error: BatchError) -> Self {
        match error {
            BatchError::Decode(message) | BatchError::Model(message) => Self {
                status: StatusCode::BAD_REQUEST,
                message,
            },
            BatchError::Failed(message) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message,
            },
        }
    }
}

impl From<MultipartError> for ScribeError {
    fn from(error: MultipartError) -> Self {
        Self {
            status: error.status(),
            message: error.body_text(),
        }
    }
}

/// Multipart form of `/v1/speech-to-text`; Scribe options without a counterpart (`num_speakers`,
/// `diarization_threshold`, `tag_audio_events`, `timestamps_granularity`, …) are accepted and ignored.
#[derive(Debug, ToSchema)]
pub struct ScribeForm {
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
    /// A loaded engine name or model id selects it; any other value (`scribe_v1`) uses the default.
    pub model_id: String,
    /// `es`/`en` or `spa`/`eng`.
    pub language_code: Option<String>,
    /// `field`, `tag` or `off` for the `emotion` extension.
    pub emotion: Option<String>,
    /// `true` sets every word's `speaker_id` (`speaker_0`, …); ignored while diarization is off.
    pub diarize: Option<bool>,
}

/// Scribe's response; `words` holds one entry per recognized segment.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScribeResponse {
    pub language_code: &'static str,
    pub language_probability: f64,
    pub text: String,
    #[schema(value_type = Vec<Object>)]
    pub words: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<Emotion>,
}

/// ElevenLabs Scribe-compatible transcription.
///
/// # Errors
/// 400 for a bad form, language or emotion mode or undecodable audio; 500 when the pipeline fails.
#[utoipa::path(
    post,
    path = "/v1/speech-to-text",
    tag = "elevenlabs",
    request_body(content = ScribeForm, content_type = "multipart/form-data"),
    responses((status = 200, body = ScribeResponse), (status = 400, description = "Invalid request (ElevenLabs error shape)"))
)]
pub async fn scribe(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ScribeResponse>, ScribeError> {
    let invalid = |message: String| ScribeError {
        status: StatusCode::BAD_REQUEST,
        message,
    };
    let (mut file, mut extension, mut lang, mut emotion) = (None, None, state.lang, state.emotion);
    let (mut model, mut diarization): (Option<String>, _) = (None, Diarization::Skip);
    while let Some(field) = multipart.next_field().await? {
        match field.name().unwrap_or_default() {
            "file" => {
                extension = field
                    .file_name()
                    .and_then(|name| name.rsplit_once('.'))
                    .map(|(_, extension)| extension.to_ascii_lowercase());
                file = Some(field.bytes().await?);
            }
            "language_code" => {
                let code = field.text().await?;
                let short = match code.to_ascii_lowercase().as_str() {
                    "spa" => "es".to_owned(),
                    "eng" => "en".to_owned(),
                    other => other.to_owned(),
                };
                lang = short.parse::<Lang>().map_err(invalid)?;
            }
            "emotion" => emotion = field.text().await?.parse::<EmotionMode>().map_err(invalid)?,
            "model_id" => model = Some(field.text().await?),
            "diarize" => {
                diarization = match field.text().await?.as_str() {
                    "true" | "1" => Diarization::Run,
                    _ => Diarization::Skip,
                };
            }
            _ => {}
        }
    }
    let file = file
        .filter(|file| !file.is_empty())
        .ok_or_else(|| invalid("missing required field: file".to_owned()))?;
    let diarization = Batch::diarization(&state, diarization);
    let transcript = Batch::transcribe(&state, model.as_deref(), file.to_vec(), extension, lang, diarization).await?;
    let words = transcript
        .segments
        .iter()
        .filter(|segment| !segment.text.is_empty())
        .map(|segment| {
            let mut word = json!({
                "text": Transcript::line(segment, emotion.tags()),
                "start": Transcript::seconds(segment.span.start),
                "end": Transcript::seconds(segment.span.end),
                "type": "word",
            });
            // Asked speakers: the key always present, null where no voice overlaps the segment.
            if let (Some(object), Diarization::Run) = (word.as_object_mut(), diarization) {
                let speaker = segment.speaker.map(|speaker| format!("speaker_{}", speaker.0));
                object.insert("speaker_id".to_owned(), json!(speaker));
            }
            word
        })
        .collect();
    Ok(Json(ScribeResponse {
        language_code: transcript.lang.locale().split('-').next().unwrap_or("es"),
        language_probability: 1.0,
        text: transcript.text(emotion.tags()),
        words,
        emotion: emotion.field().then(|| transcript.emotion()),
    }))
}
