use std::time::Duration;

use e_voice_daemon::schema::event::RecordState::{Finished, Heard, Learned, Started};

use crate::rig::{RATE, Rig, core};

const RATE_OUT: usize = 24_000;

#[tokio::test(flavor = "multi_thread")]
async fn test_silence_after_speech_ends_it_and_the_clip_is_the_speech_alone() {
    let mut rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.audio(1.0, 0.0).await;
    rig.started(1.0);
    rig.audio(2.0, 0.5).await;
    rig.stopped(3.0);
    rig.final_of("hola mundo");
    rig.audio(2.5, 0.0).await;
    let learned = rig.done(recording).await.unwrap();
    assert_eq!((learned.id.as_str(), learned.seconds), ("ana", 2.0));
    assert_eq!(learned.transcript, "hola mundo");
    assert_eq!(learned.text.as_deref(), Some("hola mundo"));
    let clips = rig.clips();
    assert_eq!(clips.len(), 1);
    assert_eq!(
        (clips[0].rate.get() as usize, clips[0].samples.len()),
        (RATE_OUT, 2 * RATE_OUT)
    );
    assert!(
        (core(&clips[0].samples) - 0.5).abs() < 0.05,
        "{}",
        core(&clips[0].samples)
    );
    assert_eq!(rig.states(), [Started, Heard, Finished, Learned]);
    assert!(rig.w.mic.freed().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_silence_is_counted_from_the_end_of_the_latest_speech_not_the_first() {
    let mut rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.audio(0.5, 0.0).await;
    rig.started(0.5);
    rig.audio(1.0, 0.5).await;
    rig.stopped(1.5);
    rig.audio(2.0, 0.0).await;
    rig.started(3.5);
    rig.audio(1.0, 0.5).await;
    rig.stopped(4.5);
    rig.final_of("uno");
    rig.final_of("dos");
    rig.audio(2.5, 0.0).await;
    let learned = rig.done(recording).await.unwrap();
    assert_eq!(learned.seconds, 4.0, "from the first start to the last stop");
    assert_eq!(learned.transcript, "uno dos");
    assert_eq!(rig.clips()[0].samples.len(), 4 * RATE_OUT);
    rig.states();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_custom_silence_ends_it_earlier() {
    let rig = Rig::new(1);
    let take = e_voice_daemon::workflow::record::Take {
        silence: Duration::from_millis(500),
        ..Rig::take("ana")
    };
    let recording = rig.start(take);
    rig.live().await;
    rig.started(0.0);
    rig.audio(1.0, 0.5).await;
    rig.stopped(1.0);
    rig.final_of("hola");
    rig.audio(0.5, 0.0).await;
    assert_eq!(rig.done(recording).await.unwrap().seconds, 1.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_max_ends_it_with_exactly_max_from_the_start_whatever_the_block_size() {
    for block in [1_600, 1_234, 997] {
        let rig = Rig::new(1);
        let take = e_voice_daemon::workflow::record::Take {
            max: Duration::from_secs(3),
            ..Rig::take("ana")
        };
        let recording = rig.start(take);
        rig.live().await;
        rig.feed(RATE, 0.0, block).await;
        rig.started(1.0);
        rig.final_of("largo");
        rig.feed(4 * RATE, 0.5, block).await;
        let learned = rig.done(recording).await.unwrap();
        assert_eq!(learned.seconds, 3.0, "block {block}");
        assert_eq!(rig.clips()[0].samples.len(), 3 * RATE_OUT, "block {block}");
        assert!((core(&rig.clips()[0].samples) - 0.5).abs() < 0.05, "block {block}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_longest_clip_is_thirty_seconds_and_at_most_720000_samples_at_24_khz() {
    let rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.final_of("muy largo");
    rig.audio(31.0, 0.3).await;
    let learned = rig.done(recording).await.unwrap();
    assert_eq!(learned.seconds, 30.0);
    let clips = rig.clips();
    assert_eq!(clips[0].rate.get(), 24_000);
    assert_eq!(clips[0].samples.len(), 720_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_finish_mid_speech_keeps_to_the_last_sample_and_waits_for_the_finals() {
    let rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.audio(1.0, 0.0).await;
    rig.started(1.0);
    rig.audio(1.5, 0.5).await;
    let before = rig.feeds[0].sent().len();
    assert!(rig.w.record.finish(), "a recording was running");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while rig.feeds[0].sent().len() < before + RATE && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        rig.feeds[0].sent().len(),
        before + RATE,
        "a second of silence closes the open segment"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!recording.is_finished(), "the clip was learned before the final came");
    assert!(rig.clips().is_empty());
    rig.final_of("a medias");
    let learned = rig.done(recording).await.unwrap();
    assert_eq!(learned.seconds, 1.5, "the hush is not part of the clip");
    assert_eq!(learned.transcript, "a medias");
    assert_eq!(rig.clips()[0].samples.len(), 36_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_finish_between_two_sentences_keeps_to_the_latest_stop() {
    let rig = Rig::new(1);
    let recording = rig.start(Rig::take("ana"));
    rig.live().await;
    rig.started(0.0);
    rig.audio(1.0, 0.5).await;
    rig.stopped(1.0);
    rig.final_of("una");
    rig.audio(0.5, 0.0).await;
    assert!(rig.w.record.finish());
    let learned = rig.done(recording).await.unwrap();
    assert_eq!(learned.seconds, 1.0, "to the stop, without the silence after it");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_finish_with_no_recording_is_false() {
    let rig = Rig::new(0);
    assert!(!rig.w.record.finish());
    assert!(!rig.w.record.cancel());
}
