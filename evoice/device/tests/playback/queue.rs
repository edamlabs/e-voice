use std::num::NonZeroU32;

use e_voice_core::audio::{AudioEncoding, AudioIngest};
use e_voice_device::playback::queue::PlaybackQueue;

fn ramp(from: usize, to: usize) -> Vec<f32> {
    (from..to).map(|n| (n as f32 * 0.37).sin()).collect()
}

fn sine(rate: u32, hertz: f32, seconds: f32) -> Vec<f32> {
    (0..(rate as f32 * seconds) as usize)
        .map(|n| 0.5 * (2.0 * std::f32::consts::PI * hertz * n as f32 / rate as f32).sin())
        .collect()
}

fn drain(queue: &mut PlaybackQueue) -> Vec<f32> {
    std::iter::from_fn(|| queue.pop()).collect()
}

fn put(queue: &mut PlaybackQueue, samples: &[f32], rate: u32, target: u32) {
    queue.push(samples, NonZeroU32::new(rate).unwrap(), target).unwrap();
}

fn close(left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len());
    for (index, (a, b)) in left.iter().zip(right).enumerate() {
        assert!((a - b).abs() < 1e-6, "sample {index}: {a} vs {b}");
    }
}

/// One stream through the queue: every sample pushed, then finished.
fn once(samples: &[f32], rate: u32, target: u32) -> Vec<f32> {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, samples, rate, target);
    queue.finish().unwrap();
    drain(&mut queue)
}

/// What core's resampler gives for the same stream: the oracle the queue must match.
fn oracle(samples: &[f32], rate: u32, target: u32) -> Vec<f32> {
    let mut ingest = AudioIngest::new(rate, target, AudioEncoding::F32le).unwrap();
    let mut out = ingest.feed(samples).unwrap();
    out.extend(ingest.flush().unwrap());
    out
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn crossings(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0 || pair[0] >= 0.0 && pair[1] < 0.0)
        .count()
}

#[test]
fn test_equal_rates_are_the_identity_sample_for_sample() {
    let signal = ramp(0, 777);
    assert_eq!(once(&signal, 16_000, 16_000), signal);
}

#[test]
fn test_equal_rates_across_calls_are_the_identity_and_finish_adds_nothing() {
    let signal = ramp(1, 65);
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &signal[..20], 16_000, 16_000);
    assert_eq!(queue.len(), 20);
    put(&mut queue, &signal[20..], 16_000, 16_000);
    queue.finish().unwrap();
    assert_eq!(drain(&mut queue), signal);
}

#[test]
fn test_the_length_after_finish_is_what_the_core_resampler_gives() {
    for n in [1usize, 7, 100, 1_000, 1_023, 1_024, 1_025, 5_000, 24_000] {
        let signal = ramp(0, n);
        for (rate, target) in [(24_000, 48_000), (44_100, 48_000), (48_000, 24_000), (16_000, 48_000)] {
            assert_eq!(
                once(&signal, rate, target).len(),
                oracle(&signal, rate, target).len(),
                "{n} samples {rate}→{target}"
            );
        }
    }
}

#[test]
fn test_the_content_after_finish_is_what_the_core_resampler_gives() {
    let signal = ramp(0, 5_000);
    close(&once(&signal, 24_000, 48_000), &oracle(&signal, 24_000, 48_000));
}

/// Observed, not designed: the resampler works in whole 1024-sample input chunks and pads the last one with zeros,
/// so n samples at 24 kHz → 48 kHz come out as 2048 · ceil(n / 1024).
#[test]
fn test_pinned_length_formula_for_24k_to_48k() {
    for n in [1usize, 100, 1_000, 1_024, 1_025, 2_048, 24_000] {
        assert_eq!(
            once(&ramp(0, n), 24_000, 48_000).len(),
            2_048 * n.div_ceil(1_024),
            "{n} samples"
        );
    }
}

