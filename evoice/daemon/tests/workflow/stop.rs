use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use e_voice_daemon::schema::event::SpeechState;
use e_voice_daemon::speech::base::SpeechError;
use e_voice_daemon::workflow::queue::{Halted, Outcome, SpeakError, SpeechQueue};

use crate::fake::{Device, Ev, FakeSpeech, cleared, events, long, queue, say};

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn pushes(device: &Device) -> usize {
    events(&device.log)
        .iter()
        .filter(|event| matches!(event, Ev::Push { .. }))
        .count()
}

fn say_in_background(speech: &SpeechQueue, text: &str) -> tokio::task::JoinHandle<Result<(u64, Outcome), SpeakError>> {
    let (speech, text) = (speech.clone(), text.to_owned());
    tokio::spawn(async move { speech.say(say(&text)).await })
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_idle_does_nothing_and_says_so_twice() {
    let device = Device::new(0, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    for _ in 0..2 {
        let halted = speech.stop().await.unwrap();
        assert_eq!(
            halted,
            Halted {
                stopped: None,
                dropped: 0
            }
        );
    }
    assert_eq!(speech.state(), SpeechState::Idle);
    assert_eq!(device.opened(), 0);
    assert_eq!(speech.say(say("1")).await.unwrap(), (0, Outcome::Done));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_speaking_cuts_the_service_the_device_and_the_say() {
    let device = Device::new(0, 0);
    let fake = long(50, 7.0);
    let cancels = fake.cancels.clone();
    let speech = queue(fake, &device);
    let talking = say_in_background(&speech, "x");
    until("the first frame", || pushes(&device) >= 1).await;
    let halted = speech.stop().await.unwrap();
    let returned = Instant::now();
    assert_eq!(
        halted,
        Halted {
            stopped: Some(0),
            dropped: 0
        }
    );
    assert_eq!(talking.await.unwrap().unwrap(), (0, Outcome::Cancelled));
    assert_eq!(cancels.load(Ordering::SeqCst), 1);
    assert_eq!(speech.state(), SpeechState::Idle);
    while returned.elapsed() < Duration::from_millis(80)
        && !device
            .stamps
            .lock()
            .unwrap()
            .iter()
            .any(|(_, event)| *event == Ev::Clear)
    {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let stamps = device.stamps.lock().unwrap().clone();
    let clear = stamps
        .iter()
        .find(|(_, event)| *event == Ev::Clear)
        .expect("the device was not cleared within one frame of the stop")
        .0;
    assert!(
        clear <= returned + Duration::from_millis(80),
        "clear came {:?} after stop returned",
        clear - returned
    );
    let before = pushes(&device);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(pushes(&device), before, "a frame was pushed after the stop");
    let after_clear = stamps
        .iter()
        .filter(|(at, event)| *at > clear && matches!(event, Ev::Push { .. }))
        .count();
    assert_eq!(after_clear, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_draining_clears_the_device_without_telling_the_service() {
    let device = Device::new(500, 0);
    let fake = FakeSpeech::marked();
    let cancels = fake.cancels.clone();
    let speech = queue(fake, &device);
    let talking = say_in_background(&speech, "3");
    until("draining", || matches!(speech.state(), SpeechState::Draining { id: 0 })).await;
    let halted = speech.stop().await.unwrap();
    assert_eq!(
        halted,
        Halted {
            stopped: Some(0),
            dropped: 0
        }
    );
    assert_eq!(talking.await.unwrap().unwrap(), (0, Outcome::Cancelled));
    assert_eq!(cancels.load(Ordering::SeqCst), 0);
    assert!(cleared(&device).await, "the device was never cleared");
    assert_eq!(speech.state(), SpeechState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_drops_what_waits_and_answers_each_with_the_next_id() {
    let device = Device::new(0, 0);
    let fake = long(50, 1.0);
    let seen = fake.requests.clone();
    let speech = queue(fake, &device);
    let first = say_in_background(&speech, "a");
    until("speaking", || speech.state() == (SpeechState::Speaking { id: 0 })).await;
    let waiting: Vec<_> = ["b", "c", "d"]
        .iter()
        .map(|text| say_in_background(&speech, text))
        .collect();
    tokio::time::sleep(Duration::from_millis(40)).await;
    let halted = speech.stop().await.unwrap();
    assert_eq!(
        halted,
        Halted {
            stopped: Some(0),
            dropped: 3
        }
    );
    assert_eq!(first.await.unwrap().unwrap(), (0, Outcome::Cancelled));
    let mut ids = Vec::new();
    for task in waiting {
        let (id, outcome) = task.await.unwrap().unwrap();
        assert_eq!(outcome, Outcome::Cancelled);
        ids.push(id);
    }
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "a dropped text must never reach the service"
    );
    assert_eq!(speech.say(say("e")).await.unwrap(), (4, Outcome::Done));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failed_cancel_is_logged_not_fatal_and_the_device_is_still_cleared() {
    let device = Device::new(0, 0);
    let mut fake = long(50, 1.0);
    fake.cancel_error = Some(SpeechError::Protocol("socket gone".to_owned()));
    let cancels = fake.cancels.clone();
    let speech = queue(fake, &device);
    let talking = say_in_background(&speech, "x");
    until("the first frame", || pushes(&device) >= 1).await;
    let halted = speech.stop().await.unwrap();
    assert_eq!(halted.stopped, Some(0));
    assert_eq!(talking.await.unwrap().unwrap(), (0, Outcome::Cancelled));
    assert_eq!(cancels.load(Ordering::SeqCst), 1);
    assert!(cleared(&device).await, "the device was never cleared");
    assert_eq!(speech.state(), SpeechState::Idle);
    assert!(speech.say(say("again")).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancelling_is_observable_while_the_service_is_told_to_stop() {
    let device = Device::new(0, 0);
    let mut fake = long(50, 1.0);
    fake.cancel_delay = Duration::from_millis(200);
    let speech = queue(fake, &device);
    let talking = say_in_background(&speech, "x");
    until("the first frame", || pushes(&device) >= 1).await;
    let stopping = tokio::spawn({
        let speech = speech.clone();
        async move { speech.stop().await }
    });
    until("cancelling", || speech.state() == (SpeechState::Cancelling { id: 0 })).await;
    assert!(stopping.await.unwrap().is_ok());
    talking.await.unwrap().unwrap();
    assert_eq!(speech.state(), SpeechState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_more_stops_than_the_control_channel_holds_are_all_answered() {
    let device = Device::new(0, 0);
    let speech = queue(long(50, 1.0), &device);
    let talking = say_in_background(&speech, "x");
    until("the first frame", || pushes(&device) >= 1).await;
    let stops: Vec<_> = (0..12)
        .map(|_| {
            let speech = speech.clone();
            tokio::spawn(async move { speech.stop().await })
        })
        .collect();
    let mut stopped = 0;
    for stop in stops {
        let halted = stop.await.unwrap().unwrap();
        stopped += usize::from(halted.stopped.is_some());
    }
    assert_eq!(stopped, 1, "one stop cuts the text, the others find it idle");
    assert_eq!(talking.await.unwrap().unwrap().1, Outcome::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_while_the_service_is_still_connecting_does_not_hang() {
    let device = Device::new(0, 0);
    let (fake, _gate) = FakeSpeech::marked().gated();
    let speech = queue(fake, &device);
    let talking = say_in_background(&speech, "1");
    until("speaking", || speech.state() == (SpeechState::Speaking { id: 0 })).await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    let halted = tokio::time::timeout(Duration::from_secs(2), speech.stop()).await;
    assert!(
        halted.is_ok(),
        "a stop waited on a service that never answered its connect"
    );
    drop(talking);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_while_the_device_is_still_opening_does_not_hang() {
    let device = Device::new(0, 0);
    device.hold_open.store(true, Ordering::SeqCst);
    let fake = FakeSpeech::marked();
    let seen = fake.requests.clone();
    let speech = queue(fake, &device);
    let talking = say_in_background(&speech, "1");
    until("the device open to start", || device.opened() == 1).await;
    let halted = tokio::time::timeout(Duration::from_secs(2), speech.stop()).await;
    device.hold_open.store(false, Ordering::SeqCst);
    let halted = halted
        .expect("a stop waited on a device that never finished opening")
        .unwrap();
    assert_eq!(
        halted,
        Halted {
            stopped: Some(0),
            dropped: 0
        }
    );
    assert_eq!(talking.await.unwrap().unwrap(), (0, Outcome::Cancelled));
    assert!(
        seen.lock().unwrap().is_empty(),
        "the service was asked for audio nobody would hear"
    );
    assert_eq!(speech.state(), SpeechState::Idle);
}
