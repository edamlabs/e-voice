use std::sync::Arc;
use std::time::Duration;

use e_voice_core::schema::error::NodeError;
use e_voice_daemon::hearing::base::{Heard, HearingError, Recognized};
use e_voice_daemon::workflow::listen::{Listen, ListenError, Listener};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::fake::{FakeHearing, Feed, Mic, heard};

type Task = JoinHandle<Result<Heard, ListenError>>;

async fn start(timeout: Duration) -> (Task, Feed, Mic) {
    let (hearing, mic) = (FakeHearing::default(), Mic::default());
    let feed = hearing.session();
    let listener = Arc::new(Listener::new(
        hearing,
        mic.thread(),
        Duration::from_secs(15),
        crate::fake::bus(),
    ));
    let task = tokio::spawn(async move { listener.listen(Some(timeout)).await });
    while !mic.is_open() {
        tokio::task::yield_now().await;
    }
    sleep(Duration::from_millis(1)).await;
    (task, feed, mic)
}

fn silent(task: &Task) -> bool {
    !task.is_finished()
}

#[tokio::test(start_paused = true)]
async fn test_a_started_event_then_a_final_five_seconds_after_a_one_second_timeout_is_a_success() {
    let (task, feed, _) = start(Duration::from_secs(1)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(5)).await;
    assert!(silent(&task));
    feed.say(Recognized::Final(heard("tarde"), None));
    assert_eq!(task.await.unwrap().unwrap(), heard("tarde"));
}

#[tokio::test(start_paused = true)]
async fn test_silence_is_a_timeout_exactly_at_the_deadline_never_before() {
    let (task, _feed, _) = start(Duration::from_secs(10)).await;
    sleep(Duration::from_millis(9_900)).await;
    assert!(silent(&task));
    sleep(Duration::from_millis(200)).await;
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, ListenError::Timeout(t) if t == Duration::from_secs(10)),
        "{error:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn test_started_then_no_final_for_forty_seconds_is_a_hearing_error_never_a_timeout() {
    let (task, feed, mic) = start(Duration::from_secs(1)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(39)).await;
    assert!(silent(&task));
    sleep(Duration::from_secs(2)).await;
    let error = task.await.unwrap().unwrap_err();
    match &error {
        ListenError::Hearing(HearingError::Server(message)) => {
            assert_eq!(message, "no final within 40 s of speech");
        }
        other => panic!("{other:?}"),
    }
    assert!(mic.freed().await);
}

#[tokio::test(start_paused = true)]
async fn test_a_final_just_inside_forty_seconds_of_the_start_is_heard() {
    let (task, feed, _) = start(Duration::from_secs(1)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(39)).await;
    feed.say(Recognized::Final(heard("justo"), None));
    assert_eq!(task.await.unwrap().unwrap(), heard("justo"));
}

#[tokio::test(start_paused = true)]
async fn test_a_second_started_rearms_the_forty_seconds_from_itself() {
    let (task, feed, _) = start(Duration::from_secs(1)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(30)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(39)).await;
    assert!(silent(&task), "69 s after the first start, 39 s after the second");
    sleep(Duration::from_secs(2)).await;
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, ListenError::Hearing(HearingError::Server(_))),
        "{error:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn test_partials_do_not_rearm_the_forty_seconds() {
    let (task, feed, _) = start(Duration::from_secs(1)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(30)).await;
    feed.say(Recognized::Partial("sigo".to_owned()));
    sleep(Duration::from_secs(11)).await;
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, ListenError::Hearing(HearingError::Server(_))),
        "{error:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn test_started_then_an_error_final_waits_again_and_times_out_at_the_original_deadline() {
    let (task, feed, _) = start(Duration::from_secs(10)).await;
    sleep(Duration::from_secs(2)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(3)).await;
    feed.say(Recognized::Final(
        heard("x"),
        Some(NodeError::Backend("down".to_owned())),
    ));
    sleep(Duration::from_secs(4)).await;
    assert!(silent(&task), "9 s in, deadline at 10 s");
    sleep(Duration::from_secs(2)).await;
    let error = task.await.unwrap().unwrap_err();
    assert!(
        matches!(error, ListenError::Timeout(t) if t == Duration::from_secs(10)),
        "{error:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn test_after_an_error_final_a_new_start_gets_a_fresh_forty_seconds() {
    let (task, feed, _) = start(Duration::from_secs(10)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    feed.say(Recognized::Final(heard("x"), Some(NodeError::Timeout)));
    sleep(Duration::from_secs(9)).await;
    feed.say(Recognized::Started(std::time::Duration::ZERO));
    sleep(Duration::from_secs(30)).await;
    assert!(
        silent(&task),
        "past the original deadline, inside the new forty seconds"
    );
    feed.say(Recognized::Final(heard("bien"), None));
    assert_eq!(task.await.unwrap().unwrap(), heard("bien"));
}
