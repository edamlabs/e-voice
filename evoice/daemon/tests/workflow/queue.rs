use std::sync::Arc;
use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::speech::base::SpeechError;
use e_voice_daemon::workflow::queue::{BACKLOG, Outcome, SpeakError, SpeechQueue, SpeechState};
use e_voice_device::error::DeviceError;
use tokio::sync::mpsc;

use crate::fake::{Device, Ev, FakeSpeech, events, frame, queue, same, say};

async fn settle() {
    tokio::time::sleep(Duration::from_millis(60)).await;
}

async fn until_speaking(speech: &SpeechQueue, id: u64) {
    while speech.state() != (SpeechState::Speaking { id }) {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn pushes(device: &Device) -> Vec<f32> {
    events(&device.log)
        .into_iter()
        .filter_map(|event| match event {
            Ev::Push { first, .. } => Some(first),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_say_resolves_only_after_the_device_has_played_everything() {
    let device = Device::new(3, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    let watcher = speech.clone();
    let seen = tokio::spawn(async move {
        let mut states = Vec::new();
        for _ in 0..400 {
            let now = watcher.state();
            if states.last() != Some(&now) {
                states.push(now);
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        states
    });
    let started = Instant::now();
    let (id, outcome) = speech.say(say("7")).await.unwrap();
    assert_eq!(outcome, Outcome::Done);
    let took = started.elapsed();
    assert_eq!(id, 0);
    // Each lead check before a frame also asks the fake device how much it holds, which uses up one of its three
    // polls, so the drain itself sleeps at least once more.
    assert!(
        took >= Duration::from_millis(20),
        "the drain did not wait, took {took:?}"
    );
    let log = events(&device.log);
    assert_eq!(log.last(), Some(&Ev::Silent), "{log:?}");
    assert!(
        log.iter().filter(|event| **event == Ev::Playing).count() >= 3,
        "{log:?}"
    );
    let states = seen.await.unwrap();
    assert!(states.contains(&SpeechState::Draining { id: 0 }), "{states:?}");
    assert_eq!(states.last(), Some(&SpeechState::Idle));
    assert_eq!(speech.state(), SpeechState::Idle);
}

#[test]
fn test_the_drain_poll_lives_only_in_its_own_function() {
    let source = include_str!("../../src/workflow/queue.rs");
    let uses: Vec<&str> = source
        .lines()
        .filter(|line| line.contains("DRAIN") && !line.trim_start().starts_with("//"))
        .collect();
    assert_eq!(uses.len(), 2, "{uses:?}");
    assert!(uses[0].contains("const DRAIN"), "{uses:?}");
    assert!(uses[1].contains("sleep(DRAIN)"), "{uses:?}");
    assert_eq!(source.matches("tokio::time::sleep").count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ids_start_at_zero_and_empty_text_is_refused_without_one() {
    let device = Device::new(0, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    assert!(matches!(speech.say(say("")).await, Err(SpeakError::Empty)));
    assert!(matches!(speech.say(say("  \n\t")).await, Err(SpeakError::Empty)));
    assert_eq!(speech.say(say("1")).await.unwrap(), (0, Outcome::Done));
    assert_eq!(speech.say(say("2")).await.unwrap(), (1, Outcome::Done));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_queue_is_bounded_and_finishes_in_order() {
    let device = Device::new(0, 0);
    let (fake, gate) = FakeSpeech::marked().gated();
    let speech = queue(fake.clone(), &device);
    let (finished, mut order) = mpsc::unbounded_channel();
    let mut waiting = Vec::new();
    for n in 0..=BACKLOG {
        let (handle, finished) = (speech.clone(), finished.clone());
        waiting.push(tokio::spawn(async move {
            let (id, _) = handle.say(say(&n.to_string())).await.unwrap();
            finished.send((n, id)).unwrap();
        }));
        if n == 0 {
            until_speaking(&speech, 0).await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    settle().await;
    let refused = speech.say(say("17")).await;
    assert!(matches!(refused, Err(SpeakError::Full)), "{refused:?}");
    assert_eq!(fake.texts(), ["0"]);
    gate.add_permits(BACKLOG + 1);
    for task in waiting {
        task.await.unwrap();
    }
    drop(finished);
    let mut done = Vec::new();
    while let Some(entry) = order.recv().await {
        done.push(entry);
    }
    assert_eq!(
        done.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        (0..=BACKLOG).collect::<Vec<_>>()
    );
    let ids: Vec<u64> = done.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids, (0..=BACKLOG as u64).collect::<Vec<_>>());
    assert_eq!(fake.texts(), (0..=BACKLOG).map(|n| n.to_string()).collect::<Vec<_>>());
    gate.add_permits(1);
    assert_eq!(
        speech.say(say("after")).await.unwrap(),
        (BACKLOG as u64 + 1, Outcome::Done)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_two_concurrent_says_never_interleave() {
    let device = Device::new(2, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    let (a, b) = tokio::join!(speech.say(say("1")), speech.say(say("2")));
    assert_eq!(a.unwrap().0 + b.unwrap().0, 1);
    let marks = pushes(&device);
    assert_eq!(marks.len(), 6, "{marks:?}");
    assert!(marks[..3].iter().all(|mark| same(*mark, marks[0])), "{marks:?}");
    assert!(marks[3..].iter().all(|mark| same(*mark, marks[3])), "{marks:?}");
    assert_ne!(marks[0], marks[3]);
    let log = events(&device.log);
    let last_of_first = log
        .iter()
        .rposition(|event| matches!(event, Ev::Push { first, .. } if same(*first, marks[0])))
        .unwrap();
    let first_of_second = log
        .iter()
        .position(|event| matches!(event, Ev::Push { first, .. } if same(*first, marks[3])))
        .unwrap();
    assert!(log[last_of_first..first_of_second].contains(&Ev::Silent), "{log:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_say_whose_future_is_dropped_still_plays_to_the_end() {
    let device = Device::new(1, 0);
    let (fake, gate) = FakeSpeech::marked().gated();
    let speech = queue(fake, &device);
    let waiting = tokio::spawn({
        let speech = speech.clone();
        async move { speech.say(say("5")).await }
    });
    until_speaking(&speech, 0).await;
    waiting.abort();
    gate.add_permits(1);
    while speech.state() != SpeechState::Idle || pushes(&device).len() < 3 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(pushes(&device), [5.0, 5.0, 5.0]);
    gate.add_permits(1);
    assert_eq!(speech.say(say("6")).await.unwrap(), (1, Outcome::Done));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failure_midway_still_plays_and_drains_what_was_sent_before_the_next_text() {
    let device = Device::new(2, 0);
    let broken = SpeechError::Server("boom".to_owned());
    let fake = FakeSpeech::new(move |request| {
        if request.text == "bad" {
            vec![Ok(frame(9.0, 4)), Err(broken.clone())]
        } else {
            vec![Ok(frame(3.0, 4))]
        }
    });
    let speech = queue(fake, &device);
    let (first, second) = tokio::join!(speech.say(say("bad")), speech.say(say("good")));
    assert!(
        matches!(first, Err(SpeakError::Speech(SpeechError::Server(ref why))) if why == "boom"),
        "{first:?}"
    );
    assert!(second.is_ok());
    let log = events(&device.log);
    let nine = log
        .iter()
        .position(|event| matches!(event, Ev::Push { first, .. } if same(*first, 9.0)))
        .unwrap();
    let three = log
        .iter()
        .position(|event| matches!(event, Ev::Push { first, .. } if same(*first, 3.0)))
        .unwrap();
    assert!(log[nine..three].contains(&Ev::Silent), "{log:?}");
    assert_eq!(speech.state(), SpeechState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_service_that_cannot_start_is_a_speech_error_and_the_queue_goes_on() {
    let device = Device::new(0, 0);
    let mut fake = FakeSpeech::marked();
    fake.fail_say = Some(SpeechError::Connect("refused".to_owned()));
    let speech = queue(fake, &device);
    let failed = speech.say(say("1")).await;
    assert!(
        matches!(failed, Err(SpeakError::Speech(SpeechError::Connect(_)))),
        "{failed:?}"
    );
    assert_eq!(speech.state(), SpeechState::Idle);
    assert!(matches!(speech.say(say("2")).await, Err(SpeakError::Speech(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_device_that_cannot_open_is_a_device_error_retried_on_the_next_say() {
    let device = Device::new(0, 1);
    let speech = queue(FakeSpeech::marked(), &device);
    let first = speech.say(say("1")).await;
    assert!(
        matches!(first, Err(SpeakError::Device(DeviceError::Output))),
        "{first:?}"
    );
    assert_eq!(speech.say(say("2")).await.unwrap(), (1, Outcome::Done));
    assert_eq!(device.opened(), 2);
    assert_eq!(pushes(&device), [2.0, 2.0, 2.0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_dead_queue_task_reads_as_stopped() {
    let device = Device::new(0, 0);
    let mut fake = FakeSpeech::marked();
    fake.panic_say = true;
    let speech = queue(fake, &device);
    let first = speech.say(say("1")).await;
    assert!(matches!(first, Err(SpeakError::Stopped)), "{first:?}");
    settle().await;
    let later = speech.say(say("2")).await;
    assert!(matches!(later, Err(SpeakError::Stopped)), "{later:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_voice_and_lang_travel_with_the_request() {
    let device = Device::new(0, 0);
    let fake = FakeSpeech::marked();
    let seen = Arc::clone(&fake.requests);
    let speech = queue(fake, &device);
    let mut request = say("1");
    request.voice = Some("ef_dora".to_owned());
    request.lang = Some(Lang::Es);
    speech.say(request.clone()).await.unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), [request]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_queue_prints_its_state() {
    let device = Device::new(0, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    assert!(format!("{speech:?}").contains("Idle"));
    let (fake, _gate) = FakeSpeech::marked().gated();
    let busy = queue(fake, &device);
    let waiting = tokio::spawn({
        let busy = busy.clone();
        async move { busy.say(say("1")).await }
    });
    until_speaking(&busy, 0).await;
    assert!(format!("{busy:?}").contains("Speaking"));
    waiting.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stream_ending_finishes_the_device_and_the_drain_waits_for_the_tail() {
    let mut device = Device::new(1, 0);
    device.tail_polls = 3;
    let speech = queue(FakeSpeech::marked(), &device);
    let started = Instant::now();
    assert_eq!(speech.say(say("5")).await.unwrap(), (0, Outcome::Done));
    assert!(
        started.elapsed() >= Duration::from_millis(60),
        "three drain sleeps of 20 ms (the lead check takes one of the four polls), took {:?}",
        started.elapsed()
    );
    let log = events(&device.log);
    let finish = log
        .iter()
        .position(|event| *event == Ev::Finish)
        .expect("the stream was never finished");
    let last_push = log.iter().rposition(|event| matches!(event, Ev::Push { .. })).unwrap();
    assert!(last_push < finish, "{log:?}");
    assert_eq!(
        log[finish + 1..].iter().filter(|event| **event == Ev::Playing).count(),
        3,
        "{log:?}"
    );
    assert_eq!(log.last(), Some(&Ev::Silent));
    assert_eq!(log.iter().filter(|event| **event == Ev::Finish).count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stream_that_fails_midway_is_finished_too() {
    let device = Device::new(0, 0);
    let fake = FakeSpeech::new(|_| vec![Ok(frame(4.0, 4)), Err(SpeechError::Server("cut".to_owned()))]);
    let speech = queue(fake, &device);
    assert!(speech.say(say("x")).await.is_err());
    let log = events(&device.log);
    let finish = log
        .iter()
        .position(|event| *event == Ev::Finish)
        .expect("a failed stream was not finished");
    assert!(
        log[..finish].iter().any(|event| matches!(event, Ev::Push { .. })),
        "{log:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_device_that_refuses_the_audio_does_not_kill_the_queue() {
    let mut device = Device::new(0, 0);
    device.fail_pushes = true;
    let speech = queue(FakeSpeech::marked(), &device);
    assert_eq!(speech.say(say("1")).await.unwrap(), (0, Outcome::Done));
    assert_eq!(speech.say(say("2")).await.unwrap(), (1, Outcome::Done));
}
