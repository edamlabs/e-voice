use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::api::state::AppState;

/// Silences what is playing and drops what waits — everything, the voice conversation's reply and any
/// other client's queued texts alike: 200 `{"stopped": id | null, "dropped": n}`, the same shape while
/// idle. A voice conversation that is thinking, speaking or asking also drops the agent's turn and listens
/// again; one that is listening goes on listening. 500 `{"error"}` when the speech queue is gone. The
/// sequence is [`Talk::stop`]'s, shared with the MCP `stop` tool.
pub async fn stop(State(state): State<AppState>) -> Response {
    match state.talk.stop().await {
        Ok(halted) => (StatusCode::OK, Json(halted)).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}
