use std::time::Duration;

use crate::fake::Mic;

#[tokio::test(flavor = "multi_thread")]
async fn test_the_device_opens_and_closes_on_a_thread_of_its_own_the_same_one() {
    let mic = Mic::default();
    let thread = mic.thread();
    let microphone = thread.open().await.unwrap();
    assert_eq!(microphone.rate.get(), 16_000);
    let opened_on = mic.opened_on.lock().unwrap().unwrap();
    assert_ne!(opened_on, std::thread::current().id());
    drop(microphone);
    assert!(mic.freed().await);
    assert_eq!(mic.released_on.lock().unwrap().unwrap(), opened_on);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_blocks_past_the_bound_are_dropped_counted_and_the_first_ones_arrive_in_order() {
    let mic = Mic::default();
    let mut microphone = mic.thread().open().await.unwrap();
    for n in 0..100u8 {
        mic.block(&[f32::from(n)]);
    }
    assert_eq!(microphone.dropped(), 36);
    for n in 0..64u8 {
        assert_eq!(microphone.recv().await, Some(vec![f32::from(n)]));
    }
    mic.block(&[999.0]);
    assert_eq!(microphone.recv().await, Some(vec![999.0]));
    assert_eq!(microphone.dropped(), 36);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_microphone_gone_reads_as_none_after_its_buffered_blocks() {
    let mic = Mic::default();
    let mut microphone = mic.thread().open().await.unwrap();
    mic.block(&[1.0]);
    mic.die();
    assert_eq!(microphone.recv().await, Some(vec![1.0]));
    assert_eq!(microphone.recv().await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_each_open_is_its_own_thread_and_device() {
    let mic = Mic::default();
    let thread = mic.thread();
    let first = thread.open().await.unwrap();
    let second = thread.open().await.unwrap();
    assert_eq!(mic.opened(), 2);
    drop(first);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(mic.is_open());
    drop(second);
    assert!(mic.freed().await);
}
