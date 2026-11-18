use std::time::Duration;

use e_voice_core::audio::{AudioError, AudioFile};

use crate::flac::silent_flac;

const RATE: u32 = 16_000;

/// 32 s of 16 kHz silence is exactly 125 blocks of 4096 samples: no padding blurs the cap.
fn thirty_two_seconds() -> Vec<u8> {
    silent_flac(32, RATE)
}

fn decode(bytes: Vec<u8>, limit: Duration) -> Result<Vec<f32>, AudioError> {
    AudioFile::decode(bytes, Some("flac"), RATE, limit)
}

fn tone() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/resources/audio/tone.flac")).unwrap()
}

#[test]
fn test_the_hand_built_silent_flac_decodes_to_its_length() {
    let samples = decode(silent_flac(3, RATE), Duration::MAX).unwrap();
    assert_eq!(samples.len(), 49_152, "3 s at 16 kHz is 12 blocks of 4096");
    assert!(samples.iter().all(|sample| *sample == 0.0));
}

#[test]
fn test_an_hour_of_silence_is_a_small_file() {
    let bytes = silent_flac(3_600, RATE);
    assert!(bytes.len() < 400_000, "{} bytes", bytes.len());
}

#[test]
fn test_cap_is_the_duration_in_samples_of_the_target() {
    assert_eq!(AudioFile::cap(Duration::from_secs(30), 24_000), 720_000);
    assert_eq!(AudioFile::cap(Duration::from_hours(4), 16_000), 230_400_000);
    assert_eq!(AudioFile::cap(Duration::ZERO, 16_000), 0);
}

#[test]
fn test_audio_exactly_at_the_limit_is_accepted() {
    let samples = decode(thirty_two_seconds(), Duration::from_secs(32)).unwrap();
    assert_eq!(samples.len(), 32 * RATE as usize);
}

#[test]
fn test_audio_one_second_past_the_limit_is_refused_naming_the_limit() {
    let refused = decode(thirty_two_seconds(), Duration::from_secs(31));
    assert!(
        matches!(refused, Err(AudioError::TooLong(limit)) if limit == Duration::from_secs(31)),
        "{refused:?}"
    );
    assert!(refused.unwrap_err().to_string().contains("31 s"));
}

#[test]
fn test_head_is_the_prefix_of_a_whole_decode_and_stops_at_the_count() {
    let whole = decode(tone(), Duration::MAX).unwrap();
    for count in [0usize, 1, 999, 1_024, 5_000, whole.len() - 1] {
        let head = AudioFile::head(tone(), Some("flac"), RATE, count).unwrap();
        assert_eq!(head, whole[..count], "{count} samples");
    }
}

#[test]
fn test_head_past_the_end_returns_the_whole_file() {
    let whole = decode(tone(), Duration::MAX).unwrap();
    assert_eq!(
        AudioFile::head(tone(), Some("flac"), RATE, whole.len() + 10_000).unwrap(),
        whole
    );
}

#[test]
fn test_head_of_something_that_is_not_audio_is_an_error() {
    assert!(AudioFile::head(vec![1, 2, 3, 4, 5, 6, 7, 8], Some("flac"), RATE, 100).is_err());
}
