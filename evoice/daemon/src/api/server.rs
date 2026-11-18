use std::future::Future;
use std::net::SocketAddr;

use axum::Router;
use axum::routing::{get, post};
use tokio::net::TcpListener;

use crate::api::health::health;
use crate::api::speak::speak;
use crate::api::state::AppState;
use crate::core::settings::Settings;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("cannot bind {address}: {source}")]
    Bind {
        address: SocketAddr,
        source: std::io::Error,
    },
    #[error("cannot serve: {0}")]
    Serve(std::io::Error),
}

/// The local gateway every voice client talks to.
#[derive(Debug)]
pub struct Server;

impl Server {
    pub fn router(state: AppState) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/speak", post(speak))
            .with_state(state)
    }

    /// Serves on `[daemon] host` (loopback by default, never `[server] host`) until `shutdown` resolves,
    /// then stops accepting and lets requests in flight finish.
    ///
    /// # Errors
    /// The address cannot be bound, or serving fails.
    pub async fn serve(
        settings: &Settings,
        state: AppState,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerError> {
        let address = SocketAddr::new(settings.daemon.host, settings.daemon.port);
        let listener = TcpListener::bind(address)
            .await
            .map_err(|source| ServerError::Bind { address, source })?;
        tracing::info!(%address, "server.listening");
        axum::serve(listener, Self::router(state))
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServerError::Serve)
    }
}
