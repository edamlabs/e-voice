use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::Router;
use axum::routing::get;
use tokio::net::TcpListener;

use crate::api::health::health;
use crate::core::settings::Settings;

/// The daemon owns the audio devices: it listens on loopback only, never on `[server] host`.
const HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

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
    pub fn router() -> Router {
        Router::new().route("/health", get(health))
    }

    /// Serves until `shutdown` resolves, then stops accepting and lets requests in flight finish.
    ///
    /// # Errors
    /// The address cannot be bound, or serving fails.
    pub async fn serve(
        settings: &Settings,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerError> {
        let address = SocketAddr::new(HOST, settings.daemon.port);
        let listener = TcpListener::bind(address)
            .await
            .map_err(|source| ServerError::Bind { address, source })?;
        tracing::info!(%address, "server.listening");
        axum::serve(listener, Self::router())
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServerError::Serve)
    }
}
