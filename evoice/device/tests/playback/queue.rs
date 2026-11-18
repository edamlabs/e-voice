use std::num::NonZeroU32;

use e_voice_device::playback::queue::PlaybackQueue;

fn put(queue: &mut PlaybackQueue, samples: &[f32], rate: u32, target: u32) {
    queue.push(samples, NonZeroU32::new(rate).unwrap(), target);
}

fn ramp(from: usize, to: usize) -> Vec<f32> {
    (from..to).map(|n| (n as f32 * 0.37).sin()).collect()
}

fn drain(queue: &mut PlaybackQueue) -> Vec<f32> {
    std::iter::from_fn(|| queue.pop()).collect()
}

fn close(left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len());
    for (index, (a, b)) in left.iter().zip(right).enumerate() {
        assert!((a - b).abs() < 1e-6, "sample {index}: {a} vs {b}");
    }
}

fn once(samples: &[f32], rate: u32, target: u32) -> Vec<f32> {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, samples, rate, target);
    drain(&mut queue)
}

#[test]
fn test_push_doubles_the_length_when_the_target_is_twice_the_rate() {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &ramp(0, 1_000), 24_000, 48_000);
    assert_eq!(queue.len(), 2_000);
    assert!(!queue.is_empty());
}

#[test]
fn test_push_halves_the_length_when_the_target_is_half_the_rate() {
    assert_eq!(once(&ramp(0, 1_000), 48_000, 24_000).len(), 500);
}

#[test]
fn test_push_keeps_the_length_at_equal_rates() {
    assert_eq!(once(&ramp(0, 777), 16_000, 16_000).len(), 777);
}

/// The resampler of the old speaker, pinned: at equal rates the output is the input delayed by one
/// source sample (the first sample out is the initial `last`, 0.0), length preserved, the final input
/// sample held back; identity and the tail flush belong to the rubato cut.
#[test]
fn test_push_at_equal_rates_delays_by_exactly_one_sample() {
    let signal = ramp(1, 65);
    let mut expected = vec![0.0];
    expected.extend_from_slice(&signal[..signal.len() - 1]);
    close(&once(&signal, 16_000, 16_000), &expected);
}

#[test]
fn test_the_delay_holds_across_calls_at_equal_rates() {
    let signal = ramp(1, 65);
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &signal[..20], 16_000, 16_000);
    put(&mut queue, &signal[20..], 16_000, 16_000);
    let mut expected = vec![0.0];
    expected.extend_from_slice(&signal[..signal.len() - 1]);
    close(&drain(&mut queue), &expected);
}

#[test]
fn test_upsampling_interpolates_linearly_between_neighbours() {
    let out = once(&[1.0, 3.0, 5.0, 7.0], 24_000, 48_000);
    let middles: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    close(&middles, &[0.5, 2.0, 4.0, 6.0]);
}

#[test]
fn test_two_pushes_equal_one_push_of_the_whole_at_every_split() {
    let signal = ramp(0, 600);
    for (rate, target) in [
        (24_000, 48_000),
        (44_100, 48_000),
        (48_000, 44_100),
        (16_000, 48_000),
        (22_050, 48_000),
    ] {
        let whole = once(&signal, rate, target);
        for split in [1, 7, 100, 299, 599] {
            let mut queue = PlaybackQueue::default();
            put(&mut queue, &signal[..split], rate, target);
            put(&mut queue, &signal[split..], rate, target);
            close(&drain(&mut queue), &whole);
        }
    }
}

#[test]
fn test_clear_drops_the_queue_but_keeps_the_phase() {
    let signal = ramp(0, 400);
    for (rate, target) in [(44_100, 48_000), (24_000, 48_000)] {
        let whole = once(&signal, rate, target);
        let mut queue = PlaybackQueue::default();
        put(&mut queue, &signal[..150], rate, target);
        let dropped = queue.len();
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.pop(), None);
        put(&mut queue, &signal[150..], rate, target);
        close(&drain(&mut queue), &whole[dropped..]);
    }
}

#[test]
fn test_pop_returns_samples_in_the_order_pushed() {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &[0.1, 0.2, 0.3], 16_000, 16_000);
    put(&mut queue, &[0.4, 0.5], 16_000, 16_000);
    let out = drain(&mut queue);
    assert_eq!(out.len(), 5);
    assert_eq!(queue.pop(), None);
    assert!(queue.is_empty());
}

#[test]
fn test_a_zero_target_rate_does_not_panic() {
    let mut queue = PlaybackQueue::default();
    put(&mut queue, &ramp(0, 10), 16_000, 0);
    assert!(queue.len() <= 10 * 16_000);
}

#[test]
fn test_the_smallest_source_rate_terminates_with_the_exact_length() {
    let mut queue = PlaybackQueue::default();
    queue.push(&[0.5; 4], NonZeroU32::MIN, 48_000);
    assert_eq!(queue.len(), 4 * 48_000);
}
