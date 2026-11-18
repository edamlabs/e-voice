use std::net::{Ipv4Addr, TcpListener};
use std::time::Duration;

use e_voice_core::config::server::ServerConfig;
use e_voice_daemon::api::server::{Server, ServerError};
use e_voice_daemon::api::state::AppState;
use e_voice_daemon::config::daemon::DaemonConfig;
use e_voice_daemon::core::settings::Settings;
use tokio::sync::oneshot;

use crate::fake::{Device, FakeSpeech, queue};
use crate::http::{free_port, request, wait_ready};

fn state() -> AppState {
    AppState {
        speech: queue(FakeSpeech::marked(), &Device::new(0, 0)),
        listen: crate::fake::idle(),
    }
}

fn on(port: u16) -> Settings {
    Settings {
        daemon: DaemonConfig {
            port,
            ..DaemonConfig::default()
        },
        ..Settings::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_serve_stops_cleanly_when_shutdown_fires() {
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        Server::serve(&on(0), state(), async {
            stopped.await.ok();
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(()).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_serve_answers_on_the_configured_port_then_stops() {
    let port = free_port();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        Server::serve(&on(port), state(), async {
            stopped.await.ok();
        })
        .await
    });
    let (status, _) = tokio::task::spawn_blocking(move || {
        wait_ready(port);
        request(port, "GET", "/health")
    })
    .await
    .unwrap();
    assert_eq!(status, 200);
    stop.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_serve_on_a_held_port_fails_naming_the_loopback_address() {
    let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let error = Server::serve(&on(port), state(), std::future::pending())
        .await
        .unwrap_err();
    assert!(matches!(error, ServerError::Bind { .. }), "{error:?}");
    assert!(
        error
            .to_string()
            .starts_with(&format!("cannot bind 127.0.0.1:{port}: ")),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_serve_binds_the_daemon_host_never_the_shared_one() {
    let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let settings = Settings {
        server: ServerConfig {
            host: Ipv4Addr::UNSPECIFIED.into(),
            ..ServerConfig::default()
        },
        ..on(port)
    };
    let error = Server::serve(&settings, state(), std::future::pending())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with(&format!("cannot bind 127.0.0.1:{port}: ")),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_serve_binds_the_host_the_daemon_table_names() {
    let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let settings = Settings {
        daemon: DaemonConfig {
            host: Ipv4Addr::UNSPECIFIED.into(),
            port,
            ..DaemonConfig::default()
        },
        ..Settings::default()
    };
    let error = Server::serve(&settings, state(), std::future::pending())
        .await
        .unwrap_err();
    assert!(
        error.to_string().starts_with(&format!("cannot bind 0.0.0.0:{port}: ")),
        "{error}"
    );
}
