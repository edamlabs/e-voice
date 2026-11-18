use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use e_voice_core::schema::lang::Lang;
use serde::Deserialize;
use serde_json::json;

use crate::api::state::AppState;
use crate::speech::base::SpeechRequest;
use crate::workflow::queue::SpeakError;

/// `POST /speak` body; `voice` and `lang` (`es` or `en`) fall back to the speech service's defaults.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakBody {
    text: String,
    voice: Option<String>,
    lang: Option<Lang>,
}

/// Answers once the device has played the text, 200 `{"id", "outcome": "done"}`, or once a stop cut it
/// short, 200 `{"id", "outcome": "cancelled"}`: ended by another client, not an error. A failure answers
/// `{"outcome": "failed", "error"}`: 400 empty text or over 4096 characters, 429 full queue, 502 speech
/// service, 503 output device, 500 stopped queue. Once the text is accepted, a client that stops waiting
/// does not stop the speech; a client gone before that never reaches the queue.
pub async fn speak(State(state): State<AppState>, Json(body): Json<SpeakBody>) -> Response {
    let request = SpeechRequest {
        text: body.text,
        voice: body.voice,
        lang: body.lang,
    };
    match state.speech.say(request).await {
        Ok((id, outcome)) => (StatusCode::OK, Json(json!({"id": id, "outcome": outcome}))).into_response(),
        Err(error) => {
            let status = match &error {
                SpeakError::Empty | SpeakError::TooLong(_) => StatusCode::BAD_REQUEST,
                SpeakError::Full => StatusCode::TOO_MANY_REQUESTS,
                SpeakError::Speech(_) => StatusCode::BAD_GATEWAY,
                SpeakError::Device(_) => StatusCode::SERVICE_UNAVAILABLE,
                SpeakError::Stopped => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let body = json!({"outcome": "failed", "error": error.to_string()});
            (status, Json(body)).into_response()
        }
    }
}
