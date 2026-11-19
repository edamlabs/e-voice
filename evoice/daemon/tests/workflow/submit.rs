use std::time::{Duration, Instant};

use e_voice_daemon::schema::event::SpeechState;
use e_voice_daemon::speech::base::SpeechError;
use e_voice_daemon::workflow::queue::{BACKLOG, SpeakError, TEXT};

use crate::fake::{Device, FakeSpeech, queue, say};

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_returns_at_once_while_the_speech_is_still_held() {
    let (fake, gate) = FakeSpeech::marked().gated();
    let device = Device::new(0, 0);
    let speech = queue(fake.clone(), &device);
    let started = Instant::now();
    speech.submit(say("1")).unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(speech.state(), SpeechState::Speaking { id: 0 });
    gate.add_permits(1);
    let deadline = Instant::now() + Duration::from_secs(30);
    while speech.state() != SpeechState::Idle && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(speech.state(), SpeechState::Idle);
    assert_eq!(fake.texts(), ["1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_refuses_what_say_refuses_before_it_returns() {
    let device = Device::new(0, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    assert!(matches!(speech.submit(say("   ")), Err(SpeakError::Empty)));
    assert!(matches!(speech.submit(say(&"a".repeat(TEXT + 1))), Err(SpeakError::TooLong(n)) if n == TEXT + 1));
    assert!(speech.submit(say(&"a".repeat(TEXT))).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_into_a_full_queue_is_full_and_ids_stay_contiguous() {
    let (fake, gate) = FakeSpeech::marked().gated();
    let device = Device::new(0, 0);
    let speech = queue(fake, &device);
    speech.submit(say("0")).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    for n in 0..BACKLOG {
        speech.submit(say(&n.to_string())).unwrap();
    }
    assert!(matches!(speech.submit(say("de más")), Err(SpeakError::Full)));
    gate.add_permits(100);
    let deadline = Instant::now() + Duration::from_secs(30);
    let id = loop {
        match speech.say(say("último")).await {
            Ok((id, _)) => break id,
            Err(SpeakError::Full) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(error) => panic!("{error:?}"),
        }
    };
    assert_eq!(id, u64::try_from(BACKLOG).unwrap() + 1, "refused texts spend no id");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_submitted_text_and_a_said_one_share_the_queue_in_order() {
    let fake = FakeSpeech::marked();
    let device = Device::new(0, 0);
    let speech = queue(fake.clone(), &device);
    speech.submit(say("1")).unwrap();
    let (id, _) = speech.say(say("2")).await.unwrap();
    assert_eq!(id, 1);
    assert_eq!(fake.texts(), ["1", "2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_submitted_job_that_fails_does_not_stop_the_queue() {
    let mut broken = FakeSpeech::marked();
    broken.fail_say = Some(SpeechError::Server("down".to_owned()));
    let device = Device::new(0, 0);
    let speech = queue(broken, &device);
    speech.submit(say("1")).unwrap();
    let error = speech.say(say("2")).await.unwrap_err();
    assert!(matches!(error, SpeakError::Speech(SpeechError::Server(_))), "{error:?}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while speech.state() != SpeechState::Idle && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(speech.state(), SpeechState::Idle);
    speech.submit(say("3")).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_after_the_queue_task_is_gone_is_stopped() {
    let device = Device::new(0, 0);
    let speech = queue(FakeSpeech::marked(), &device);
    let copy = speech.clone();
    drop(speech);
    assert!(copy.submit(say("1")).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_cuts_a_submitted_text_like_any_other() {
    let device = Device::new(0, 0);
    let speech = queue(crate::fake::long(300, 1.0), &device);
    speech.submit(say("x")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while speech.state() != (SpeechState::Speaking { id: 0 }) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let halted = speech.stop().await.unwrap();
    assert_eq!(halted.stopped, Some(0));
}
