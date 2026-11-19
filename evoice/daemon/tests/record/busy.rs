use std::sync::atomic::Ordering;
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::Recognized;
use e_voice_daemon::workflow::record::RecordError;

use crate::fake::{heard, talk};
use crate::rig::Rig;

fn assert_refused_untouched(rig: &mut Rig) {
    assert_eq!(
        rig.w.voices.calls.load(Ordering::SeqCst),
        0,
        "the speech service was asked first"
    );
    assert!(rig.clips().is_empty());
    assert!(rig.states().is_empty(), "a refused recording announced itself");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_listen_holding_the_microphone_refuses_a_recording_busy() {
    let mut rig = Rig::new(2);
    let listen = std::sync::Arc::clone(&rig.w.state.listen);
    let holding = tokio::spawn(async move { listen.listen(Some(Duration::from_secs(5))).await });
    rig.live().await;
    let refused = rig.done(rig.start(Rig::take("ana"))).await;
    assert!(matches!(refused, Err(RecordError::Busy)), "{refused:?}");
    assert_eq!(rig.w.hearing.opened(), 1, "only the listen opened the recognizer");
    assert_refused_untouched(&mut rig);
    rig.feeds[0].say(Recognized::Started(Duration::ZERO));
    rig.feeds[0].say(Recognized::Final(heard("hola"), None));
    holding.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_voice_conversation_holding_the_microphone_refuses_a_recording_busy() {
    let mut rig = Rig::new(2);
    let conversation = talk(
        &rig.w,
        Lang::Es,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(1),
    );
    conversation.start().await.unwrap();
    rig.live().await;
    let refused = rig.done(rig.start(Rig::take("ana"))).await;
    assert!(matches!(refused, Err(RecordError::Busy)), "{refused:?}");
    assert_refused_untouched(&mut rig);
    conversation.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_second_recording_is_refused_busy_while_the_first_runs_and_the_first_is_untouched() {
    let rig = Rig::new(2);
    let first = rig.start(Rig::take("ana"));
    rig.live().await;
    let second = rig.done(rig.start(Rig::take("eva"))).await;
    assert!(matches!(second, Err(RecordError::Busy)), "{second:?}");
    assert_eq!(rig.w.hearing.opened(), 1);
    rig.audio(0.5, 0.0).await;
    rig.started(0.0);
    rig.audio(0.5, 0.5).await;
    assert!(rig.w.record.finish());
    rig.final_of("primero");
    assert_eq!(rig.done(first).await.unwrap().id, "ana");
    assert!(rig.w.mic.freed().await);
    assert_eq!(rig.clips().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_microphone_is_free_for_a_listen_once_a_recording_ends() {
    let rig = Rig::waiting(2, Duration::from_millis(200));
    assert!(matches!(
        rig.done(rig.start(Rig::take("ana"))).await,
        Err(RecordError::Timeout(_))
    ));
    let listen = std::sync::Arc::clone(&rig.w.state.listen);
    let listening = tokio::spawn(async move { listen.listen(Some(Duration::from_secs(5))).await });
    rig.feeds[1].say(Recognized::Started(Duration::ZERO));
    rig.feeds[1].say(Recognized::Final(heard("hola"), None));
    assert_eq!(listening.await.unwrap().unwrap().text, "hola");
}
