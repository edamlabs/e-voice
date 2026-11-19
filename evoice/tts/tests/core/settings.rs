use std::path::PathBuf;

use e_voice_tts::config::synth::SynthBackend;
use e_voice_tts::core::settings::Settings;

const PACKAGED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/evoice.toml");

fn load(name: &str, text: &str) -> Result<Settings, String> {
    let path: PathBuf = std::env::temp_dir().join(format!("evoice-tts-{}-{name}.toml", std::process::id()));
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
fn test_the_packaged_file_loads_as_qwen3_two_workers_four_threads() {
    let loaded = load("packaged", &std::fs::read_to_string(PACKAGED).unwrap()).unwrap();
    let mut expected = Settings::default();
    expected.tts.synth.backend = SynthBackend::Qwen3;
    expected.tts.synth.workers = 2;
    expected.tts.synth.threads = 4;
    assert_eq!(loaded, expected);
}
