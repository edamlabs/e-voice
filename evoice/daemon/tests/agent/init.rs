use std::time::Duration;

use e_voice_daemon::talk::agent::base::{Agent, AgentError, Turn};

use crate::fake::{
    DELTAS, INIT_AUTO, INIT_FAILED, INIT_NONE, INIT_OK, INIT_TWO, INIT_WRONG, Rig, SUCCESS, TEXT_BEFORE_INIT, drain,
    gone, lines,
};

async fn refused(init: &str, expect: &str) {
    let rig = Rig::new(&[
        ("on_user", lines(&[init, DELTAS, SUCCESS])),
        ("grandchild", String::new()),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert!(
        events.is_empty(),
        "{expect}: nothing may be said before the startup is checked: {events:?}"
    );
    assert!(
        matches!(&error, Some(AgentError::Refused(reason)) if !reason.is_empty()),
        "{expect}: {error:?}"
    );
    let grandchild = rig.pid("grandchild.1").unwrap();
    assert!(
        gone(grandchild, Duration::from_secs(5)).await,
        "{expect}: the process group was not killed"
    );
    assert!(turn.next().await.is_none(), "{expect}: the turn is over");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_startup_that_is_not_the_asked_for_one_is_refused_and_its_group_killed() {
    refused(INIT_AUTO, "mode auto").await;
    refused(INIT_NONE, "no server").await;
    refused(INIT_FAILED, "our server failed").await;
    refused(INIT_TWO, "two servers").await;
    refused(INIT_WRONG, "a server that is not ours").await;
    refused(TEXT_BEFORE_INIT, "text before init").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_reason_of_a_refusal_names_what_was_wrong() {
    for (init, word) in [(INIT_AUTO, "auto"), (INIT_TWO, "MCP")] {
        let rig = Rig::new(&[("on_user", lines(&[init, DELTAS, SUCCESS]))]);
        let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
        let (_, error) = drain(&mut turn).await;
        match error {
            Some(AgentError::Refused(reason)) => assert!(reason.contains(word), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_after_a_refusal_the_next_turn_starts_a_fresh_resumed_session() {
    let rig = Rig::new(&[
        ("on_user.1", lines(&[INIT_AUTO])),
        ("on_user.2", lines(&[INIT_OK, DELTAS, SUCCESS])),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let (_, error) = drain(&mut first).await;
    assert!(matches!(error, Some(AgentError::Refused(_))));
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (_, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(rig.spawns(), 2);
    assert!(rig.argv(2).iter().any(|arg| arg.starts_with("--resume=")));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_binary_that_does_not_exist_is_a_start_error_not_a_hang() {
    let rig = Rig::with(&[], |config| config.bin = "/nonexistent/claude".to_owned());
    let error = rig.agent.turn("hola".to_owned()).await.unwrap_err();
    assert!(matches!(error, AgentError::Start(_)), "{error:?}");
    let again = rig.agent.turn("hola".to_owned()).await.unwrap_err();
    assert!(matches!(again, AgentError::Start(_)), "{again:?}");
}

#[tokio::test(start_paused = true)]
async fn test_a_session_that_says_nothing_for_sixty_seconds_is_a_start_error() {
    let rig = Rig::new(&[("on_user", "~SLEEP 300\n".to_owned()), ("grandchild", String::new())]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let started = tokio::time::Instant::now();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(events, Vec::new());
    assert!(matches!(error, Some(AgentError::Start(_))), "{error:?}");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(60) && waited < Duration::from_secs(61),
        "{waited:?}"
    );
    drop(turn);
}