#[test]
fn test_a_sine_keeps_its_zero_crossings_and_its_level_through_the_resampler() {
    let signal = sine(24_000, 1_000.0, 1.0);
    let out = once(&signal, 24_000, 48_000);
    let edge = 2_048;
    let middle = &out[edge..out.len() - edge];
    let expected = (middle.len() as f32 / 48_000.0 * 2_000.0) as usize;
    let seen = crossings(middle);
    assert!(seen.abs_diff(expected) <= 3, "crossings {seen} against {expected}");
    let drift = 20.0 * (rms(middle) / rms(&signal)).log10();
    assert!(drift.abs() < 0.5, "level moved {drift:.3} dB");
}

#[test]
fn test_a_rate_change_plays_the_old_tail_before_the_new_stream() {
    let (first, second) = (ramp(0, 3_000), ramp(500, 3_500));
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &first, 24_000, 48_000);
    put(&mut queue, &second, 44_100, 48_000);
    queue.finish().unwrap();
    let played = drain(&mut queue);
    let old = oracle(&first, 24_000, 48_000);
    let new = oracle(&second, 44_100, 48_000);
    close(&played[..old.len()], &old);
    close(&played[old.len()..], &new);
}

#[test]
fn test_finish_on_an_empty_queue_is_a_no_op_and_clear_then_push_starts_clean() {
    let mut queue = PlaybackQueue::default();
    queue.finish().unwrap();
    queue.finish().unwrap();
    assert!(queue.is_empty());
    let signal = ramp(0, 2_000);
    put(&mut queue, &signal[..700], 44_100, 48_000);
    queue.clear();
    assert!(queue.is_empty());
    put(&mut queue, &signal, 44_100, 48_000);
    queue.finish().unwrap();
    close(&drain(&mut queue), &once(&signal, 44_100, 48_000));
}

#[test]
fn test_clear_drops_what_the_resampler_still_holds() {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &ramp(0, 300), 24_000, 48_000);
    queue.clear();
    queue.finish().unwrap();
    assert_eq!(queue.pop(), None);
}

#[test]
fn test_pop_returns_samples_in_the_order_pushed() {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &[0.1, 0.2, 0.3], 16_000, 16_000);
    put(&mut queue, &[0.4, 0.5], 16_000, 16_000);
    assert_eq!(drain(&mut queue), [0.1, 0.2, 0.3, 0.4, 0.5]);
    assert_eq!(queue.pop(), None);
    assert!(queue.is_empty());
}

#[test]
fn test_the_smallest_source_rate_terminates_or_errors_never_hangs() {
    let mut queue = PlaybackQueue::default();
    let outcome = queue.push(&[0.5; 4], NonZeroU32::MIN, 48_000);
    assert!(outcome.is_ok() || outcome.is_err());
}

#[test]
fn test_a_zero_target_rate_does_not_panic() {
    let mut queue = PlaybackQueue::default();
    let _ = queue.push(&ramp(0, 10), NonZeroU32::new(16_000).unwrap(), 0);
}

#[test]
fn test_a_rate_beyond_the_bound_is_an_error_and_the_queue_stays_usable() {
    let mut queue = PlaybackQueue::default();
    for rate in [u32::MAX, e_voice_core::audio::MAX_RATE + 1] {
        let refused = queue.push(&[0.5; 4], NonZeroU32::new(rate).unwrap(), 48_000);
        assert!(refused.is_err(), "{rate}");
        assert!(queue.is_empty());
    }
    let signal = ramp(0, 100);
    put(&mut queue, &signal, 16_000, 16_000);
    queue.finish().unwrap();
    assert_eq!(drain(&mut queue), signal);
}

#[test]
fn test_the_bound_itself_is_played() {
    let mut queue = PlaybackQueue::default();
    queue
        .push(
            &ramp(0, 2_000),
            NonZeroU32::new(e_voice_core::audio::MAX_RATE).unwrap(),
            48_000,
        )
        .unwrap();
    queue.finish().unwrap();
    assert!(!queue.is_empty());
}
