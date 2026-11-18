use std::time::Duration;

use e_voice_daemon::api::server::{Server, ServerError};
use e_voice_daemon::api::state::AppState;
use e_voice_daemon::config::daemon::DaemonConfig;
use e_voice_daemon::core::settings::Settings;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::http::{free_port, post_json, post_typed, request, wait_ready};

/// A daemon serving `state` on a free loopback port.
pub struct Running {
    pub port: u16,
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl Running {
    pub async fn start(state: AppState) -> Self {
        let port = free_port();
        let settings = Settings {
            daemon: DaemonConfig {
                port,
                ..DaemonConfig::default()
            },
            ..Settings::default()
        };
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            Server::serve(&settings, state, async {
                stopped.await.ok();
            })
            .await
        });
        tokio::task::spawn_blocking(move || wait_ready(port)).await.unwrap();
        Self { port, stop, task }
    }

    pub async fn get(&self, path: &'static str) -> (u16, String) {
        let port = self.port;
        tokio::task::spawn_blocking(move || request(port, "GET", path))
            .await
            .unwrap()
    }

    pub async fn send(&self, method: &'static str, path: &'static str) -> (u16, String) {
        let port = self.port;
        tokio::task::spawn_blocking(move || request(port, method, path))
            .await
            .unwrap()
    }

    pub async fn speak(&self, body: &str) -> (u16, String) {
        let (port, body) = (self.port, body.to_owned());
        tokio::task::spawn_blocking(move || post_json(port, "/speak", &body))
            .await
            .unwrap()
    }

    pub async fn listen(&self, content_type: Option<&'static str>, body: Option<&str>) -> (u16, String) {
        let (port, body) = (self.port, body.map(str::to_owned));
        tokio::task::spawn_blocking(move || post_typed(port, "/listen", content_type, body.as_deref()))
            .await
            .unwrap()
    }

    pub async fn stop(self) {
        self.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
