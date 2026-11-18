use std::time::{Duration, Instant};

use e_voice_daemon::workflow::queue::{Halted, Outcome, SpeakError, SpeechState, TEXT};

use crate::fake::{Device, Ev, counting, events, queue, say};

const FRAME_SECONDS: f64 = 0.25;
const FRAME: usize = 6_000;
const AHEAD: f64 = 2.0;

fn realtime() -> Device {
    let mut device = Device::new(0, 0);
    device.realtime = true;
    device
}

fn pushes(device: &Device) -> usize {
    events(&device.log)
        .iter()
        .filter(|event| matches!(event, Ev::Push { .. }))
        .count()
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_reader_never_holds_more_than_the_lead_ahead_of_the_sink() {
    let device = realtime();
    let speech = queue(counting(24, FRAME), &device);
    let started = Instant::now();
    assert_eq!(speech.say(say("x")).await.unwrap(), (0, Outcome::Done));
    let took = started.elapsed();
    let peak = *device.max_pending.lock().unwrap();
    assert!(
        peak <= AHEAD + FRAME_SECONDS + 1e-3,
        "the device held {peak:.3} s at once, more than {AHEAD} s plus one frame"
    );
    assert!(
        peak >= AHEAD,
        "the reader never filled the lead ({peak:.3} s): the test proves nothing"
    );
    assert!(
        took >= Duration::from_millis(5_900),
        "6 s of audio were reported done after {took:?}: the say did not wait for the device to play it all"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_under_backpressure_every_sample_reaches_the_sink_exactly_once_in_order() {
    let device = realtime();
    let speech = queue(counting(24, FRAME), &device);
    assert_eq!(speech.say(say("x")).await.unwrap().1, Outcome::Done);
    let received = device.received.lock().unwrap().clone();
    assert_eq!(received.len(), 24 * FRAME);
    assert!(
        received
            .iter()
            .enumerate()
            .all(|(index, value)| (*value - index as f32).abs() < f32::EPSILON),
        "samples were dropped, repeated or reordered"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_inside_the_backpressure_wait_cuts_within_one_frame() {
    let device = realtime();
    let speech = queue(counting(1_000, FRAME), &device);
    let talking = tokio::spawn({
        let speech = speech.clone();
        async move { speech.say(say("x")).await }
    });
    until("the lead to fill", || pushes(&device) >= 9).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = pushes(&device);
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
        .expect("never cleared")
        .0;
    assert!(
        clear <= returned + Duration::from_millis(80),
        "clear came {:?} after stop returned",
        clear - returned
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after_clear = device
        .stamps
        .lock()
        .unwrap()
        .iter()
        .filter(|(at, event)| *at > clear && matches!(event, Ev::Push { .. }))
        .count();
    assert_eq!(after_clear, 0, "a frame was pushed after the clear");
    assert!(
        pushes(&device) <= before + 1,
        "{} frames, {before} before the stop",
        pushes(&device)
    );
    assert!(
        pushes(&device) < 20,
        "the reader ran ahead: {} frames pushed of 1000",
        pushes(&device)
    );
    assert_eq!(speech.state(), SpeechState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_text_of_4096_characters_is_taken_and_one_more_is_refused_counting_characters() {
    let device = Device::new(0, 0);
    let speech = queue(crate::fake::FakeSpeech::marked(), &device);
    for letter in ["a", "ñ", "語"] {
        let taken = speech.say(say(&letter.repeat(TEXT))).await;
        assert!(taken.is_ok(), "{TEXT} × {letter:?}: {taken:?}");
        let refused = speech.say(say(&letter.repeat(TEXT + 1))).await;
        assert!(
            matches!(refused, Err(SpeakError::TooLong(length)) if length == TEXT + 1),
            "{letter:?}: {refused:?}"
        );
        assert_eq!(
            refused.unwrap_err().to_string(),
            format!("text is too long: {} characters, at most 4096", TEXT + 1)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_refused_long_text_burns_no_id_and_reaches_no_service() {
    let device = Device::new(0, 0);
    let fake = crate::fake::FakeSpeech::marked();
    let seen = fake.requests.clone();
    let speech = queue(fake, &device);
    assert!(speech.say(say(&"a".repeat(TEXT + 1))).await.is_err());
    assert_eq!(seen.lock().unwrap().len(), 0);
    assert_eq!(speech.say(say("1")).await.unwrap(), (0, Outcome::Done));
}
