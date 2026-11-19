use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use e_voice_core::schema::error::{DiarizeStage, NodeError};
use e_voice_core::schema::lang::Lang;
use e_voice_stt::config::pipeline::PipelineConfig;
use e_voice_stt::schema::event::{Event, SegmentSpeaker, SpeakersEvent};
use e_voice_stt::schema::segment::{SegmentId, SegmentSpan};
use e_voice_stt::schema::speaker::{Diarization, SpeakerId, Turn};
use e_voice_stt::workflow::runner::{Runner, RunnerError};
use proptest::prelude::*;

mod sherpa;

use crate::fake::{FakeDiarize, SPEECH, batch, diarizing, nodes};

fn span(start: u64, end: u64) -> SegmentSpan {
    SegmentSpan { start, end }
}

fn turn(start: u64, end: u64, speaker: u32) -> Turn {
    Turn {
        span: span(start, end),
        speaker: SpeakerId(speaker),
    }
}

fn segments(spans: &[(u64, u64)]) -> Vec<(SegmentId, SegmentSpan)> {
    spans
        .iter()
        .enumerate()
        .map(|(i, &(start, end))| (SegmentId(i as u64), span(start, end)))
        .collect()
}

fn speakers(joined: &SpeakersEvent) -> Vec<(u64, u32)> {
    joined
        .segments
        .iter()
        .map(|named| (named.segment.0, named.speaker.0))
        .collect()
}

#[test]
fn test_join_majority_overlap_wins() {
    let joined = SpeakersEvent::join(&segments(&[(0, 100)]), &[turn(0, 30, 5), turn(30, 100, 9)]);
    assert_eq!(speakers(&joined), [(0, 0)]);
    let two = SpeakersEvent::join(&segments(&[(0, 100), (100, 200)]), &[turn(0, 30, 5), turn(30, 200, 9)]);
    assert_eq!(speakers(&two), [(0, 0), (1, 0)]);
    assert_eq!(joined.error, None);
}

#[test]
fn test_join_segment_without_turn_is_absent() {
    let joined = SpeakersEvent::join(
        &segments(&[(0, 100), (200, 300), (400, 500)]),
        &[turn(0, 100, 1), turn(400, 500, 2)],
    );
    assert_eq!(speakers(&joined), [(0, 0), (2, 1)]);
}

#[test]
fn test_join_touching_turn_is_not_overlap() {
    let joined = SpeakersEvent::join(&segments(&[(100, 200)]), &[turn(0, 100, 1), turn(200, 300, 2)]);
    assert_eq!(joined.segments, Vec::new());
}

#[test]
fn test_join_tie_goes_to_lowest_backend_id() {
    let joined = SpeakersEvent::join(
        &segments(&[(0, 100), (100, 150)]),
        &[turn(0, 50, 9), turn(50, 100, 4), turn(100, 150, 9)],
    );
    assert_eq!(speakers(&joined), [(0, 0), (1, 1)]);
    let reversed = SpeakersEvent::join(&segments(&[(0, 100)]), &[turn(0, 50, 4), turn(50, 100, 9)]);
    assert_eq!(speakers(&reversed), [(0, 0)]);
    let again = SpeakersEvent::join(&segments(&[(0, 100)]), &[turn(0, 50, 4), turn(50, 100, 9)]);
    assert_eq!(reversed, again);
}

#[test]
fn test_join_renumbers_by_first_appearance() {
    let joined = SpeakersEvent::join(
        &segments(&[(0, 10), (10, 20), (20, 30)]),
        &[turn(0, 10, 7), turn(10, 20, 3), turn(20, 30, 7)],
    );
    assert_eq!(speakers(&joined), [(0, 0), (1, 1), (2, 0)]);
}

#[test]
fn test_join_long_turn_spans_several_segments() {
    let joined = SpeakersEvent::join(
        &segments(&[(0, 100), (100, 200), (200, 300), (300, 400)]),
        &[turn(0, 350, 2), turn(350, 400, 6)],
    );
    assert_eq!(speakers(&joined), [(0, 0), (1, 0), (2, 0), (3, 0)]);
}

