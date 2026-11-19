use axum::Json;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Health {
    status: &'static str,
    version: &'static str,
}

/// Liveness and the daemon's version (its release tag).
pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("E_VOICE_VERSION"),
    })
}
