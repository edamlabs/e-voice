use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::api::state::AppState;

/// Silences what is playing and drops what waits: 200 `{"stopped": id | null, "dropped": n}`, the same
/// shape while idle. 500 `{"error"}` when the speech queue is gone.
pub async fn stop(State(state): State<AppState>) -> Response {
    match state.speech.stop().await {
        Ok(halted) => (StatusCode::OK, Json(halted)).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}