#[test]
fn test_join_nested_turn_does_not_hide_the_long_one() {
    let joined = SpeakersEvent::join(
        &segments(&[(0, 100), (100, 200), (200, 300)]),
        &[turn(0, 300, 1), turn(40, 60, 2)],
    );
    assert_eq!(speakers(&joined), [(0, 0), (1, 0), (2, 0)]);
    let inner = SpeakersEvent::join(&segments(&[(40, 60), (150, 250)]), &[turn(0, 300, 5), turn(40, 90, 2)]);
    assert_eq!(speakers(&inner), [(0, 0), (1, 1)]);
}

#[test]
fn test_join_empty_inputs() {
    assert_eq!(SpeakersEvent::join(&[], &[turn(0, 10, 1)]).segments, Vec::new());
    assert_eq!(SpeakersEvent::join(&segments(&[(0, 10)]), &[]).segments, Vec::new());
}

fn reference(segs: &[(SegmentId, SegmentSpan)], turns: &[Turn]) -> Vec<(u64, u32)> {
    let mut order: BTreeMap<u32, u32> = BTreeMap::new();
    segs.iter()
        .filter_map(|&(id, s)| {
            let mut totals: BTreeMap<u32, u64> = BTreeMap::new();
            for t in turns {
                let overlap = s.end.min(t.span.end).saturating_sub(s.start.max(t.span.start));
                if overlap > 0 {
                    *totals.entry(t.speaker.0).or_default() += overlap;
                }
            }
            let best = totals
                .iter()
                .fold(None::<(u32, u64)>, |best, (&who, &sum)| match best {
                    Some((_, top)) if top >= sum => best,
                    _ => Some((who, sum)),
                })?;
            let next = order.len() as u32;
            Some((id.0, *order.entry(best.0).or_insert(next)))
        })
        .collect()
}

fn timeline() -> impl Strategy<Value = (Vec<(u64, u64)>, Vec<Turn>)> {
    let segs = prop::collection::vec((0u64..50, 1u64..40), 0..12).prop_map(|steps| {
        steps
            .iter()
            .scan(0u64, |cursor, &(gap, len)| {
                let start = *cursor + gap;
                *cursor = start + len;
                Some((start, *cursor))
            })
            .collect::<Vec<_>>()
    });
    let turns = prop::collection::vec((0u64..600, 1u64..200, 0u32..5), 0..16).prop_map(|raw| {
        let mut turns: Vec<Turn> = raw
            .iter()
            .map(|&(start, len, who)| turn(start, start + len, who))
            .collect();
        turns.sort_by_key(|t| t.span.start);
        turns
    });
    (segs, turns)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn test_join_matches_the_quadratic_reference((spans, turns) in timeline()) {
        let segs = segments(&spans);
        let joined = SpeakersEvent::join(&segs, &turns);
        prop_assert_eq!(speakers(&joined), reference(&segs, &turns));
        let ids: Vec<u32> = joined.segments.iter().map(|SegmentSpeaker { speaker, .. }| speaker.0).collect();
        let firsts: Vec<u32> = ids.iter().enumerate().filter(|(i, id)| !ids[..*i].contains(id)).map(|(_, &id)| id).collect();
        prop_assert_eq!(firsts, (0..ids.iter().copied().max().map_or(0, |top| top + 1)).collect::<Vec<_>>());
    }
}

fn config() -> PipelineConfig {
    PipelineConfig {
        tick: Duration::from_millis(10),
        ..PipelineConfig::default()
    }
}

fn utterances() -> Vec<f32> {
    [
        (0.0f32, 1_600usize),
        (SPEECH, 3_200),
        (0.0, 1_600),
        (SPEECH, 3_200),
        (0.0, 1_600),
    ]
    .iter()
    .flat_map(|&(value, n)| std::iter::repeat_n(value, n))
    .collect()
}

fn voices() -> Vec<Turn> {
    vec![turn(0, 4_800, 7), turn(4_800, 12_000, 3)]
}

async fn collect(runner: &Runner, diarization: Diarization) -> Vec<Event> {
    let mut stream = runner.file(Lang::Es, utterances(), diarization);
    let mut events = Vec::new();
    while let Some(event) = stream.recv().await {
        events.push(event);
    }
    events
}

