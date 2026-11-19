use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use e_voice_device::error::DeviceError;

use crate::fake::{Device, Ev, events, rate};

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_nothing_opens_at_spawn() {
    let device = Device::new(0, 0);
    let _audio = device.thread();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(device.opened(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_open_opens_once_however_often_it_is_asked() {
    let device = Device::new(0, 0);
    let audio = device.thread();
    audio.open().await.unwrap();
    audio.open().await.unwrap();
    audio.open().await.unwrap();
    assert_eq!(device.opened(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failed_open_is_returned_and_tried_again_never_cached() {
    let device = Device::new(0, 2);
    let audio = device.thread();
    assert!(matches!(audio.open().await, Err(DeviceError::Output)));
    assert!(matches!(audio.open().await, Err(DeviceError::Output)));
    audio.open().await.unwrap();
    assert_eq!(device.opened(), 3);
    audio.open().await.unwrap();
    assert_eq!(device.opened(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pushes_are_dropped_while_the_device_is_not_open() {
    let device = Device::new(0, 0);
    let audio = device.thread();
    audio.push(vec![0.5; 8], rate());
    assert_eq!(audio.pending().await, 0.0);
    audio.open().await.unwrap();
    assert_eq!(audio.pending().await, 0.0);
    assert!(
        events(&device.log)
            .iter()
            .all(|event| !matches!(event, Ev::Push { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_push_after_open_reaches_the_device_with_its_rate() {
    let device = Device::new(0, 0);
    let audio = device.thread();
    audio.open().await.unwrap();
    audio.push(vec![0.25; 5], rate());
    audio.pending().await;
    assert_eq!(
        events(&device.log)[0],
        Ev::Push {
            rate: 24_000,
            len: 5,
            first: 0.25
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pending_is_what_the_device_reports_once_open() {
    let device = Device::new(2, 0);
    let audio = device.thread();
    audio.open().await.unwrap();
    audio.push(vec![0.1], rate());
    assert!(audio.pending().await > 0.0);
    assert!(audio.pending().await > 0.0);
    assert_eq!(audio.pending().await, 0.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dropping_the_handle_ends_the_thread_and_closes_the_device() {
    let device = Device::new(0, 0);
    let audio = device.thread();
    audio.open().await.unwrap();
    assert!(!device.dropped.load(Ordering::SeqCst));
    drop(audio);
    until("the device to be dropped", || device.dropped.load(Ordering::SeqCst)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_clear_is_a_no_op_while_the_device_is_not_open() {
    let device = Device::new(0, 0);
    let audio = device.thread();
    audio.clear();
    assert_eq!(audio.pending().await, 0.0);
    assert_eq!(device.opened(), 0);
    assert_eq!(events(&device.log), Vec::<Ev>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_clear_after_open_reaches_the_device_and_drops_what_it_held() {
    let device = Device::new(5, 0);
    let audio = device.thread();
    audio.open().await.unwrap();
    audio.push(vec![0.1; 4], rate());
    audio.clear();
    assert_eq!(audio.pending().await, 0.0);
    let log = events(&device.log);
    assert_eq!(log[1], Ev::Clear, "{log:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_finish_reaches_the_device_when_open_and_is_a_no_op_while_closed() {
    let device = Device::new(0, 0);
    let audio = device.thread();
    audio.finish();
    assert_eq!(audio.pending().await, 0.0);
    assert_eq!(events(&device.log), Vec::<Ev>::new());
    audio.open().await.unwrap();
    audio.finish();
    assert_eq!(audio.pending().await, 0.0);
    assert_eq!(events(&device.log)[0], Ev::Finish);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_push_the_device_refuses_is_logged_and_the_thread_goes_on() {
    let mut device = Device::new(0, 0);
    device.fail_pushes = true;
    let audio = device.thread();
    audio.open().await.unwrap();
    audio.push(vec![0.1; 4], rate());
    audio.push(vec![0.2; 4], rate());
    assert_eq!(audio.pending().await, 0.0);
    let pushes = events(&device.log)
        .iter()
        .filter(|event| matches!(event, Ev::Push { .. }))
        .count();
    assert_eq!(pushes, 2);
}
