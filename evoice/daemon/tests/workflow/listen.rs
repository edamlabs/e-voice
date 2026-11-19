use std::sync::Arc;
use std::time::{Duration, Instant};

use e_voice_core::schema::error::NodeError;
use e_voice_daemon::hearing::base::{HearingError, Recognized};
use e_voice_daemon::workflow::listen::{LONGEST, Listen, ListenError, Listener};
use e_voice_device::error::DeviceError;

use crate::fake::{FakeHearing, Mic, heard};

const DEFAULT: Duration = Duration::from_millis(150);

fn rig() -> (Arc<Listener<FakeHearing>>, FakeHearing, Mic) {
    let (hearing, mic) = (FakeHearing::default(), Mic::default());
    let listener = Arc::new(Listener::new(
        hearing.clone(),
        mic.thread(),
        DEFAULT,
        crate::fake::bus(),
    ));
    (listener, hearing, mic)
}

fn spawn(
    listener: &Arc<Listener<FakeHearing>>,
    timeout: Option<Duration>,
) -> tokio::task::JoinHandle<Result<e_voice_daemon::hearing::base::Heard, ListenError>> {
    let listener = Arc::clone(listener);
    tokio::spawn(async move { listener.listen(timeout).await })
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listen_returns_the_first_final_and_sends_every_block_bit_exact() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    let blocks: [&[f32]; 3] = [&[0.25, -0.5], &[f32::MIN_POSITIVE, 1.0, -1.0], &[0.125]];
    for block in blocks {
        mic.block(block);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while feed.sent().len() < blocks.concat().len() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.say(Recognized::Partial("ho".to_owned()));
    feed.say(Recognized::Final(heard("hola"), None));
    let got = task.await.unwrap().unwrap();
    assert_eq!(got, heard("hola"));
    assert_eq!(feed.sent(), blocks.concat());
    assert_eq!(hearing.rates.lock().unwrap().as_slice(), [16_000]);
    assert!(mic.freed().await);
    assert!(feed.is_closed());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_final_with_an_error_or_without_text_is_skipped_and_the_next_one_is_returned() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.say(Recognized::Final(heard("lost"), Some(NodeError::Timeout)));
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.say(Recognized::Final(heard("   "), None));
    feed.say(Recognized::Final(heard(""), None));
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.say(Recognized::Final(heard("segunda"), None));
    assert_eq!(task.await.unwrap().unwrap(), heard("segunda"));
    assert!(mic.freed().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_silence_past_the_timeout_is_a_timeout_naming_it_and_releases_everything() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let started = Instant::now();
    let error = listener.listen(Some(Duration::from_millis(250))).await.unwrap_err();
    let took = started.elapsed();
    assert!(
        matches!(error, ListenError::Timeout(t) if t == Duration::from_millis(250)),
        "{error:?}"
    );
    assert_eq!(error.to_string(), "nobody started speaking within 0.25 s");
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_secs(2),
        "{took:?}"
    );
    assert!(mic.freed().await);
    assert!(feed.is_closed());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_no_timeout_given_uses_the_listeners_default() {
    let (listener, hearing, _) = rig();
    let _feed = hearing.session();
    let error = listener.listen(None).await.unwrap_err();
    assert!(matches!(error, ListenError::Timeout(t) if t == DEFAULT), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_final_long_after_the_timeout_succeeds_when_speech_started_in_time() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_millis(100)));
    assert!(mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    tokio::time::sleep(Duration::from_millis(500)).await;
    feed.say(Recognized::Final(heard("tarde"), None));
    assert_eq!(task.await.unwrap().unwrap(), heard("tarde"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_started_then_an_error_final_returns_to_waiting_against_the_original_deadline() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let started = Instant::now();
    let task = spawn(&listener, Some(Duration::from_millis(400)));
    assert!(mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    tokio::time::sleep(Duration::from_millis(100)).await;
    feed.say(Recognized::Final(heard("x"), Some(NodeError::Overload)));
    let error = task.await.unwrap().unwrap_err();
    let took = started.elapsed();
    assert!(matches!(error, ListenError::Timeout(_)), "{error:?}");
    assert!(
        took >= Duration::from_millis(380) && took < Duration::from_secs(2),
        "{took:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_error_final_after_the_original_deadline_times_out_at_once() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_millis(100)));
    assert!(mic.live().await);
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!task.is_finished());
    let at = Instant::now();
    feed.say(Recognized::Final(heard("x"), Some(NodeError::Stalled)));
    let error = task.await.unwrap().unwrap_err();
    assert!(matches!(error, ListenError::Timeout(_)), "{error:?}");
    assert!(at.elapsed() < Duration::from_secs(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_timeout_of_zero_or_over_the_longest_is_refused_before_anything_opens() {
    let (listener, hearing, mic) = rig();
    for timeout in [
        Duration::ZERO,
        LONGEST + Duration::from_millis(1),
        Duration::from_secs(301),
        Duration::MAX,
    ] {
        let error = listener.listen(Some(timeout)).await.unwrap_err();
        assert!(
            matches!(error, ListenError::Invalid(t) if t == timeout),
            "{timeout:?}: {error:?}"
        );
    }
    assert_eq!(mic.opened(), 0);
    assert_eq!(hearing.opened(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_invalid_text_names_the_bounds() {
    let (listener, ..) = rig();
    let error = listener.listen(Some(Duration::from_secs(301))).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "a listen timeout is above zero and at most 300 s, not 301 s"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_second_listen_while_one_runs_is_busy_at_once_and_opens_nothing() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let first = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    let at = Instant::now();
    let second = listener.listen(Some(Duration::from_secs(5))).await.unwrap_err();
    assert!(matches!(second, ListenError::Busy), "{second:?}");
    assert!(at.elapsed() < Duration::from_millis(500));
    assert_eq!(mic.opened(), 1);
    assert_eq!(hearing.opened(), 1);
    feed.say(Recognized::Final(heard("uno"), None));
    assert_eq!(first.await.unwrap().unwrap(), heard("uno"));
    assert!(mic.freed().await);
    let again = hearing.session();
    again.say(Recognized::Final(heard("dos"), None));
    assert_eq!(
        listener.listen(Some(Duration::from_secs(5))).await.unwrap(),
        heard("dos")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_invalid_timeout_is_refused_even_while_busy() {
    let (listener, hearing, mic) = rig();
    let _feed = hearing.session();
    let first = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    let error = listener.listen(Some(Duration::ZERO)).await.unwrap_err();
    assert!(matches!(error, ListenError::Invalid(_)), "{error:?}");
    first.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_caller_that_stops_waiting_releases_the_microphone_and_the_session_and_frees_the_turn() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let gave_up = tokio::time::timeout(
        Duration::from_millis(100),
        listener.listen(Some(Duration::from_secs(60))),
    )
    .await;
    assert!(gave_up.is_err());
    assert!(mic.freed().await);
    assert!(feed.is_closed());
    let next = hearing.session();
    next.say(Recognized::Final(heard("otra"), None));
    assert_eq!(
        listener.listen(Some(Duration::from_secs(5))).await.unwrap(),
        heard("otra")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_microphone_that_cannot_open_is_a_device_error_and_the_recognizer_is_never_opened() {
    let (listener, hearing, mic) = rig();
    mic.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let error = listener.listen(None).await.unwrap_err();
    assert!(
        matches!(error, ListenError::Device(DeviceError::Input(None))),
        "{error:?}"
    );
    assert_eq!(hearing.opened(), 0);
    mic.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    hearing.session().say(Recognized::Final(heard("ya"), None));
    assert_eq!(
        listener.listen(Some(Duration::from_secs(5))).await.unwrap(),
        heard("ya")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_recognizer_that_cannot_open_is_a_hearing_error_and_the_microphone_is_released() {
    let (listener, hearing, mic) = rig();
    *hearing.fail_open.lock().unwrap() = Some(HearingError::Connect("refused".to_owned()));
    let error = listener.listen(None).await.unwrap_err();
    assert!(
        matches!(&error, ListenError::Hearing(HearingError::Connect(m)) if m == "refused"),
        "{error:?}"
    );
    assert!(mic.freed().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_recognizer_error_mid_listen_ends_it_as_a_hearing_error() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    feed.fail(HearingError::Server("closed with 1011".to_owned()));
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(&error, ListenError::Hearing(HearingError::Server(m)) if m == "closed with 1011"),
        "{error:?}"
    );
    assert!(mic.freed().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_session_closed_before_any_final_is_a_hearing_error() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    drop(feed);
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, ListenError::Hearing(HearingError::Server(_))),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_dead_input_stream_is_a_device_error() {
    let (listener, hearing, mic) = rig();
    let _feed = hearing.session();
    let task = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    mic.die();
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, ListenError::Device(DeviceError::Input(None))),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_listener_that_falls_behind_loses_blocks_and_still_hears_the_final() {
    let (listener, hearing, mic) = rig();
    let feed = hearing.session();
    hearing.hold_sends.store(true, std::sync::atomic::Ordering::SeqCst);
    let task = spawn(&listener, Some(Duration::from_secs(5)));
    assert!(mic.live().await);
    for n in 0..200u8 {
        mic.block(&[f32::from(n)]);
    }
    hearing.hold_sends.store(false, std::sync::atomic::Ordering::SeqCst);
    feed.say(Recognized::Final(heard("tarde"), None));
    assert_eq!(task.await.unwrap().unwrap(), heard("tarde"));
    assert!(feed.sent().len() < 200);
    assert!(mic.freed().await);
}

#[test]
fn test_the_listener_and_the_capture_thread_print_without_their_insides() {
    let (listener, _, mic) = rig();
    assert_eq!(format!("{listener:?}"), "Listener { timeout: 150ms, .. }");
    assert_eq!(format!("{:?}", mic.thread()), "CaptureThread { .. }");
}
