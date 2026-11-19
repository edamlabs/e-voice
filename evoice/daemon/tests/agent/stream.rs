use std::time::Duration;

use e_voice_daemon::talk::agent::base::{Agent, AgentError, AgentEvent, Ended, Turn};

use crate::fake::{
    ABORTED, DELTAS, ERROR_DURING, INIT_OK, MAX_TURNS, PLACEHOLDER, Rig, STATUS, SUCCESS, deltas, drain, lines,
};

fn delta(text: &str) -> String {
    format!(
        r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":{}}}}},"session_id":"{PLACEHOLDER}","parent_tool_use_id":null,"uuid":"00000000-0000-4000-8000-0000000000ff","api_message_id":"msg_PLACEHOLDER"}}"#,
        serde_json::to_string(text).unwrap()
    ) + "\n"
}

#[tokio::test(flavor = "multi_thread")]
async fn test_only_main_thread_text_is_spoken_thinking_tool_input_and_other_lines_are_not() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, STATUS, DELTAS, SUCCESS]))]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["Hola", ", mundo.\n¿Qué tal?"]);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
    assert_eq!(events.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_end_of_a_turn_is_one_event_and_the_turn_then_yields_nothing() {
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, DELTAS, SUCCESS]))]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    drain(&mut turn).await;
    assert!(turn.next().await.is_none());
    assert!(turn.next().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_garbage_and_unknown_lines_between_events_are_skipped() {
    let junk = "not json at all\n{}\n[]\n{\"type\":\"unheard-of\"}\n{\"type\":\"stream_event\"}\n\n";
    let rig = Rig::new(&[(
        "on_user",
        lines(&[INIT_OK, junk, &delta("uno"), junk, &delta(" dos"), SUCCESS]),
    )]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["uno", " dos"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_every_result_maps_to_its_ending() {
    let rig = Rig::new(&[
        ("on_user.1.1", lines(&[INIT_OK, SUCCESS])),
        ("on_user.1.2", ABORTED.to_owned()),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    assert_eq!(drain(&mut first).await, (vec![AgentEvent::Done(Ended::Answered)], None));
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    assert_eq!(
        drain(&mut second).await,
        (vec![AgentEvent::Done(Ended::Cancelled)], None)
    );
    assert_eq!(rig.spawns(), 1, "a cancelled turn leaves the session alive");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_other_results_are_a_failed_turn_naming_the_subtype_and_the_message() {
    for (result, expected) in [
        (ERROR_DURING, "error_during_execution: boom"),
        (MAX_TURNS, "error_max_turns: too many turns"),
    ] {
        let rig = Rig::new(&[("on_user", lines(&[INIT_OK, result]))]);
        let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
        let (events, error) = drain(&mut turn).await;
        assert_eq!(events, Vec::new());
        assert_eq!(error, Some(AgentError::Agent(expected.to_owned())));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_error_result_without_a_message_still_names_its_subtype() {
    let bare = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true}"#.to_owned() + "\n";
    let rig = Rig::new(&[("on_user", lines(&[INIT_OK, &bare]))]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (_, error) = drain(&mut turn).await;
    assert_eq!(error, Some(AgentError::Agent("error_max_budget_usd: ".to_owned())));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_line_over_the_cap_is_skipped_whole_and_the_ones_after_it_still_arrive() {
    let huge = delta(&"x".repeat(9 * 1024 * 1024));
    let rig = Rig::new(&[
        (
            "on_user",
            lines(&[INIT_OK, &delta("antes"), "~CAT huge\n", &delta("después"), SUCCESS]),
        ),
        ("huge", huge),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events), ["antes", "después"]);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_line_just_under_the_cap_is_read_and_one_at_it_is_skipped() {
    let under = delta(&"y".repeat(7 * 1024 * 1024));
    let rig = Rig::new(&[
        ("on_user", lines(&[INIT_OK, "~CAT under\n", &delta("fin"), SUCCESS])),
        ("under", under),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    let said = deltas(&events);
    assert_eq!(said.len(), 2);
    assert_eq!(said[0].len(), 7 * 1024 * 1024);
    assert_eq!(said[1], "fin");
    let cap = 8 * 1024 * 1024;
    let probe = |len: usize| {
        let overhead = delta("").len();
        delta(&"z".repeat(len - overhead))
    };
    let exact = probe(cap);
    assert_eq!(exact.len(), cap);
    let rig = Rig::new(&[
        ("on_user", lines(&[INIT_OK, "~CAT exact\n", &delta("fin"), SUCCESS])),
        ("exact", exact),
    ]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let (events, error) = drain(&mut turn).await;
    assert_eq!(error, None);
    assert_eq!(deltas(&events).last().map(String::as_str), Some("fin"));
    assert!(deltas(&events).len() <= 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_child_closing_its_output_mid_turn_is_gone_and_the_next_turn_resumes() {
    let rig = Rig::new(&[
        ("on_user.1", lines(&[INIT_OK, &delta("a medias"), "~EXIT\n"])),
        ("on_user.2", lines(&[INIT_OK, DELTAS, SUCCESS])),
    ]);
    let mut first = rig.agent.turn("uno".to_owned()).await.unwrap();
    let (events, error) = drain(&mut first).await;
    assert_eq!(deltas(&events), ["a medias"]);
    assert_eq!(error, Some(AgentError::Gone));
    assert!(first.next().await.is_none());
    drop(first);
    let mut second = rig.agent.turn("dos".to_owned()).await.unwrap();
    let (events, error) = drain(&mut second).await;
    assert_eq!(error, None);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
    assert_eq!(rig.spawns(), 2);
    assert!(rig.argv(2).iter().any(|arg| arg.starts_with("--resume=")));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_next_dropped_mid_line_in_a_select_loses_nothing() {
    let rig = Rig::new(&[(
        "on_user",
        lines(&[
            INIT_OK,
            "~PART {\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"entero\"}},\n",
            "~SLEEP 0.4\n",
            "~PART \"parent_tool_use_id\":null}\n",
            "\n",
            "~SLEEP 0.2\n",
            &delta("segundo"),
            SUCCESS,
        ]),
    )]);
    let mut turn = rig.agent.turn("hola".to_owned()).await.unwrap();
    let mut events = Vec::new();
    let mut dropped = 0;
    loop {
        tokio::select! {
            next = turn.next() => match next {
                Some(Ok(event)) => events.push(event),
                Some(Err(error)) => panic!("{error:?}"),
                None => break,
            },
            () = tokio::time::sleep(Duration::from_millis(50)) => dropped += 1,
        }
    }
    assert!(
        dropped >= 3,
        "the select must have dropped next() mid-line, {dropped} times"
    );
    assert_eq!(deltas(&events), ["entero", "segundo"]);
    assert_eq!(events.last(), Some(&AgentEvent::Done(Ended::Answered)));
}
