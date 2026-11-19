use std::time::{Duration, Instant};

use e_voice_daemon::talk::agent::base::{Agent, AgentEvent, Ended, Turn};
use serde_json::Value;

use crate::fake::{ABORTED, DELTAS, INIT_OK, INTERRUPT_REPLY, Rig, SUCCESS, deltas, drain, gone, lines};

#[tokio::test(flavor = "multi_thread")]
async fn test_cancel_writes_the_exact_interrupt_and_the_answering_result_ends_the_turn_cancelled() {
    let rig = Rig::new(&[
        ("on_user", lines(&[INIT_OK, "~PART {\"type\":\"noise\"}\n", "\n"])),
        ("on_interrupt", INTERRUPT_REPLY.to_owned()),
    ]);
    let mut turn = rig.agent.turn("cuenta".to_owned()).await.unwrap();
    let started = Instant::now();
    turn.cancel().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert!(turn.next().await.is_none(), "the turn has ended when cancel returns");
    let sent = rig.stdin(1);
    assert_eq!(sent.len(), 2, "{sent:?}");
    let interrupt: Value = serde_json::from_str(&sent[1]).unwrap();
    let object = interrupt.as_object().unwrap();
    assert_eq!(object.len(), 3);
    assert_eq!(interrupt["type"], "control_request");
    assert_eq!(interrupt["request"], serde_json::json!({"subtype": "interrupt"}));
    let id = interrupt["request_id"].as_str().unwrap();
    assert_eq!(id.len(), 36);
    assert_eq!(id.matches('-').count(), 4);
    assert_eq!(rig.spawns(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancel_mid_reply_drops_what_came_before_its_result_and_the_session_serves_the_next_turn() {
    let rig = Rig::new(&[
        ("on_user.1.1", lines(&[INIT_OK, DELTAS])),
        ("on_interrupt", INTERRUPT_REPLY.to_owned()),
        ("on_user.1.2", lines(&[INIT_OK, DELTAS, SUCCESS])),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let mut seen = Vec::new();
    while let Some(Ok(AgentEvent::Delta(text))) = first.next().await {
        seen.push(text);
        if seen.len() == 1 {
            break;
        }
    }
    assert_eq!(seen, ["Hola"]);
    first.cancel().await.unwrap();
    assert!(first.next().await.is_none());
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["Hola", ", mundo.\n¿Qué tal?"]);
    assert_eq!(
        events.last(),
        Some(&AgentEvent::Done(Ended::Answered)),
        "the interrupted turn's leftovers did not leak"
    );
    assert_eq!(rig.spawns(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancel_on_a_turn_that_already_ended_does_nothing() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    turn.cancel().await.unwrap();
    assert_eq!(rig.stdin(1).len(), 1, "no interrupt was sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_session_that_ignores_the_interrupt_is_killed_after_five_seconds_and_the_next_turn_resumes_it() {
    let rig = Rig::new(&[
        ("on_user.1", lines(&[INIT_OK, DELTAS])),
        ("on_user.2", lines(&[INIT_OK, DELTAS, SUCCESS])),
        ("grandchild", String::new()),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    first.next().await.unwrap().unwrap();
    let started = Instant::now();
    first.cancel().await.unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(5) && took < Duration::from_secs(7),
        "{took:?}"
    );
    assert!(first.next().await.is_none(), "the turn has ended when cancel returns");
    let grandchild = rig.pid("grandchild.1").unwrap();
    assert!(
        gone(grandchild, Duration::from_secs(5)).await,
        "the killed session's group is gone"
    );
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (_, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(rig.spawns(), 2);
    let id = rig
        .argv(1)
        .into_iter()
        .find_map(|arg| arg.strip_prefix("--session-id=").map(str::to_owned))
        .unwrap();
    assert!(rig.argv(2).contains(&format!("--resume={id}")));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_result_that_is_not_a_cancel_after_an_interrupt_still_ends_the_turn() {
    let rig = Rig::new(&[("on_user", INIT_OK.to_owned()), ("on_interrupt", lines(&[SUCCESS]))]);
    let mut turn = rig.agent.turn("uno".to_owned()).await.unwrap();
    turn.cancel().await.unwrap();
    assert!(turn.next().await.is_none());
    let _ = ABORTED;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_close_ends_the_session_with_term_and_leaves_no_orphan_in_its_group() {
    let rig = Rig::new(&[
        ("on_user", lines(&[INIT_OK, DELTAS, SUCCESS])),
        ("grandchild", String::new()),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    drop(turn);
    let grandchild = rig.pid("grandchild.1").unwrap();
    let started = Instant::now();
    rig.agent.close().await;
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    assert!(
        gone(grandchild, Duration::from_secs(5)).await,
        "an orphan outlived close"
    );
    rig.agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_close_waits_the_grace_for_a_session_that_ignores_term_then_kills_its_group() {
    let rig = Rig::new(&[
        ("on_user", lines(&[INIT_OK, DELTAS, SUCCESS])),
        ("grandchild", String::new()),
        ("trap_term", String::new()),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    drop(turn);
    let grandchild = rig.pid("grandchild.1").unwrap();
    let started = Instant::now();
    rig.agent.close().await;
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(5) && took < Duration::from_secs(7),
        "{took:?}"
    );
    assert!(
        gone(grandchild, Duration::from_secs(5)).await,
        "the group was not killed after the grace"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_close_before_any_turn_is_a_no_op_and_a_turn_after_close_starts_a_resumed_session() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))]);
    rig.agent.close().await;
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    drain(&mut first).await;
    drop(first);
    rig.agent.close().await;
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (_, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(rig.spawns(), 2);
    assert!(!rig.argv(1).iter().any(|arg| arg.starts_with("--resume")));
    assert!(rig.argv(2).iter().any(|arg| arg.starts_with("--resume=")));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dropping_the_agent_kills_the_whole_group() {
    let rig = Rig::new(&[
        ("on_user", lines(&[INIT_OK, DELTAS, SUCCESS])),
        ("grandchild", String::new()),
    ]);
    let config = e_voice_daemon::config::talk::ClaudeConfig {
        bin: rig.bin(),
        cwd: Some(rig.dir.clone()),
        ..Default::default()
    };
    let agent = e_voice_daemon::talk::agent::claude::ClaudeAgent::new(
        config,
        "http://127.0.0.1:1/mcp".to_owned(),
        "PROMPT".to_owned(),
    );
    let mut turn = agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    drop(turn);
    let grandchild = rig.pid("grandchild.1").unwrap();
    drop(agent);
    assert!(
        gone(grandchild, Duration::from_secs(5)).await,
        "the group survived the agent"
    );
}
