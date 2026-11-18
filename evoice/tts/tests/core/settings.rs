use std::path::PathBuf;

use e_voice_tts::core::settings::Settings;

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
