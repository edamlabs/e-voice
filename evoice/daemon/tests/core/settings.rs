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

#[test]
fn test_the_defaults_name_loopback_and_the_local_tts() {
    let daemon = Settings::default().daemon;
    assert_eq!(daemon.host.to_string(), "127.0.0.1");
    assert_eq!(daemon.tts, "ws://127.0.0.1:5600");
    assert_eq!(daemon.output, None);
}

#[test]
fn test_host_tts_and_output_are_read_from_the_daemon_table() {
    let loaded = load(
        "keys",
        "[daemon]\nhost = \"0.0.0.0\"\ntts = \"ws://tts.local:5601\"\noutput = \"Speakers\"\n",
    )
    .unwrap();
    assert_eq!(loaded.daemon.host.to_string(), "0.0.0.0");
    assert_eq!(loaded.daemon.tts, "ws://tts.local:5601");
    assert_eq!(loaded.daemon.output.as_deref(), Some("Speakers"));
}

#[test]
fn test_a_tts_url_that_is_not_ws_is_refused_naming_the_key() {
    for url in ["wss://tts.local", "http://tts.local", "tts.local:5600", ""] {
        let error = load("tts", &format!("[daemon]\ntts = \"{url}\"\n")).unwrap_err();
        assert!(
            error.contains("daemon.tts") && error.contains("ws://"),
            "{url}: {error}"
        );
    }
}

#[test]
fn test_a_host_that_is_not_an_ip_is_refused() {
    assert!(load("host", "[daemon]\nhost = \"localhost\"\n").is_err());
}
