use std::path::PathBuf;

use e_voice_daemon::core::settings::Settings;

fn file(name: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("evoiced-{}-{name}.toml", std::process::id()));
    std::fs::write(&path, text).unwrap();
    path
}

fn load(name: &str, text: &str) -> Result<Settings, String> {
    let path = file(name, text);
    let loaded = Settings::load(Some(&path)).map_err(|error| error.to_string());
    std::fs::remove_file(&path).ok();
    loaded
}

#[test]
fn test_defaults_listen_on_5700() {
    assert_eq!(Settings::default().daemon.port, 5700);
}

#[test]
fn test_daemon_port_is_read_from_its_table() {
    assert_eq!(load("port", "[daemon]\nport = 5801\n").unwrap().daemon.port, 5801);
}

#[test]
fn test_unknown_key_in_the_daemon_table_is_refused() {
    let error = load("unknown", "[daemon]\nporrt = 1\n").unwrap_err();
    assert!(error.contains("porrt"), "{error}");
}

#[test]
fn test_unknown_top_level_table_is_refused() {
    assert!(load("table", "[voice]\nx = 1\n").is_err());
}

#[test]
fn test_the_services_tables_are_accepted_whatever_they_hold() {
    let loaded = load(
        "services",
        "[stt]\nlang = \"es\"\n[stt.api]\nport = 5500\n[tts]\nanything = 1\n[daemon]\nport = 5702\n",
    );
    assert_eq!(loaded.unwrap().daemon.port, 5702);
}

#[test]
fn test_port_zero_is_refused_naming_the_key() {
    let error = load("zero", "[daemon]\nport = 0\n").unwrap_err();
    assert!(error.contains("daemon.port") && error.contains("at least 1"), "{error}");
}

#[test]
fn test_port_out_of_range_or_ill_typed_is_refused() {
    assert!(load("big", "[daemon]\nport = 70000\n").is_err());
    assert!(load("text", "[daemon]\nport = \"x\"\n").is_err());
}

#[test]
fn test_a_missing_explicit_file_is_refused() {
    let missing = std::env::temp_dir().join("evoiced-definitely-missing.toml");
    assert!(Settings::load(Some(&missing)).is_err());
}
