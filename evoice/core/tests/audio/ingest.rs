use e_voice_core::audio::{AudioEncoding, AudioError, AudioIngest, MAX_RATE};

fn ingest(rate: u32, target: u32) -> Result<AudioIngest, AudioError> {
    AudioIngest::new(rate, target, AudioEncoding::F32le)
}

#[test]
fn test_a_rate_beyond_the_bound_is_refused_before_anything_is_allocated() {
    for rate in [u32::MAX, MAX_RATE + 1, 1_000_000] {
        assert!(
            matches!(ingest(rate, 48_000), Err(AudioError::Rate(refused)) if refused == rate),
            "{rate}"
        );
        assert!(
            matches!(ingest(48_000, rate), Err(AudioError::Rate(refused)) if refused == rate),
            "{rate}"
        );
    }
}

#[test]
fn test_zero_rates_are_refused() {
    assert!(matches!(ingest(0, 48_000), Err(AudioError::Rate(0))));
    assert!(matches!(ingest(48_000, 0), Err(AudioError::Rate(0))));
}

#[test]
fn test_the_bound_itself_is_accepted() {
    assert_eq!(MAX_RATE, 384_000);
    assert!(ingest(MAX_RATE, 48_000).is_ok());
    assert!(ingest(48_000, MAX_RATE).is_ok());
    assert!(ingest(MAX_RATE, MAX_RATE).is_ok());
}
