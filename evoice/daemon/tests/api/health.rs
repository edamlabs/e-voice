use e_voice_daemon::api::state::AppState;

use crate::fake::{Device, FakeSpeech, queue};
use crate::running::Running;

fn state(device: &Device) -> AppState {
    crate::fake::state(
        queue(FakeSpeech::marked(), device),
        crate::fake::idle(),
        crate::fake::bus(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn test_health_answers_ok_with_the_release_version() {
    let daemon = Running::start(state(&Device::new(0, 0))).await;
    let (status, body) = daemon.get("/health").await;
    daemon.stop().await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        format!(r#"{{"status":"ok","version":"{}"}}"#, env!("E_VOICE_VERSION"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_health_needs_no_output_device() {
    let device = Device::new(0, usize::MAX);
    let daemon = Running::start(state(&device)).await;
    let (status, _) = daemon.get("/health").await;
    daemon.stop().await;
    assert_eq!(status, 200);
    assert_eq!(device.opened(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unknown_route_is_404_and_a_post_to_health_is_405() {
    let daemon = Running::start(state(&Device::new(0, 0))).await;
    let not_found = daemon.get("/nope").await.0;
    let wrong_method = daemon.send("POST", "/health").await.0;
    let get_speak = daemon.get("/speak").await.0;
    daemon.stop().await;
    assert_eq!((not_found, wrong_method, get_speak), (404, 405, 405));
}
