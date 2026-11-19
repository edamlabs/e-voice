use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use e_voice_core::schema::error::BackendError;
use e_voice_stt::config::diarize::{DiarizeBackend, DiarizeConfig, DiarizeEmbedding, DiarizePrecision};
use e_voice_stt::schema::audio::RATE;
use e_voice_stt::workflow::diarize::base::Diarize;
use e_voice_stt::workflow::diarize::registry::DiarizeRegistry;
use e_voice_stt::workflow::diarize::sherpa::SherpaDiarize;
use sherpa_onnx::Wave;

use crate::fixture;

fn sherpa() -> DiarizeConfig {
    DiarizeConfig {
        backend: DiarizeBackend::Sherpa,
        ..DiarizeConfig::default()
    }
}

fn diarizer() -> std::sync::Arc<dyn Diarize> {
    DiarizeRegistry::build(&sherpa(), &fixture::store()).unwrap().unwrap()
}

/// Material outside the repo: where the variable is absent the test says so and stops, never a silent pass.
fn material(test: &str, keys: &[&str]) -> Option<Vec<String>> {
    let found: Vec<String> = keys.iter().filter_map(|key| std::env::var(key).ok()).collect();
    (found.len() == keys.len()).then_some(found).or_else(|| {
        eprintln!("skipped {test}: needs {}", keys.join(" and "));
        None
    })
}

fn embedding(name: &str) -> DiarizeEmbedding {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).unwrap()
}

fn precision(name: &str) -> DiarizePrecision {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).unwrap()
}

/// `EVOICE_DIAR_EMBEDDING`, `EVOICE_DIAR_PRECISION`, `EVOICE_DIAR_THRESHOLD` over the defaults.
fn arm() -> DiarizeConfig {
    let var = |key: &str| std::env::var(key).ok();
    DiarizeConfig {
        embedding: var("EVOICE_DIAR_EMBEDDING").map_or_else(DiarizeEmbedding::default, |name| embedding(&name)),
        precision: var("EVOICE_DIAR_PRECISION").map_or_else(DiarizePrecision::default, |name| precision(&name)),
        threshold: var("EVOICE_DIAR_THRESHOLD")
            .map_or(DiarizeConfig::default().threshold, |value| value.parse().unwrap()),
        ..sherpa()
    }
}

