use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use e_voice_daemon::config::talk::ClaudeConfig;
use e_voice_daemon::talk::agent::base::{Agent, AgentError, AgentEvent, Ended};
use e_voice_daemon::talk::agent::claude::ClaudeAgent;

use crate::fake::{DELTAS, INIT_OK, Rig, SUCCESS, TOKEN, deltas, drain, lines};

#[tokio::test(flavor = "multi_thread")]
async fn test_a_child_that_writes_to_stderr_never_blocks_the_turn() {
    let burst = "e".repeat(2 * 1024 * 1024) + "\n";
    let rig = Rig::new(&[
        (
            "on_user",
            lines(&[
                "~ERR una línea por stderr\n",
                INIT_OK,
                "~CATERR burst\n",
                DELTAS,
                SUCCESS,
            ]),
        ),
        ("burst", burst),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = tokio::time::timeout(Duration::from_secs(20), drain(&mut turn))
        .await
        .expect("the turn hung on a full stderr pipe");
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["Hola", ", mundo.\n¿Qué tal?"]);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_binary_that_does_not_exist_is_a_typed_start_error_every_time_until_it_does() {
    let rig = Rig::with(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))], |config| {
        config.bin = "/nonexistent-dir/claude".to_owned();
    });
    for _ in 0..2 {
        let error = rig.agent.turn("hola".to_owned()).await.unwrap_err();
        let AgentError::Start(reason) = error else {
            panic!("{error:?}")
        };
        assert!(
            reason.contains("No such file") || reason.contains("not found"),
            "{reason}"
        );
    }
    let later = format!("{}", rig.dir.join("claude-later").display());
    let agent = ClaudeAgent::new(
        ClaudeConfig {
            bin: later.clone(),
            cwd: Some(rig.dir.clone()),
            ..ClaudeConfig::default()
        },
        "http://127.0.0.1:1/mcp".to_owned(),
        "PROMPT".to_owned(),
        TOKEN.to_owned(),
    );
    assert!(matches!(agent.turn("a".to_owned()).await, Err(AgentError::Start(_))));
    fs::copy(rig.bin(), &later).unwrap();
    fs::set_permissions(&later, fs::Permissions::from_mode(0o755)).unwrap();
    let mut turn = agent.turn("b".to_owned()).await.expect("the binary exists now");
    let (_, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    let argv = rig.read("argv.1");
    assert!(
        argv.contains("--session-id="),
        "a failed spawn must not count as a started session:\n{argv}"
    );
    assert!(!argv.contains("--resume"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_binary_that_is_not_executable_or_is_a_directory_is_a_start_error() {
    let rig = Rig::new(&[]);
    let plain = rig.dir.join("plain-file");
    fs::write(&plain, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
    let directory = rig.dir.join("a-directory");
    fs::create_dir_all(&directory).unwrap();
    for bin in [plain, directory] {
        let agent = ClaudeAgent::new(
            ClaudeConfig {
                bin: bin.to_string_lossy().into_owned(),
                cwd: Some(rig.dir.clone()),
                ..ClaudeConfig::default()
            },
            "http://127.0.0.1:1/mcp".to_owned(),
            "PROMPT".to_owned(),
            TOKEN.to_owned(),
        );
        let error = agent.turn("hola".to_owned()).await.unwrap_err();
        assert!(
            matches!(error, AgentError::Start(ref reason) if !reason.is_empty()),
            "{bin:?}: {error:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_working_directory_that_does_not_exist_is_a_start_error() {
    let rig = Rig::with(&[], |config| config.cwd = Some("/nonexistent-dir/project".into()));
    let error = rig.agent.turn("hola".to_owned()).await.unwrap_err();
    assert!(matches!(error, AgentError::Start(_)), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_child_that_closes_its_stdin_makes_the_next_write_fail_and_the_session_resets_and_resumes() {
    let rig = Rig::new(&[
        ("on_user.1", lines(&[INIT_OK, DELTAS, SUCCESS, "~CLOSEIN\n"])),
        ("on_user.2", lines(&[INIT_OK, DELTAS, SUCCESS])),
        ("grandchild", String::new()),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let (_, error) = drain(&mut first).await;
    assert_eq!(error, None);
    drop(first);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let failed = rig.agent.turn("dos".to_owned()).await;
    assert!(
        matches!(failed, Err(AgentError::Gone)),
        "{:?}",
        failed.map(|_| "a turn")
    );
    let mut third = rig.agent.turn("tres".to_owned()).await.unwrap();
    let (events, error) = drain(&mut third).await;
    assert_eq!(error, None);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
    assert_eq!(rig.spawns(), 2);
    let id = rig
        .argv(1)
        .into_iter()
        .find_map(|arg| arg.strip_prefix("--session-id=").map(str::to_owned))
        .unwrap();
    assert!(rig.argv(2).contains(&format!("--resume={id}")));
    assert_eq!(rig.stdin(2).len(), 1, "the resumed process gets only the new turn");
    let grandchild = rig.pid("grandchild.1").unwrap();
    assert!(
        crate::fake::gone(grandchild, Duration::from_secs(5)).await,
        "the closed session's group is gone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_no_debug_output_of_the_agent_or_its_turn_prints_the_secret_the_prompt_the_url_or_the_environment() {
    const SECRET: &str = "SECRET-c0ffee";
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))]);
    let agent = ClaudeAgent::new(
        ClaudeConfig {
            bin: rig.bin(),
            cwd: Some(rig.dir.clone()),
            ..ClaudeConfig::default()
        },
        "http://127.0.0.1:4242/mcp-private-path".to_owned(),
        "PROMPT-text-private".to_owned(),
        SECRET.to_owned(),
    );
    let turn = agent.turn("hola".to_owned()).await.unwrap();
    for (what, plain, pretty) in [
        ("ClaudeAgent", format!("{agent:?}"), format!("{agent:#?}")),
        ("ClaudeTurn", format!("{turn:?}"), format!("{turn:#?}")),
    ] {
        for shown in [&plain, &pretty] {
            for private in [
                SECRET,
                "c0ffee",
                "PROMPT-text-private",
                "mcp-private-path",
                "4242",
                "EVOICE_APPROVER",
                "HOME=",
                "PATH=",
            ] {
                assert!(!shown.contains(private), "{what} prints {private}:\n{shown}");
            }
        }
    }
    assert!(format!("{agent:?}").contains("ClaudeAgent"));
    drop(turn);
}
