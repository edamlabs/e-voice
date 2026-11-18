use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::state::AppState;
use crate::workflow::listen::ListenError;

/// `POST /listen` body, optional as a whole: `timeout` (`"8s"`, `"500ms"`) is how long to wait for
/// someone to start speaking, `[daemon] listen` when absent.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenBody {
    #[serde(default, with = "humantime_serde")]
    timeout: Option<Duration>,
}

impl ListenBody {
    /// Read through a map, because serde's derive would also take a sequence: `["2s"]` as a timeout.
    fn parse_object(body: &[u8]) -> Result<Self, serde_json::Error> {
        let object: Map<String, Value> = serde_json::from_slice(body)?;
        serde_json::from_value(Value::Object(object))
    }

    /// No body when empty; otherwise a JSON object, or the error that says why it is not one.
    fn parse(body: &[u8]) -> Result<Option<Self>, serde_json::Error> {
        (!body.is_empty()).then(|| Self::parse_object(body)).transpose()
    }
}

/// Opens the microphone and answers with the first segment heard in full, 200 `{"text", "lang",
/// "emotion"}`. A failure answers `{"error"}`: 400 a body that is not a listen or a timeout of zero or
/// over five minutes, 408 nobody started speaking in time, 409 someone is already listening, 502 speech
/// recognizer, 503 input device. The body is read whatever its content type, so an empty one is no body
/// and a malformed one answers in the daemon's own shape. A client that stops waiting closes the
/// microphone.
pub async fn listen(State(state): State<AppState>, body: Bytes) -> Response {
    let refused = |status: StatusCode, error: String| (status, Json(json!({"error": error}))).into_response();
    let timeout = match ListenBody::parse(&body) {
        Ok(body) => body.and_then(|body| body.timeout),
        Err(error) => return refused(StatusCode::BAD_REQUEST, error.to_string()),
    };
    match state.listen.listen(timeout).await {
        Ok(heard) => (StatusCode::OK, Json(heard)).into_response(),
        Err(error) => {
            let status = match &error {
                ListenError::Invalid(_) => StatusCode::BAD_REQUEST,
                ListenError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
                ListenError::Busy => StatusCode::CONFLICT,
                ListenError::Hearing(_) => StatusCode::BAD_GATEWAY,
                ListenError::Device(_) => StatusCode::SERVICE_UNAVAILABLE,
            };
            refused(status, error.to_string())
        }
    }
}
