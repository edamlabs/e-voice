use std::process::Command;

use e_voice_daemon::talk::agent::base::Agent;
use e_voice_daemon::talk::agent::base::AgentEvent;

use crate::fake::{DELTAS, INIT_OK, Rig, SUCCESS, deltas, drain, lines};

fn on_user() -> String {
    lines(&[INIT_OK, DELTAS, SUCCESS])
}

const SETTINGS: &str = r#"--settings={"permissions":{"ask":["Bash","Edit","Write","mcp__*"],"deny":["mcp__evoice__speak","mcp__evoice__stop","mcp__evoice__listen","mcp__evoice__list_voices"]}}"#;
const MCP: &str = r#"--mcp-config={"mcpServers":{"evoice":{"headers":{"x-evoice-approver":"${EVOICE_APPROVER}"},"type":"http","url":"http://127.0.0.1:1/mcp"}}}"#;

fn expected(session: &str, model: Option<&str>, tools: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format=stream-json",
        "--output-format=stream-json",
        "--verbose",
        "--include-partial-messages",
        "--setting-sources=project",
        "--permission-mode=manual",
        "--permission-prompts=host",
        "--strict-mcp-config",
        "--permission-prompt-tool=mcp__evoice__approve",
        "--append-system-prompt=PROMPT",
        SETTINGS,
        MCP,
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(session.to_owned());
    args.extend(model.map(|model| format!("--model={model}")));
    args.push("--tools".to_owned());
    args.extend(tools.iter().map(|tool| (*tool).to_owned()));
    args
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_argv_is_exact_with_the_tools_last_and_nothing_positional() {
    let rig = Rig::new(&[("on_user", on_user())]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done(e_voice_daemon::talk::agent::base::Ended::Answered))
    );
    let argv = rig.argv(1);
    let session = argv
        .iter()
        .find(|arg| arg.starts_with("--session-id="))
        .unwrap()
        .clone();
    let id = session.trim_start_matches("--session-id=");
    assert_eq!(id.len(), 36);
    assert_eq!(id.matches('-').count(), 4);
    assert_eq!(
        argv,
        expected(&session, Some("sonnet"), &["Read", "Bash", "Edit", "Write"])
    );
    assert_eq!(argv.last().map(String::as_str), Some("Write"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_without_a_model_none_is_passed_and_the_tools_follow_the_config() {
    let rig = Rig::with(&[("on_user", on_user())], |config| {
        config.model = None;
        config.tools = vec!["Read".to_owned(), "Glob".to_owned(), "Bash".to_owned()];
    });
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    let argv = rig.argv(1);
    let session = argv
        .iter()
        .find(|arg| arg.starts_with("--session-id="))
        .unwrap()
        .clone();
    let mut want = expected(&session, None, &["Read", "Glob", "Bash"]);
    let at = want.iter().position(|arg| arg.starts_with("--settings=")).unwrap();
    want[at] = r#"--settings={"permissions":{"ask":["Bash","mcp__*"],"deny":["mcp__evoice__speak","mcp__evoice__stop","mcp__evoice__listen","mcp__evoice__list_voices"]}}"#.to_owned();
    assert_eq!(argv, want);
    assert!(!argv.iter().any(|arg| arg.starts_with("--model")));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_prompt_that_looks_like_a_flag_or_holds_quotes_is_one_value_not_a_new_option() {
    let rig = Rig::new(&[("on_user", on_user())]);
    let agent = e_voice_daemon::talk::agent::claude::ClaudeAgent::new(
        e_voice_daemon::config::talk::ClaudeConfig {
            bin: rig.bin(),
            cwd: Some(rig.dir.clone()),
            ..Default::default()
        },
        "http://127.0.0.1:1/mcp".to_owned(),
        "--dangerous \"quoted\" and $(whoami) `x`\nsecond line".to_owned(),
        crate::fake::TOKEN.to_owned(),
    );
    let mut turn = agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    let text = rig.read("argv.1");
    assert!(
        text.contains("--append-system-prompt=--dangerous \"quoted\" and $(whoami) `x`"),
        "{text}"
    );
    assert!(text.contains("second line"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_session_killed_is_resumed_by_the_same_id_after_a_kill() {
    let rig = Rig::new(&[("on_user.1", lines(&[INIT_OK, "~EXIT\n"])), ("on_user.2", on_user())]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let (_, error) = drain(&mut first).await;
    assert_eq!(error, Some(e_voice_daemon::talk::agent::base::AgentError::Gone));
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events).concat(), "Hola, mundo.\n¿Qué tal?");
    assert_eq!(rig.spawns(), 2);
    let (one, two) = (rig.argv(1), rig.argv(2));
    let id = one.iter().find_map(|arg| arg.strip_prefix("--session-id=")).unwrap();
    assert_eq!(two.iter().find_map(|arg| arg.strip_prefix("--resume=")), Some(id));
    assert!(!two.iter().any(|arg| arg.starts_with("--session-id")));
    assert_eq!(
        expected(
            &format!("--resume={id}"),
            Some("sonnet"),
            &["Read", "Bash", "Edit", "Write"]
        ),
        two
    );
    assert_eq!(rig.stdin(2).len(), 1, "the resumed process gets only the new turn");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_each_turn_writes_one_exact_user_line_and_the_process_is_spawned_once() {
    let rig = Rig::new(&[("on_user", on_user())]);
    for text in ["hola", "¿Qué \"tal\"?\nsegunda línea", ""] {
        let mut turn = rig.agent.turn(text.to_owned()).await.unwrap();
        let (_, error) = drain(&mut turn).await;
        assert_eq!(error, None);
    }
    assert_eq!(rig.spawns(), 1);
    let sent = rig.stdin(1);
    assert_eq!(sent.len(), 3, "{sent:?}");
    let parsed: Vec<serde_json::Value> = sent.iter().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(parsed[0]["message"]["content"], "hola");
    assert_eq!(parsed[1]["message"]["content"], "¿Qué \"tal\"?\nsegunda línea");
    assert_eq!(parsed[2]["message"]["content"], "");
    for value in &parsed {
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert!(value["parent_tool_use_id"].is_null());
        assert_eq!(value.as_object().unwrap().len(), 3);
    }
}

fn env_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_owned()))
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_the_child_sees_none_of_the_sessions_identity_and_keeps_what_it_needs() {
    const MARK: &str = "EVOICE_AGENT_ENV_INNER";
    if std::env::var_os(MARK).is_some() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let rig = Rig::new(&[("on_user", on_user())]);
            let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
            drain(&mut turn).await;
            let seen = rig.read("env.1");
            let names = env_names(&seen);
            for gone in [
                "CLAUDECODE",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_CODE_MESSAGING_TOKEN",
                "CLAUDE_CODE_MESSAGING_SOCKET",
                "CLAUDE_PID",
                "FOO",
                MARK,
                "RUST_BACKTRACE",
            ] {
                assert!(
                    !names.iter().any(|name| name == gone),
                    "{gone} reached the child:\n{seen}"
                );
            }
            assert!(
                !names.iter().any(|name| name.starts_with("CARGO")),
                "cargo's variables reached the child:\n{seen}"
            );
            for kept in [
                "HOME",
                "PATH",
                "LC_ALL",
                "ANTHROPIC_TEST",
                "AWS_TEST",
                "https_proxy",
                "NODE_EXTRA_CA_CERTS",
                "TZ",
            ] {
                assert!(
                    names.iter().any(|name| name == kept),
                    "{kept} did not reach the child:\n{seen}"
                );
            }
            let secret: Vec<&str> = seen
                .lines()
                .filter(|line| line.starts_with("EVOICE_APPROVER="))
                .collect();
            assert_eq!(
                secret,
                [format!("EVOICE_APPROVER={}", crate::fake::TOKEN)],
                "the child's secret is the agent's own, not the parent's:\n{seen}"
            );
            assert!(
                !seen.contains("parent-approver-secret"),
                "the parent's value reached the child"
            );
            let argv = rig.read("argv.1");
            assert!(
                !argv.contains(crate::fake::TOKEN),
                "the secret is in the child's argv:\n{argv}"
            );
            let shell = ["PWD", "OLDPWD", "SHLVL", "_"];
            let allowed = [
                "EVOICE_APPROVER",
                "HOME",
                "PATH",
                "USER",
                "LOGNAME",
                "SHELL",
                "LANG",
                "LANGUAGE",
                "TZ",
                "TMPDIR",
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
                "XDG_STATE_HOME",
                "XDG_RUNTIME_DIR",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLOUD_ML_REGION",
                "GOOGLE_APPLICATION_CREDENTIALS",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "NO_PROXY",
                "http_proxy",
                "https_proxy",
                "no_proxy",
                "SSL_CERT_FILE",
                "SSL_CERT_DIR",
                "NODE_EXTRA_CA_CERTS",
            ];
            let prefixed = ["LC_", "ANTHROPIC_", "AWS_"];
            let extra: Vec<&String> = names
                .iter()
                .filter(|name| {
                    !shell.contains(&name.as_str())
                        && !allowed.contains(&name.as_str())
                        && !prefixed.iter().any(|prefix| name.starts_with(prefix))
                })
                .collect();
            assert!(extra.is_empty(), "unexpected variables {extra:?}");
        });
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "argv::test_the_child_sees_none_of_the_sessions_identity_and_keeps_what_it_needs",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MARK, "1")
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "session-secret")
        .env("CLAUDE_CODE_MESSAGING_TOKEN", "token-secret")
        .env("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/socket-secret")
        .env("CLAUDE_PID", "4242")
        .env("FOO", "bar")
        .env("EVOICE_APPROVER", "parent-approver-secret")
        .env("RUST_BACKTRACE", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("ANTHROPIC_TEST", "x")
        .env("AWS_TEST", "x")
        .env("https_proxy", "http://127.0.0.1:9")
        .env("NODE_EXTRA_CA_CERTS", "/nonexistent.pem")
        .env("TZ", "UTC")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() && text.contains("1 passed"), "{text}");
}