fn count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::Speakers(_)))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_loaded_node_not_asked_never_runs() {
    let fake = FakeDiarize::new(Ok(voices()));
    let on = Runner::new(diarizing(batch(0), &fake), config());
    let off = Runner::new(nodes(batch(0), None, None), config());
    let (with_node, without) = (
        collect(&on, Diarization::Skip).await,
        collect(&off, Diarization::Skip).await,
    );
    assert_eq!(fake.calls(), 0);
    assert_eq!(count(&with_node), 0);
    assert_eq!(with_node, without);
    let transcript = on.transcribe(Lang::Es, utterances(), Diarization::Skip).await.unwrap();
    assert!(transcript.segments.iter().all(|done| done.speaker.is_none()));
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asked_runs_once_and_speakers_precede_closed() {
    let fake = FakeDiarize::new(Ok(voices()));
    let runner = Runner::new(diarizing(batch(0), &fake), config());
    let events = collect(&runner, Diarization::Run).await;
    assert_eq!(fake.calls(), 1);
    assert_eq!(fake.heard.load(std::sync::atomic::Ordering::SeqCst), utterances().len());
    assert_eq!(count(&events), 1);
    let n = events.len();
    assert_eq!(events[n - 1], Event::Closed);
    let Event::Speakers(joined) = &events[n - 2] else {
        panic!("{events:?}")
    };
    assert_eq!(speakers(joined), [(0, 0), (1, 1)]);
    assert_eq!(joined.error, None);
    let finals = events[..n - 2]
        .iter()
        .filter(|event| matches!(event, Event::Final(_)))
        .count();
    assert_eq!(finals, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_transcribe_folds_speakers_into_finals() {
    let fake = FakeDiarize::new(Ok(voices()));
    let runner = Runner::new(diarizing(batch(0), &fake), config());
    let transcript = runner
        .transcribe(Lang::Es, utterances(), Diarization::Run)
        .await
        .unwrap();
    let labelled: Vec<Option<u32>> = transcript
        .segments
        .iter()
        .map(|done| done.speaker.map(|id| id.0))
        .collect();
    assert_eq!(labelled, [Some(0), Some(1)]);
    assert_eq!(fake.calls(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_segment_without_turn_keeps_no_speaker() {
    let fake = FakeDiarize::new(Ok(vec![turn(0, 4_800, 2)]));
    let runner = Runner::new(diarizing(batch(0), &fake), config());
    let transcript = runner
        .transcribe(Lang::Es, utterances(), Diarization::Run)
        .await
        .unwrap();
    let labelled: Vec<Option<u32>> = transcript
        .segments
        .iter()
        .map(|done| done.speaker.map(|id| id.0))
        .collect();
    assert_eq!(labelled, [Some(0), None]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_node_off_asked_emits_nothing_and_no_error() {
    let runner = Runner::new(nodes(batch(0), None, None), config());
    let events = collect(&runner, Diarization::Run).await;
    assert_eq!(count(&events), 0);
    assert_eq!(events.last(), Some(&Event::Closed));
    let transcript = runner
        .transcribe(Lang::Es, utterances(), Diarization::Run)
        .await
        .unwrap();
    assert!(
        transcript
            .segments
            .iter()
            .all(|done| done.speaker.is_none() && done.error.is_none())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failing_node_asked_reports_and_closes() {
    let failure = NodeError::Diarize(DiarizeStage::Process);
    let fake = FakeDiarize::new(Err(failure.clone()));
    let runner = Runner::new(diarizing(batch(0), &fake), config());
    let events = collect(&runner, Diarization::Run).await;
    let n = events.len();
    assert_eq!(events[n - 1], Event::Closed);
    assert_eq!(
        events[n - 2],
        Event::Speakers(SpeakersEvent {
            segments: Vec::new(),
            error: Some(failure.clone())
        })
    );
    let outcome = runner.transcribe(Lang::Es, utterances(), Diarization::Run).await;
    assert_eq!(outcome.unwrap_err(), RunnerError::Diarize(failure));
    assert_eq!(fake.calls(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failing_node_not_asked_succeeds() {
    let fake = FakeDiarize::new(Err(NodeError::Diarize(DiarizeStage::Segments)));
    let runner = Runner::new(diarizing(batch(0), &fake), config());
    let transcript = runner
        .transcribe(Lang::Es, utterances(), Diarization::Skip)
        .await
        .unwrap();
    assert_eq!(transcript.segments.len(), 2);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dropped_receiver_ends_the_run() {
    let fake = FakeDiarize::new(Ok(voices()));
    let runner = Runner::new(diarizing(batch(0), &fake), config());
    drop(runner.file(Lang::Es, utterances(), Diarization::Run));
    drop(runner);
    let released = async {
        while Arc::strong_count(&fake) > 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), released).await.unwrap();
}
