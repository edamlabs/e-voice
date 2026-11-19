use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use e_voice_core::schema::lang::Lang;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::state::AppState;
use crate::speech::script::VoiceScript;
use crate::workflow::record::{LONGEST, RecordError, SILENCE, Take};

/// `POST /voices` body: `id` is the voice to learn; `text` what is read, else the reading script of `lang`
/// when one is named, else the recognizer's transcript; `silence` (`"2.5s"`) after speech ends the
/// recording, `max` (`"30s"`, at most 30 s) bounds the clip; `replace` lets an existing voice be
/// overwritten.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceBody {
    id: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    lang: Option<Lang>,
    #[serde(default, with = "humantime_serde")]
    silence: Option<Duration>,
    #[serde(default, with = "humantime_serde")]
    max: Option<Duration>,
    #[serde(default)]
    replace: bool,
}

impl VoiceBody {
    /// Read through a map, because serde's derive would also take a sequence.
    fn parse(body: &[u8]) -> Result<Self, serde_json::Error> {
        let object: Map<String, Value> = serde_json::from_slice(body)?;
        serde_json::from_value(Value::Object(object))
    }

    fn take(self) -> Take {
        let script = self.lang.map(|lang| VoiceScript::Reading.text(lang).to_owned());
        Take {
            id: self.id,
            text: self.text.filter(|text| !text.trim().is_empty()).or(script),
            silence: self.silence.unwrap_or(SILENCE),
            max: self.max.unwrap_or(LONGEST),
            replace: self.replace,
        }
    }
}

/// `GET /voices/script` query: the language of the script, `[daemon] lang` when absent.
#[derive(Debug, Deserialize)]
pub struct ScriptQuery {
    lang: Option<Lang>,
}

fn refused(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({"error": error}))).into_response()
}

/// Records a voice from the microphone and has the speech service learn it: 200 `{"id", "seconds",
/// "text", "transcript"}`. The recording ends on the first of `silence` after speech, `max` of clip, or
/// `POST /voices/finish`; `POST /stop` or a client that stops waiting cancels it, and nothing is learned.
/// A failure answers `{"error"}`: 400 a body that is not a take, 408 nobody spoke, 409 the microphone is
/// held, the voice exists without `replace`, or the recording was stopped, 500 the clip could not be
/// resampled, 502 speech recognizer or speech service (its refusal), 503 input device.
pub async fn create(State(state): State<AppState>, body: Bytes) -> Response {
    let take = match VoiceBody::parse(&body) {
        Ok(body) => body.take(),
        Err(error) => return refused(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    match state.record.record(take).await {
        Ok(learned) => (StatusCode::OK, Json(learned)).into_response(),
        Err(error) => refused(error.status(), &error.to_string()),
    }
}

/// Ends the recording running and keeps what it has: 200 `{"finished": true}`, `false` when none runs.
pub async fn finish(State(state): State<AppState>) -> Response {
    (StatusCode::OK, Json(json!({"finished": state.record.finish()}))).into_response()
}

/// The text to read to record a voice, and the sentence to hear it with once learned: 200 `{"lang",
/// "text", "sample"}`; 400 `{"error"}` for a language the daemon has no script in.
pub async fn script(State(state): State<AppState>, query: Result<Query<ScriptQuery>, QueryRejection>) -> Response {
    let lang = match query {
        Ok(Query(query)) => query.lang.unwrap_or(state.lang),
        Err(error) => return refused(StatusCode::BAD_REQUEST, &error.body_text()),
    };
    let body = json!({
        "lang": lang,
        "text": VoiceScript::Reading.text(lang),
        "sample": VoiceScript::Sample.text(lang),
    });
    (StatusCode::OK, Json(body)).into_response()
}

impl RecordError {
    /// The HTTP status of each failure.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Timeout(_) | Self::Unheard => StatusCode::REQUEST_TIMEOUT,
            Self::Busy | Self::Exists(_) | Self::Cancelled => StatusCode::CONFLICT,
            Self::Audio(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Hearing(_) | Self::Speech(_) => StatusCode::BAD_GATEWAY,
            Self::Device(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}
