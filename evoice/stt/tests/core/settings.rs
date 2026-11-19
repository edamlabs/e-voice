use std::path::PathBuf;

use e_voice_stt::core::settings::Settings;

const PACKAGED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/evoice.toml");

fn load(name: &str, text: &str) -> Result<Settings, String> {
    let path: PathBuf = std::env::temp_dir().join(format!("evoice-stt-{}-{name}.toml", std::process::id()));
    std::fs::write(&path, text).unwrap();
    let loaded = Settings::load(Some(&path)).map_err(|error| error.to_string());
    std::fs::remove_file(&path).ok();
    loaded
}

#[test]
fn test_a_daemon_table_in_the_shared_file_still_loads() {
    let loaded = load("daemon", "[daemon]\nport = 5801\n[daemon.anything]\nx = 1\n");
    assert!(loaded.is_ok(), "{loaded:?}");
}

#[test]
fn test_an_unknown_top_level_table_is_still_refused() {
    assert!(load("unknown", "[voice]\nx = 1\n").is_err());
}

#[test]
fn test_the_packaged_file_loads_and_leaves_the_stt_defaults() {
    let loaded = load("packaged", &std::fs::read_to_string(PACKAGED).unwrap());
    assert_eq!(loaded.unwrap().stt, Settings::default().stt);
}

#[test]
fn test_diarize_values_in_range_are_accepted() {
    let loaded = load(
        "diarize-ok",
        "[stt.pipeline.diarize]\nthreads = 1\nthreshold = 0.01\nspeakers = 1\n",
    );
    assert!(loaded.is_ok(), "{loaded:?}");
}

#[test]
fn test_diarize_speakers_unset_is_accepted() {
    assert!(load("diarize-unset", "[stt.pipeline.diarize]\nthreshold = 0.7\n").is_ok());
}

#[test]
fn test_diarize_threshold_out_of_range_is_refused() {
    for (name, value) in [
        ("zero", "0.0"),
        ("negative", "-0.5"),
        ("nan", "nan"),
        ("inf", "inf"),
        ("neginf", "-inf"),
    ] {
        let error = load(name, &format!("[stt.pipeline.diarize]\nthreshold = {value}\n")).unwrap_err();
        assert_eq!(
            error, "invalid settings: `stt.pipeline.diarize.threshold` must be finite and positive",
            "{value}"
        );
    }
}

#[test]
fn test_diarize_speakers_zero_is_refused() {
    let error = load("speakers", "[stt.pipeline.diarize]\nspeakers = 0\n").unwrap_err();
    assert_eq!(
        error,
        "invalid settings: `stt.pipeline.diarize.speakers` must be at least 1"
    );
}

#[test]
fn test_diarize_threads_zero_is_refused() {
    let error = load("threads", "[stt.pipeline.diarize]\nthreads = 0\n").unwrap_err();
    assert_eq!(error, "invalid settings: `stt.pipeline.*.threads` must be at least 1");
}
