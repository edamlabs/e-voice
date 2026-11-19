use std::path::PathBuf;
use std::time::Duration;

use e_voice_core::config::server::ServerConfig;
use e_voice_daemon::config::daemon::DaemonConfig;
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

#[test]
fn test_the_defaults_name_the_local_stt_and_a_fifteen_second_listen() {
    let daemon = Settings::default().daemon;
    assert_eq!(daemon.stt, "ws://127.0.0.1:5500");
    assert_eq!(daemon.listen, Duration::from_secs(15));
    assert_eq!(daemon.input, None);
}

#[test]
fn test_listen_and_stt_and_input_are_read_from_the_daemon_table() {
    let loaded = load(
        "listen",
        "[daemon]\nlisten = \"500ms\"\nstt = \"ws://stt.local:5501\"\ninput = \"Headset\"\n",
    )
    .unwrap();
    assert_eq!(loaded.daemon.listen, Duration::from_millis(500));
    assert_eq!(loaded.daemon.stt, "ws://stt.local:5501");
    assert_eq!(loaded.daemon.input.as_deref(), Some("Headset"));
}

#[test]
fn test_listen_at_the_longest_is_accepted_and_above_it_or_zero_is_refused_naming_the_key() {
    assert_eq!(
        load("l300", "[daemon]\nlisten = \"300s\"\n").unwrap().daemon.listen,
        Duration::from_secs(300)
    );
    assert_eq!(
        load("l5m", "[daemon]\nlisten = \"5m\"\n").unwrap().daemon.listen,
        Duration::from_secs(300)
    );
    assert_eq!(
        load("l1ms", "[daemon]\nlisten = \"1ms\"\n").unwrap().daemon.listen,
        Duration::from_millis(1)
    );
    for value in ["301s", "0s", "0ms", "1h"] {
        let error = load("lbad", &format!("[daemon]\nlisten = \"{value}\"\n")).unwrap_err();
        assert!(
            error.contains("daemon.listen") && error.contains("300 s"),
            "{value}: {error}"
        );
    }
}

#[test]
fn test_a_listen_that_is_not_a_duration_is_refused() {
    for value in ["\"soon\"", "15", "\"-1s\"", "\"\"", "true"] {
        assert!(
            load("lkind", &format!("[daemon]\nlisten = {value}\n")).is_err(),
            "{value}"
        );
    }
}

#[test]
fn test_a_stt_url_that_is_not_ws_is_refused_naming_the_key() {
    for url in ["wss://stt.local", "http://stt.local", "stt.local:5500", ""] {
        let error = load("stt", &format!("[daemon]\nstt = \"{url}\"\n")).unwrap_err();
        assert!(
            error.contains("daemon.stt") && error.contains("ws://"),
            "{url}: {error}"
        );
    }
}

#[test]
fn test_the_example_file_loads_and_its_daemon_block_is_the_code_defaults() {
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evoice.example.toml");
    let settings = Settings::load(Some(&example)).unwrap();
    assert_eq!(settings.daemon, DaemonConfig::default());
    assert_eq!(settings.server, ServerConfig::default());
}

#[test]
fn test_the_voices_table_is_read_from_the_daemon_table_and_empty_by_default() {
    assert!(Settings::default().daemon.voices.is_empty());
    let loaded = load(
        "voices",
        "[daemon.voices]\nlens = \"narrator\"\n\"claude-code\" = \"ef_dora\"\n",
    )
    .unwrap();
    let table: Vec<(&str, &str)> = loaded
        .daemon
        .voices
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(table, [("claude-code", "ef_dora"), ("lens", "narrator")]);
    assert_eq!(loaded.daemon.port, 5700);
}

#[test]
fn test_a_voices_table_that_is_not_names_to_names_is_refused() {
    for text in [
        "[daemon.voices]\nlens = 3\n",
        "[daemon]\nvoices = \"narrator\"\n",
        "[daemon]\nvoices = [\"a\"]\n",
    ] {
        assert!(load("badvoices", text).is_err(), "{text}");
    }
}

#[test]
fn test_lang_defaults_to_es_and_reads_es_or_en() {
    use e_voice_core::schema::lang::Lang;
    assert_eq!(Settings::default().daemon.lang, Lang::Es);
    assert_eq!(
        load("langen", "[daemon]\nlang = \"en\"\n").unwrap().daemon.lang,
        Lang::En
    );
    assert_eq!(
        load("langes", "[daemon]\nlang = \"es\"\n").unwrap().daemon.lang,
        Lang::Es
    );
}

#[test]
fn test_a_lang_that_is_not_one_of_the_two_is_refused() {
    for value in ["\"fr\"", "\"ES\"", "\"\"", "1", "true"] {
        assert!(
            load("langbad", &format!("[daemon]\nlang = {value}\n")).is_err(),
            "{value}"
        );
    }
}

#[test]
fn test_the_talk_table_defaults_to_claude_with_the_read_and_change_tools() {
    let talk = Settings::default().daemon.talk;
    assert_eq!(talk.claude.bin, "claude");
    assert_eq!(talk.claude.cwd, None);
    assert_eq!(talk.claude.model, None);
    assert_eq!(talk.claude.tools, ["Read", "Bash", "Edit", "Write"]);
}

#[test]
fn test_the_talk_claude_table_is_read_under_daemon_and_every_key_parses() {
    let loaded = load(
        "talk",
        "[daemon.talk.claude]\nbin = \"/opt/claude\"\ncwd = \"/work\"\nmodel = \"sonnet\"\ntools = [\"Read\", \"Grep\"]\n",
    )
    .unwrap();
    let claude = loaded.daemon.talk.claude;
    assert_eq!(claude.bin, "/opt/claude");
    assert_eq!(claude.cwd.as_deref(), Some(std::path::Path::new("/work")));
    assert_eq!(claude.model.as_deref(), Some("sonnet"));
    assert_eq!(claude.tools, ["Read", "Grep"]);
    assert_eq!(loaded.daemon.port, 5700);
}

#[test]
fn test_an_unknown_key_in_the_talk_tables_is_refused_and_a_top_level_talk_table_is_not_accepted() {
    for text in [
        "[daemon.talk.claude]\nbinn = \"x\"\n",
        "[daemon.talk]\nother = 1\n",
        "[talk.claude]\nbin = \"x\"\n",
    ] {
        assert!(load("talkbad", text).is_err(), "{text}");
    }
}

#[test]
fn test_ill_typed_talk_values_are_refused() {
    for text in [
        "[daemon.talk.claude]\ntools = \"Read\"\n",
        "[daemon.talk.claude]\nbin = 3\n",
        "[daemon.talk.claude]\nmodel = true\n",
    ] {
        assert!(load("talktype", text).is_err(), "{text}");
    }
}

#[test]
fn test_the_packaged_file_loads_and_leaves_the_daemon_defaults() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/evoice.toml")).unwrap();
    let loaded = load("packaged", &text).unwrap();
    assert_eq!(loaded.daemon, Settings::default().daemon);
    assert_eq!(loaded.server, Settings::default().server);
}
