use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::api::state::AppState;
use crate::schema::event::TalkState;

/// `POST /talk`: starts a voice conversation with the agent, listening at once: 200 `{"state":
/// "listening"}`. 409 `{"error"}` while a conversation runs or a listen holds the microphone. Each
/// conversation starts a fresh agent session. It is half-duplex: the microphone is closed while the agent
/// thinks or speaks; `POST /stop` cuts a long answer and listens again.
pub async fn start(State(state): State<AppState>) -> Response {
    match state.talk.start().await {
        Ok(talking) => (StatusCode::OK, Json(json!({"state": talking}))).into_response(),
        Err(error) => {
            let body = Json(json!({"error": error.to_string()}));
            (StatusCode::CONFLICT, body).into_response()
        }
    }
}

/// `DELETE /talk`: ends the voice conversation, if one runs: the speech is stopped, the agent's turn
/// cancelled and its session closed. 200 `{"state": "idle"}`, with or without a conversation. The
/// conversation ends first and the speech is stopped after: a stop first would make room for a reply
/// still waiting to be queued.
pub async fn end(State(state): State<AppState>) -> Response {
    state.talk.end().await;
    state.speech.stop().await.ok();
    (StatusCode::OK, Json(json!({"state": TalkState::Idle}))).into_response()
}