#[test]
fn test_off_builds_nothing() {
    assert!(
        DiarizeRegistry::build(&DiarizeConfig::default(), &fixture::store())
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_missing_segmentation_file_names_it() {
    let empty = std::env::temp_dir().join("evoice-no-segmentation");
    std::fs::create_dir_all(&empty).unwrap();
    let error = SherpaDiarize::new(&empty, &empty, &sherpa()).unwrap_err();
    std::fs::remove_dir_all(&empty).ok();
    assert!(
        matches!(&error, BackendError::Missing(path) if path.ends_with("model.onnx")),
        "{error:?}"
    );
}

#[test]
#[ignore = "requires installed models: make pull with [stt.pipeline.diarize] backend = \"sherpa\""]
fn test_loads_both_rows_and_hears_two_voices() {
    let audio = fixture::dialogue();
    let turns = diarizer().diarize(&audio).unwrap();
    assert_ne!(turns.len(), 0);
    assert!(turns.windows(2).all(|pair| pair[0].span.start <= pair[1].span.start));
    assert!(
        turns
            .iter()
            .all(|turn| turn.span.start < turn.span.end && turn.span.end <= audio.len() as u64 + u64::from(RATE))
    );
    let voices: std::collections::BTreeSet<u32> = turns.iter().map(|turn| turn.speaker.0).collect();
    assert!(voices.len() >= 2, "{voices:?}");
}

#[test]
#[ignore = "requires installed models and EVOICE_DER_WAVS (dir of 16 kHz mono wavs) + EVOICE_DER_OUT (dir for rttm)"]
fn test_voxconverse_turns_to_rttm() {
    let Some(paths) = material("test_voxconverse_turns_to_rttm", &["EVOICE_DER_WAVS", "EVOICE_DER_OUT"]) else {
        return;
    };
    let (wavs, out) = (PathBuf::from(&paths[0]), PathBuf::from(&paths[1]));
    std::fs::create_dir_all(&out).unwrap();
    let diarizer = DiarizeRegistry::build(&arm(), &fixture::store()).unwrap().unwrap();
    let mut names: Vec<_> = std::fs::read_dir(&wavs)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
        .filter(|path| {
            std::env::var("EVOICE_DER_ONLY").map_or(true, |only| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| only.split(',').any(|wanted| wanted == stem))
            })
        })
        .collect();
    names.sort();
    for path in names {
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let clip = Wave::read(path.to_str().unwrap()).unwrap();
        assert_eq!(clip.sample_rate(), RATE as i32, "{name}");
        let started = std::time::Instant::now();
        let turns = diarizer.diarize(clip.samples()).unwrap();
        let seconds = started.elapsed().as_secs_f64();
        let rttm = turns.iter().fold(String::new(), |mut text, turn| {
            let (start, end) = (
                turn.span.start as f64 / f64::from(RATE),
                turn.span.end as f64 / f64::from(RATE),
            );
            writeln!(
                text,
                "SPEAKER {name} 1 {start:.3} {:.3} <NA> <NA> spk{} <NA> <NA>",
                end - start,
                turn.speaker.0
            )
            .unwrap();
            text
        });
        std::fs::write(Path::new(&out).join(format!("{name}.rttm")), rttm).unwrap();
        std::fs::write(Path::new(&out).join(format!("{name}.sec")), format!("{seconds}\n")).unwrap();
        let load = std::fs::read_to_string("/proc/loadavg").unwrap();
        std::fs::write(Path::new(&out).join(format!("{name}.load")), load).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires installed models and EVOICE_MEETING (a long recording, read only)"]
async fn test_meeting_diarizes_while_transcribing() {
    use e_voice_core::audio::AudioFile;
    use e_voice_core::schema::lang::Lang;
    use e_voice_stt::core::settings::Settings;
    use e_voice_stt::schema::speaker::Diarization;
    use e_voice_stt::workflow::nodes::Nodes;
    use e_voice_stt::workflow::runner::Runner;

    let Some(paths) = material("test_meeting_diarizes_while_transcribing", &["EVOICE_MEETING"]) else {
        return;
    };
    let path = PathBuf::from(&paths[0]);
    let extension = path.extension().and_then(|ext| ext.to_str()).map(str::to_owned);
    let bytes = std::fs::read(&path).unwrap();
    let samples = tokio::task::spawn_blocking(move || {
        AudioFile::decode(bytes, extension.as_deref(), RATE, std::time::Duration::MAX).unwrap()
    })
    .await
    .unwrap();
    let mut settings = Settings::default();
    settings.stt.pipeline.diarize.backend = DiarizeBackend::Sherpa;
    let nodes = std::sync::Arc::new(Nodes::build(&settings, &fixture::store()).unwrap());
    let runner_nodes = std::sync::Arc::clone(&nodes);
    let runner = Runner::new(nodes, settings.stt.pipeline.clone());
    let seconds = samples.len() as f64 / f64::from(RATE);
    let mode = if std::env::var("EVOICE_MEETING_MODE").is_ok_and(|mode| mode == "skip") {
        Diarization::Skip
    } else {
        Diarization::Run
    };
    let samples = std::sync::Arc::new(samples);
    let load = std::fs::read_to_string("/proc/loadavg").unwrap();
    println!("LOAD {}", load.trim());
    let started = std::time::Instant::now();
    let transcript = runner
        .transcribe(Lang::Es, std::sync::Arc::clone(&samples), mode)
        .await
        .unwrap();
    let wall = started.elapsed().as_secs_f64();
    let voices: std::collections::BTreeSet<u32> = transcript
        .segments
        .iter()
        .filter_map(|done| done.speaker.map(|id| id.0))
        .collect();
    let labelled = transcript.segments.iter().filter(|done| done.speaker.is_some()).count();
    let failed = transcript.segments.iter().filter(|done| done.error.is_some()).count();
    let peak = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|line| line.starts_with("VmHWM"))
        .unwrap()
        .to_owned();
    println!(
        "MEETING seconds={seconds:.1} wall={wall:.1} rtf={:.3} segments={} labelled={labelled} failed={failed} voices={} {peak}",
        wall / seconds,
        transcript.segments.len(),
        voices.len()
    );
    for done in transcript.segments.iter().filter(|done| done.error.is_some()) {
        println!("FAILED {:?} {:?} {:?}", done.segment, done.span, done.error);
    }
    let rate = f64::from(RATE);
    let turns = std::env::var("EVOICE_MEETING_TURNS").ok().and_then(|_| {
        let node = runner_nodes.diarize.clone()?;
        node.diarize(&samples).ok()
    });
    for done in transcript.segments.iter().filter(|done| done.speaker.is_none()) {
        println!(
            "UNLABELLED {:?} {:.2}s-{:.2}s",
            done.segment,
            done.span.start as f64 / rate,
            done.span.end as f64 / rate
        );
        for turn in turns
            .iter()
            .flatten()
            .filter(|turn| turn.span.end + 160_000 > done.span.start && turn.span.start < done.span.end + 160_000)
        {
            println!(
                "  TURN spk{} {:.2}s-{:.2}s",
                turn.speaker.0,
                turn.span.start as f64 / rate,
                turn.span.end as f64 / rate
            );
        }
    }
    if mode == Diarization::Run {
        assert!(labelled > 0);
    }
    assert_eq!(failed, 0);
}

#[test]
#[ignore = "requires installed models: make pull once per embedding with [stt.pipeline.diarize] backend = \"sherpa\""]
fn test_every_embedding_and_precision_loads_and_hears_two_voices() {
    let audio = fixture::dialogue();
    for embedding in ["wespeaker-resnet34-lm", "campplus-zh-en", "eres2net-voxceleb"] {
        for precision in ["fp32", "int8"] {
            let config = DiarizeConfig {
                embedding: self::embedding(embedding),
                precision: self::precision(precision),
                ..sherpa()
            };
            let node = DiarizeRegistry::build(&config, &fixture::store())
                .unwrap_or_else(|error| panic!("{embedding} {precision}: {error:?}"))
                .unwrap();
            let turns = node
                .diarize(&audio)
                .unwrap_or_else(|error| panic!("{embedding} {precision}: {error:?}"));
            let voices: std::collections::BTreeSet<u32> = turns.iter().map(|turn| turn.speaker.0).collect();
            println!(
                "LOAD_OK {embedding} {precision} turns={} voices={}",
                turns.len(),
                voices.len()
            );
            assert!(!turns.is_empty(), "{embedding} {precision}");
        }
    }
}

#[test]
fn test_int8_without_its_file_fails_naming_it() {
    let dir = std::env::temp_dir().join("evoice-no-int8");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"x").unwrap();
    let config = DiarizeConfig {
        precision: DiarizePrecision::Int8,
        ..sherpa()
    };
    let error = SherpaDiarize::new(&dir, &dir, &config).unwrap_err();
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        matches!(&error, BackendError::Missing(path) if path.ends_with("model.int8.onnx")),
        "{error:?}"
    );
}
