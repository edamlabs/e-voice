#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use std::process::{Command, Output};

/// `ecli` as a user's terminal sees it: piped, no width hint, none of its own variables set.
fn ecli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ecli"))
        .args(args)
        .env_remove("ECLI_DAEMON_URL")
        .env_remove("ECLI_STT_URL")
        .env_remove("ECLI_TTS_URL")
        .env_remove("COLUMNS")
        .output()
        .unwrap()
}

/// A golden keeps every line's end visible as `¶`, so no fixer can strip a trailing space it must not touch:
/// this restores the exact bytes the binary printed, final newline included.
fn unmarked(golden: &str) -> String {
    let lines: Vec<&str> = golden
        .lines()
        .map(|line| line.strip_suffix('¶').unwrap_or_else(|| panic!("no ¶ on {line:?}")))
        .collect();
    lines.join("\n") + "\n"
}

fn stdout(args: &[&str]) -> String {
    let output = ecli(args);
    assert_eq!(output.status.code(), Some(0), "{args:?}");
    assert!(
        output.stderr.is_empty(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn test_version_is_byte_identical_to_main() {
    assert_eq!(stdout(&["--version"]), unmarked(include_str!("golden/version.txt")));
    assert_eq!(stdout(&["-V"]), unmarked(include_str!("golden/version.txt")));
}

/// The goldens below are the output of `main`'s binary, taken byte for byte when the daemon verbs were added.
#[test]
fn test_stt_tts_and_devices_helps_are_byte_identical_to_main() {
    assert_eq!(stdout(&["stt", "--help"]), unmarked(include_str!("golden/stt.txt")));
    assert_eq!(stdout(&["tts", "--help"]), unmarked(include_str!("golden/tts.txt")));
    assert_eq!(
        stdout(&["devices", "--help"]),
        unmarked(include_str!("golden/devices.txt"))
    );
    assert_eq!(stdout(&["help", "stt"]), unmarked(include_str!("golden/stt.txt")));
    assert_eq!(stdout(&["help", "tts"]), unmarked(include_str!("golden/tts.txt")));
    assert_eq!(
        stdout(&["help", "devices"]),
        unmarked(include_str!("golden/devices.txt"))
    );
}

#[test]
fn test_the_top_level_help_is_main_s_with_exactly_four_lines_appended_after_devices() {
    let main = unmarked(include_str!("golden/top.txt"));
    let (before, after) = main.split_once("  devices").unwrap();
    let (devices, rest) = after.split_once('\n').unwrap();
    let lines = [
        "  say      Speak a text through the daemon and wait until it has played",
        "  stop     Silence the daemon's voice and drop every text waiting",
        "  listen   Listen once through the daemon and print what was heard",
        "  talk     Start or stop a voice conversation with the agent through the daemon",
    ];
    let expected = format!("{before}  devices{devices}\n{}\n{rest}", lines.join("\n"));
    let seen = stdout(&["--help"]);
    assert_eq!(seen, expected);
    assert_eq!(seen.lines().count(), main.lines().count() + 4);
    assert_eq!(stdout(&["help"]), expected);
}

#[test]
fn test_the_help_of_the_three_daemon_verbs_is_pinned() {
    assert_eq!(stdout(&["say", "--help"]), unmarked(include_str!("golden/say.txt")));
    assert_eq!(stdout(&["stop", "--help"]), unmarked(include_str!("golden/stop.txt")));
    assert_eq!(
        stdout(&["listen", "--help"]),
        unmarked(include_str!("golden/listen.txt"))
    );
}

#[test]
fn test_a_bare_ecli_and_an_unknown_verb_exit_2_with_the_usage() {
    for args in [
        &[][..],
        &["bogus"][..],
        &["say"][..],
        &["say", "a", "b"][..],
        &["listen", "--bogus"][..],
    ] {
        let output = ecli(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty() || args.is_empty(), "{args:?}");
        let text = String::from_utf8_lossy(&output.stderr).into_owned() + &String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("Usage: ecli"), "{args:?}: {text}");
    }
}
