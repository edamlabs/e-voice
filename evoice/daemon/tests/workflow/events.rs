use std::sync::atomic::Ordering;
use std::time::Duration;

use e_voice_core::schema::error::NodeError;
use e_voice_daemon::hearing::base::{HearingError, Recognized};
use e_voice_daemon::schema::event::{DaemonEvent, ListenState, SpeechState};
use e_voice_daemon::workflow::listen::ListenError;
use tokio::sync::broadcast::error::TryRecvError;

use crate::fake::{Device, FakeSpeech, Wired, drained, heard, long, say, wired};

const OPEN: DaemonEvent = DaemonEvent::Listening {
    state: ListenState::Open,
};
const CLOSED: DaemonEvent = DaemonEvent::Listening {
    state: ListenState::Closed,
};

fn speech(state: SpeechState) -> DaemonEvent {
    DaemonEvent::Speech(state)
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn listening(default: Duration) -> Wired {
    wired(FakeSpeech::marked(), Device::new(0, 0), default)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_done_say_announces_speaking_draining_idle_with_the_id_it_answers() {
    let w = wired(FakeSpeech::marked(), Device::new(2, 0), Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let (id, _) = w.state.speech.say(say("4")).await.unwrap();
    let (second, _) = w.state.speech.say(say("5")).await.unwrap();
    assert_eq!((id, second), (0, 1));
    until("idle", || w.state.speech.state() == SpeechState::Idle).await;
    assert_eq!(
        drained(&mut bus),
        [
            speech(SpeechState::Speaking { id: 0 }),
            speech(SpeechState::Draining { id: 0 }),
            speech(SpeechState::Idle),
            speech(SpeechState::Speaking { id: 1 }),
            speech(SpeechState::Draining { id: 1 }),
            speech(SpeechState::Idle),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_mid_speech_announces_speaking_cancelling_idle() {
    let w = wired(long(200, 1.0), Device::new(0, 0), Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let speaker = w.state.speech.clone();
    let said = tokio::spawn(async move { speaker.say(say("x")).await });
    until("speaking", || {
        w.state.speech.state() == (SpeechState::Speaking { id: 0 })
    })
    .await;
    w.state.speech.stop().await.unwrap();
    said.await.unwrap().unwrap();
    assert_eq!(
        drained(&mut bus),
        [
            speech(SpeechState::Speaking { id: 0 }),
            speech(SpeechState::Cancelling { id: 0 }),
            speech(SpeechState::Idle),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_during_draining_announces_draining_cancelling_idle() {
    let w = wired(FakeSpeech::marked(), Device::new(100_000, 0), Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let speaker = w.state.speech.clone();
    let said = tokio::spawn(async move { speaker.say(say("1")).await });
    until("draining", || {
        w.state.speech.state() == (SpeechState::Draining { id: 0 })
    })
    .await;
    w.state.speech.stop().await.unwrap();
    said.await.unwrap().unwrap();
    assert_eq!(
        drained(&mut bus),
        [
            speech(SpeechState::Speaking { id: 0 }),
            speech(SpeechState::Draining { id: 0 }),
            speech(SpeechState::Cancelling { id: 0 }),
            speech(SpeechState::Idle),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_that_drops_waiting_texts_announces_nothing_for_them() {
    let (fake, gate) = long(200, 1.0).gated();
    let w = wired(fake, Device::new(0, 0), Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let tasks: Vec<_> = (0..3)
        .map(|_| {
            let speaker = w.state.speech.clone();
            tokio::spawn(async move { speaker.say(say("x")).await })
        })
        .collect();
    until("speaking", || {
        w.state.speech.state() == (SpeechState::Speaking { id: 0 })
    })
    .await;
    gate.add_permits(1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let halted = w.state.speech.stop().await.unwrap();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!((halted.stopped, halted.dropped), (Some(0), 2));
    assert_eq!(
        drained(&mut bus),
        [
            speech(SpeechState::Speaking { id: 0 }),
            speech(SpeechState::Cancelling { id: 0 }),
            speech(SpeechState::Idle),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_while_idle_and_a_refused_text_announce_nothing() {
    let w = wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    w.state.speech.stop().await.unwrap();
    assert!(w.state.speech.say(say("   ")).await.is_err());
    assert_eq!(drained(&mut bus), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_speech_failure_still_walks_speaking_draining_idle() {
    let mut fake = FakeSpeech::marked();
    fake.fail_say = Some(e_voice_daemon::speech::base::SpeechError::Server("down".to_owned()));
    let w = wired(fake, Device::new(0, 0), Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    assert!(w.state.speech.say(say("1")).await.is_err());
    until("idle", || w.state.speech.state() == SpeechState::Idle).await;
    let seen = drained(&mut bus);
    assert_eq!(seen.first(), Some(&speech(SpeechState::Speaking { id: 0 })), "{seen:?}");
    assert_eq!(seen.last(), Some(&speech(SpeechState::Idle)), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_successful_listen_announces_open_partials_heard_closed_and_heard_comes_before_the_answer() {
    let w = listening(Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let feed = w.hearing.session();
    let listener = std::sync::Arc::clone(&w.state.listen);
    let task = tokio::spawn(async move { listener.listen(None).await });
    assert!(w.mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.say(Recognized::Partial("ho".to_owned()));
    feed.say(Recognized::Partial("hola".to_owned()));
    feed.say(Recognized::Final(heard("hola"), None));
    let answer = task.await.unwrap().unwrap();
    assert_eq!(answer, heard("hola"));
    let seen = drained(&mut bus);
    assert_eq!(
        seen,
        [
            OPEN,
            DaemonEvent::Partial { text: "ho".to_owned() },
            DaemonEvent::Partial {
                text: "hola".to_owned()
            },
            DaemonEvent::Heard(heard("hola")),
            CLOSED,
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failed_final_is_not_heard_and_the_next_one_is_heard_exactly_once() {
    let w = listening(Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let feed = w.hearing.session();
    let listener = std::sync::Arc::clone(&w.state.listen);
    let task = tokio::spawn(async move { listener.listen(None).await });
    assert!(w.mic.live().await);
    feed.say(Recognized::Final(heard("perdido"), Some(NodeError::Timeout)));
    feed.say(Recognized::Final(heard(" "), None));
    feed.say(Recognized::Final(heard("bien"), None));
    task.await.unwrap().unwrap();
    assert_eq!(drained(&mut bus), [OPEN, DaemonEvent::Heard(heard("bien")), CLOSED]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_listen_that_times_out_fails_or_has_no_microphone_announces_open_closed_and_no_heard() {
    let w = listening(Duration::from_millis(100));
    let mut bus = w.events.subscribe();
    let _feed = w.hearing.session();
    let timeout = w.state.listen.listen(None).await.unwrap_err();
    assert!(matches!(timeout, ListenError::Timeout(_)), "{timeout:?}");
    assert_eq!(drained(&mut bus), [OPEN, CLOSED]);

    let feed = w.hearing.session();
    feed.fail(HearingError::Server("closed with 1011".to_owned()));
    let failed = w.state.listen.listen(Some(Duration::from_secs(5))).await.unwrap_err();
    assert!(matches!(failed, ListenError::Hearing(_)), "{failed:?}");
    assert_eq!(drained(&mut bus), [OPEN, CLOSED]);

    w.mic.fail.store(true, Ordering::SeqCst);
    let device = w.state.listen.listen(None).await.unwrap_err();
    assert!(matches!(device, ListenError::Device(_)), "{device:?}");
    assert_eq!(drained(&mut bus), [OPEN, CLOSED]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_recognizer_that_cannot_open_announces_open_closed() {
    let w = listening(Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    *w.hearing.fail_open.lock().unwrap() = Some(HearingError::Connect("refused".to_owned()));
    assert!(w.state.listen.listen(None).await.is_err());
    assert_eq!(drained(&mut bus), [OPEN, CLOSED]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_refused_listen_announces_nothing_and_a_busy_one_does_not_disturb_the_running() {
    let w = listening(Duration::from_secs(5));
    let mut bus = w.events.subscribe();
    let invalid = w.state.listen.listen(Some(Duration::ZERO)).await.unwrap_err();
    assert!(matches!(invalid, ListenError::Invalid(_)));
    assert_eq!(drained(&mut bus), []);

    let feed = w.hearing.session();
    let listener = std::sync::Arc::clone(&w.state.listen);
    let first = tokio::spawn(async move { listener.listen(None).await });
    assert!(w.mic.live().await);
    let busy = w.state.listen.listen(None).await.unwrap_err();
    assert!(matches!(busy, ListenError::Busy));
    assert_eq!(drained(&mut bus), [OPEN], "the refused listen added nothing");
    feed.say(Recognized::Final(heard("uno"), None));
    first.await.unwrap().unwrap();
    assert_eq!(drained(&mut bus), [DaemonEvent::Heard(heard("uno")), CLOSED]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_caller_that_stops_waiting_still_closes_the_listen_once() {
    let w = listening(Duration::from_secs(60));
    let mut bus = w.events.subscribe();
    let _feed = w.hearing.session();
    let gave_up = tokio::time::timeout(Duration::from_millis(150), w.state.listen.listen(None)).await;
    assert!(gave_up.is_err());
    assert_eq!(drained(&mut bus), [OPEN, CLOSED]);
    assert!(matches!(bus.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_events_without_a_subscriber_are_not_a_failure_and_an_early_one_is_not_replayed() {
    let w = wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_secs(5));
    w.state.speech.say(say("1")).await.unwrap();
    until("idle", || w.state.speech.state() == SpeechState::Idle).await;
    let mut late = w.events.subscribe();
    assert_eq!(drained(&mut late), []);
    w.state.speech.say(say("2")).await.unwrap();
    until("idle again", || w.state.speech.state() == SpeechState::Idle).await;
    assert_eq!(
        drained(&mut late),
        [
            speech(SpeechState::Speaking { id: 1 }),
            speech(SpeechState::Draining { id: 1 }),
            speech(SpeechState::Idle),
        ]
    );
}
