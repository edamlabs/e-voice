use std::time::Duration;

use e_voice_daemon::talk::agent::base::{Agent, AgentEvent, Ended, Turn};

use crate::fake::{DELTAS, INIT_OK, PLACEHOLDER, Rig, SUCCESS, deltas, drain, lines};

#[tokio::test(flavor = "multi_thread")]
async fn test_a_second_turn_waits_until_the_first_is_dropped() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    drain(&mut first).await;
    let agent = &rig.agent;
    let waiting = async { agent.turn("dos".to_owned()).await };
    tokio::pin!(waiting);
    let early = tokio::time::timeout(Duration::from_millis(400), &mut waiting).await;
    assert!(early.is_err(), "the second turn started while the first was alive");
    assert_eq!(rig.stdin(1).len(), 1, "nothing was written for it yet");
    drop(first);
    let mut second = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap();
    let (_, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(rig.stdin(1).len(), 2);
    assert_eq!(rig.spawns(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_turn_dropped_unfinished_does_not_leak_its_lines_into_the_next_one() {
    let first_reply = lines(&[INIT_OK, DELTAS, SUCCESS]);
    let second_reply = format!(
        r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"respuesta dos"}}}},"session_id":"{PLACEHOLDER}","parent_tool_use_id":null}}"#
    ) + "\n";
    let rig = Rig::new(&[
        ("on_user.1.1", first_reply),
        ("on_user.1.2", lines(&[&second_reply, SUCCESS])),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let Some(Ok(AgentEvent::Delta(text))) = first.next().await else {
        panic!("no first delta")
    };
    assert_eq!(text, "Hola");
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(
        deltas(&events),
        ["respuesta dos"],
        "the second turn heard the unread rest of the first: {events:?}"
    );
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
}

#[test]
fn test_the_fixtures_hold_no_personal_data_only_placeholders() {
    let all = [
        crate::fake::INIT_OK,
        crate::fake::INIT_AUTO,
        crate::fake::INIT_NONE,
        crate::fake::INIT_FAILED,
        crate::fake::INIT_TWO,
        crate::fake::INIT_WRONG,
        crate::fake::TEXT_BEFORE_INIT,
        crate::fake::STATUS,
        crate::fake::DELTAS,
        crate::fake::SUCCESS,
        crate::fake::ABORTED,
        crate::fake::ERROR_DURING,
        crate::fake::MAX_TURNS,
        crate::fake::INTERRUPT_REPLY,
    ]
    .concat();
    for banned in [
        "@",
        "/home/",
        "accountUuid",
        "organizationUuid",
        "userEmail",
        "damien",
        "/run/user",
        "secret",
        "claude.ai",
    ] {
        assert!(!all.contains(banned), "{banned} in a fixture");
    }
    let mut at = 0;
    while let Some(found) = all[at..].find("-4000-8000-") {
        let start = at + found;
        assert!(start >= 13, "{start}");
        assert_eq!(
            &all[start - 13..start],
            "00000000-0000",
            "an id that is not a placeholder near {start}"
        );
        assert!(
            all[start + 11..start + 23]
                .chars()
                .all(|c| c == '0' || c.is_ascii_hexdigit())
        );
        at = start + 1;
    }
    assert!(all.matches("-4000-8000-").count() > 20);
    for line in all.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|error| panic!("{error}: {line}"));
    }
}

fn only_delta(text: &str) -> String {
    format!(
        r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":{}}}}},"session_id":"{PLACEHOLDER}","parent_tool_use_id":null}}"#,
        serde_json::to_string(text).unwrap()
    ) + "\n"
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_turn_dropped_mid_reply_is_settled_and_the_next_turn_hears_only_its_own_answer() {
    let rig = Rig::new(&[
        ("on_user.1.1", lines(&[INIT_OK, DELTAS])),
        ("on_interrupt", lines(&[crate::fake::INTERRUPT_REPLY])),
        ("on_user.1.2", lines(&[&only_delta("respuesta dos"), SUCCESS])),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let Some(Ok(AgentEvent::Delta(text))) = first.next().await else {
        panic!("no first delta")
    };
    assert_eq!(text, "Hola");
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["respuesta dos"]);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
    assert_eq!(rig.spawns(), 1, "a settled session is kept");
    let sent = rig.stdin(1);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent[1].contains(r#""subtype":"interrupt""#), "{}", sent[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_turn_dropped_before_any_next_is_settled_the_same_way() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, &only_delta("respuesta dos"), SUCCESS]))]);
    let first = rig.agent.turn("uno".to_owned()).await.unwrap();
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["respuesta dos"], "the first turn's text leaked");
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
    let last = rig.argv(rig.spawns());
    assert!(
        last.iter().any(|arg| arg.starts_with("--resume=")) && !last.iter().any(|arg| arg.starts_with("--session-id")),
        "a session never checked at startup cannot be settled, so the new turn resumes it: {last:?}"
    );
    assert_eq!(
        rig.stdin(rig.spawns()),
        [r#"{"message":{"content":"dos","role":"user"},"parent_tool_use_id":null,"type":"user"}"#]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_dropped_turn_whose_session_ignores_the_interrupt_is_killed_within_five_seconds_and_resumed() {
    let rig = Rig::new(&[
        ("on_user.1.1", lines(&[INIT_OK, DELTAS])),
        ("on_user.2", lines(&[INIT_OK, &only_delta("respuesta dos"), SUCCESS])),
        ("grandchild", String::new()),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    first.next().await.unwrap().unwrap();
    drop(first);
    let started = std::time::Instant::now();
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(7), "{took:?}");
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["respuesta dos"]);
    assert_eq!(rig.spawns(), 2);
    let id = rig
        .argv(1)
        .into_iter()
        .find_map(|arg| arg.strip_prefix("--session-id=").map(str::to_owned))
        .unwrap();
    assert!(rig.argv(2).contains(&format!("--resume={id}")));
    let grandchild = rig.pid("grandchild.1").unwrap();
    assert!(
        crate::fake::gone(grandchild, Duration::from_secs(5)).await,
        "the killed session's group is gone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_turn_dropped_after_its_result_owes_nothing() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    drain(&mut first).await;
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    drain(&mut second).await;
    assert_eq!(rig.stdin(1).len(), 2, "no interrupt was sent for a finished turn");
    assert_eq!(rig.spawns(), 1);
}
