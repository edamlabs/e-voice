use std::time::Duration;

use e_voice_daemon::api::server::Server;
use e_voice_daemon::config::daemon::DaemonConfig;
use e_voice_daemon::core::settings::Settings;
use tokio::sync::oneshot;

use crate::http::{free_port, request, wait_ready};

async fn with_daemon<T: Send + 'static>(probe: impl FnOnce(u16) -> T + Send + 'static) -> T {
    let port = free_port();
    let (stop, stopped) = oneshot::channel::<()>();
    let settings = Settings {
        daemon: DaemonConfig { port },
        ..Settings::default()
    };
    let task = tokio::spawn(async move {
        Server::serve(&settings, async {
            stopped.await.ok();
        })
        .await
    });
    let result = tokio::task::spawn_blocking(move || {
        wait_ready(port);
        probe(port)
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    result
}

#[tokio::test(flavor = "multi_thread")]
async fn test_health_answers_ok_with_the_release_version() {
    let (status, body) = with_daemon(|port| request(port, "GET", "/health")).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        format!(r#"{{"status":"ok","version":"{}"}}"#, env!("E_VOICE_VERSION"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_route_is_404_and_a_post_to_health_is_405() {
    let (not_found, wrong_method) =
        with_daemon(|port| (request(port, "GET", "/nope").0, request(port, "POST", "/health").0)).await;
    assert_eq!((not_found, wrong_method), (404, 405));
}
