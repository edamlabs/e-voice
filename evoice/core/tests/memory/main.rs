#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss
)]
//! Its own test binary: the peak resident set is per process, so this must be the only test in it.

use std::time::Duration;

use e_voice_core::audio::{AudioEncoding, AudioFile, AudioIngest};

#[allow(dead_code)]
#[path = "../support/flac.rs"]
mod flac;

fn ingest(rate: u32, target: u32) -> Result<AudioIngest, e_voice_core::audio::AudioError> {
    AudioIngest::new(rate, target, AudioEncoding::F32le)
}

fn peak_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .unwrap()
}

#[test]
fn test_refusing_rates_costs_nothing_and_the_worst_legal_pair_is_bounded() {
    let before = peak_kib();
    for _ in 0..1_000 {
        let _ = ingest(u32::MAX, 48_000);
    }
    let refused = peak_kib().saturating_sub(before);
    assert!(refused < 20_000, "refusing grew the peak by {refused} KiB");
    let before = peak_kib();
    let mut worst = ingest(383_999, 48_000).unwrap();
    let signal: Vec<f32> = (0..383_999).map(|n| (n as f32 * 0.01).sin()).collect();
    let mut out = worst.feed(&signal).unwrap();
    out.extend(worst.flush().unwrap());
    let grown_mib = peak_kib().saturating_sub(before) / 1024;
    println!(
        "WORST_PAIR 383999→48000: {} samples out, peak grew {grown_mib} MiB",
        out.len()
    );
    assert_ne!(out.len(), 0);
    assert!(
        grown_mib < 1_024,
        "the worst legal pair grew the peak by {grown_mib} MiB"
    );
    let before = peak_kib();
    let bomb = flac::silent_flac(10 * 3_600, 16_000);
    let refused = AudioFile::decode(bomb, Some("flac"), 16_000, Duration::from_secs(10));
    let grown_mib = peak_kib().saturating_sub(before) / 1024;
    println!("BOMB 10 h of silence, cap 10 s: {refused:?}, peak grew {grown_mib} MiB");
    assert!(refused.is_err());
    assert!(
        grown_mib < 100,
        "a 10 h bomb with a 10 s cap grew the peak by {grown_mib} MiB (a whole decode is 2300 MiB)"
    );
}
