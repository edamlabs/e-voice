use std::time::Duration;

use e_voice_core::audio::AudioFile;
use e_voice_tts::schema::audio::Audio;
use e_voice_tts::workflow::synth::prompt::Prompt;

const RATE: u32 = Prompt::RATE;
const CLIPS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/resources/audio");

fn file(name: &str) -> (Vec<u8>, Option<String>) {
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_owned());
    (std::fs::read(format!("{CLIPS}/{name}")).unwrap(), extension)
}

/// A mono s16 WAV of a steady tone, long enough to pass the limit on its own.
fn tone_wav(seconds: u32) -> (Vec<u8>, Option<String>) {
    let samples: Vec<i16> = (0..seconds * 16_000)
        .map(|n| ((n as f32 * 0.05).sin() * 9_000.0) as i16)
        .collect();
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    for part in [
        b"RIFF".as_slice(),
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &16_000u32.to_le_bytes(),
        &32_000u32.to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
        &samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<u8>>(),
    ] {
        bytes.extend_from_slice(part);
    }
    (bytes, Some("wav".to_owned()))
}

/// What main did: every clip decoded whole, then the join cut at the limit by `Prompt::prepare`.
fn whole(files: &[(Vec<u8>, Option<String>)]) -> Vec<Audio> {
    files
        .iter()
        .map(|(bytes, extension)| {
            Audio::from(AudioFile::decode(bytes.clone(), extension.as_deref(), RATE, Duration::MAX).unwrap())
        })
        .collect()
}

fn flat(clips: &[Audio]) -> Vec<f32> {
    clips.iter().flat_map(|clip| clip.iter().copied()).collect()
}

fn same_as_main(files: Vec<(Vec<u8>, Option<String>)>) {
    let old = Prompt::prepare(&whole(&files), Some("hola")).unwrap();
    let new = Prompt::prepare(&Prompt::clips(files, RATE).unwrap(), Some("hola")).unwrap();
    assert_eq!(new, old);
    assert_eq!(new.encode().unwrap(), old.encode().unwrap());
    assert!(new.audio.len() <= AudioFile::cap(Prompt::LIMIT, RATE) + RATE as usize);
}

#[test]
fn test_clips_under_the_limit_make_the_prompt_main_made() {
    same_as_main(vec![file("tone.wav"), file("tone.flac"), file("tone.mp3")]);
}

#[test]
fn test_clips_over_the_limit_make_the_prompt_main_made() {
    let files: Vec<_> = (0..5)
        .flat_map(|_| {
            [
                "tone.wav",
                "tone.flac",
                "tone.mp3",
                "tone.ogg",
                "tone.opus",
                "tone.m4a",
                "tone.webm",
                "webkit.m4a",
            ]
        })
        .map(file)
        .collect();
    let total: f32 = flat(&whole(&files)).len() as f32 / RATE as f32;
    assert!(total > 30.0, "the fixtures only add up to {total} s");
    same_as_main(files);
}

#[test]
fn test_every_container_decodes_the_same_through_head_and_a_whole_decode() {
    for name in [
        "tone.wav",
        "tone.flac",
        "tone.mp3",
        "tone.ogg",
        "tone.opus",
        "tone.m4a",
        "tone.webm",
    ] {
        let (bytes, extension) = file(name);
        let all = AudioFile::decode(bytes.clone(), extension.as_deref(), RATE, Duration::MAX).unwrap();
        for count in [1usize, 777, all.len() / 2, all.len()] {
            let head = AudioFile::head(bytes.clone(), extension.as_deref(), RATE, count).unwrap();
            assert_eq!(head, all[..count.min(all.len())], "{name} head {count}");
        }
    }
}

#[test]
fn test_one_long_clip_is_cut_at_the_limit_and_the_prompt_is_the_one_main_made() {
    same_as_main(vec![tone_wav(45)]);
}

#[test]
fn test_clips_hold_at_most_the_limit_in_all() {
    let clips = Prompt::clips(vec![tone_wav(20), tone_wav(20), tone_wav(20)], RATE).unwrap();
    assert_eq!(
        clips.len(),
        3,
        "a clip past the budget is kept as an empty one, in order"
    );
    let total: usize = clips.iter().map(|clip| clip.iter().count()).sum();
    assert_eq!(total, AudioFile::cap(Prompt::LIMIT, RATE));
    assert_eq!(clips[2].iter().count(), 0);
}

#[test]
fn test_a_clip_past_the_budget_that_is_not_audio_is_still_an_error() {
    let garbage = (vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], Some("wav".to_owned()));
    assert!(Prompt::clips(vec![tone_wav(45), garbage], RATE).is_err());
}

#[test]
fn test_the_limit_is_thirty_seconds() {
    assert_eq!(Prompt::LIMIT, Duration::from_secs(30));
    assert_eq!(AudioFile::cap(Prompt::LIMIT, RATE), 720_000);
}
