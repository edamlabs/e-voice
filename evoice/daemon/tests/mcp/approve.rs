use std::path::PathBuf;
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::talk::approval::{ApprovalInbox, Verdict};
use serde_json::{Value, json};

use crate::client::Mcp;
use crate::fake::{Device, FakeSpeech, Wired, wired};
use crate::running::Running;

const TOKEN: &str = "tok-0123456789abcdef";
const HEADER: &str = "x-evoice-approver";

fn rig() -> Wired {
    wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_secs(5))
}

async fn up(w: &Wired) -> (Running, Mcp) {
    let daemon = Running::start(w.state.clone()).await;
    let mcp = Mcp::on(daemon.port);
    (daemon, mcp)
}

fn desk(w: &Wired, lang: Lang) -> ApprovalInbox {
    w.approvals
        .open(TOKEN.to_owned(), lang, PathBuf::from("/nonexistent-project"))
        .unwrap()
}

fn approve(command: &str) -> Value {
    json!({"tool_name": "Bash", "input": {"command": command}})
}

fn behavior(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    let blocks = result["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "exactly one content block: {result}");
    assert_eq!(blocks[0]["type"], "text");
    assert!(
        result.get("structuredContent").is_none(),
        "structuredContent must be absent: {result}"
    );
    let mut keys: Vec<&str> = result.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["content", "isError"]);
    serde_json::from_str(blocks[0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_no_desk_approve_denies_at_once_as_one_text_block() {
    let w = rig();
    let (daemon, mcp) = up(&w).await;
    let started = std::time::Instant::now();
    let result = mcp
        .call_with("approve", &approve("ls"), &[(HEADER, TOKEN)])
        .await
        .result();
    daemon.stop().await;
    assert_eq!(
        result["content"][0]["text"],
        r#"{"behavior":"deny","message":"no voice conversation is active"}"#
    );
    assert_eq!(
        behavior(&result),
        json!({"behavior": "deny", "message": "no voice conversation is active"})
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_wrong_missing_empty_or_literal_header_is_denied_and_nothing_is_asked() {
    let w = rig();
    let mut inbox = desk(&w, Lang::Es);
    let (daemon, mcp) = up(&w).await;
    let headers: [&[(&str, &str)]; 6] = [
        &[],
        &[(HEADER, "")],
        &[(HEADER, "${EVOICE_APPROVER}")],
        &[(HEADER, "tok-0123456789abcdeX")],
        &[(HEADER, "wrong")],
        &[("x-evoice-client", TOKEN)],
    ];
    for extra in headers {
        let result = mcp.call_with("approve", &approve("ls"), extra).await.result();
        assert_eq!(
            behavior(&result),
            json!({"behavior": "deny", "message": "not the voice conversation's own agent"}),
            "{extra:?}"
        );
    }
    let seen = crate::fake::asked(&mut inbox).await;
    daemon.stop().await;
    assert!(
        seen.is_none(),
        "the inbox saw a question from a request that is not the agent's own"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_right_header_gets_asked_and_the_answer_is_exactly_claude_codes_text_block() {
    let w = rig();
    let mut inbox = desk(&w, Lang::Es);
    let (daemon, mcp) = up(&w).await;
    let asking = mcp.clone();
    let call = tokio::spawn(async move {
        asking
            .call_with("approve", &approve("ls -la | head"), &[(HEADER, TOKEN)])
            .await
            .result()
    });
    let pending = crate::fake::question(&mut inbox).await;
    assert_eq!(pending.question, "¿Ejecuto: ls, guion la, barra vertical, head?");
    pending.answer(Verdict::Allow);
    let result = call.await.unwrap();
    assert_eq!(
        result,
        json!({"content": [{"type": "text", "text": "{\"behavior\":\"allow\"}"}], "isError": false})
    );
    let asking = mcp.clone();
    let call = tokio::spawn(async move {
        asking
            .call_with("approve", &approve("rm x"), &[(HEADER, TOKEN)])
            .await
            .result()
    });
    crate::fake::question(&mut inbox)
        .await
        .answer(Verdict::Deny("x".to_owned()));
    let result = call.await.unwrap();
    daemon.stop().await;
    assert_eq!(result["content"][0]["text"], r#"{"behavior":"deny","message":"x"}"#);
    assert_eq!(behavior(&result), json!({"behavior": "deny", "message": "x"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_request_the_listener_cannot_hear_whole_is_denied_with_its_reason_over_mcp() {
    let w = rig();
    let mut inbox = desk(&w, Lang::En);
    let (daemon, mcp) = up(&w).await;
    let result = mcp
        .call_with("approve", &approve(&"a".repeat(200)), &[(HEADER, TOKEN)])
        .await
        .result();
    let seen = crate::fake::asked(&mut inbox).await;
    daemon.stop().await;
    assert_eq!(behavior(&result)["behavior"], "deny");
    assert!(behavior(&result)["message"].as_str().unwrap().contains("too long"));
    assert!(seen.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_bad_approve_arguments_are_a_tool_error_and_nothing_is_asked() {
    let w = rig();
    let mut inbox = desk(&w, Lang::Es);
    let (daemon, mcp) = up(&w).await;
    for arguments in [
        json!({}),
        json!({"tool_name": "Bash"}),
        json!({"input": {}}),
        json!({"tool_name": 7, "input": {}}),
    ] {
        let result = mcp.call_with("approve", &arguments, &[(HEADER, TOKEN)]).await.result();
        assert_eq!(result["isError"], true, "{arguments}: {result}");
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("failed to deserialize parameters")
        );
    }
    let seen = crate::fake::asked(&mut inbox).await;
    daemon.stop().await;
    assert!(seen.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_that_goes_away_mid_question_does_not_leave_the_desk_stuck() {
    let w = rig();
    let mut inbox = desk(&w, Lang::Es);
    let (daemon, mcp) = up(&w).await;
    let asking = mcp.clone();
    let call = tokio::spawn(async move { asking.call_with("approve", &approve("ls"), &[(HEADER, TOKEN)]).await });
    let held = crate::fake::question(&mut inbox).await;
    call.abort();
    held.answer(Verdict::Allow);
    let again = mcp.clone();
    let next = tokio::spawn(async move {
        again
            .call_with("approve", &approve("pwd"), &[(HEADER, TOKEN)])
            .await
            .result()
    });
    let pending = crate::fake::question(&mut inbox).await;
    assert_eq!(pending.question, "¿Ejecuto: pwd?");
    pending.answer(Verdict::Allow);
    assert_eq!(next.await.unwrap()["content"][0]["text"], r#"{"behavior":"allow"}"#);
    daemon.stop().await;
}
