use std::path::PathBuf;
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::talk::approval::{Approvals, Pending, QUEUED, Verdict};
use serde_json::{Value, json};

const TOKEN: &str = "tok-0123456789abcdef";

fn bash(command: &str) -> Value {
    json!({"command": command})
}

fn cwd() -> PathBuf {
    PathBuf::from("/nonexistent-project")
}

fn deny(message: &str) -> Verdict {
    Verdict::Deny(message.to_owned())
}

async fn next(inbox: &mut e_voice_daemon::talk::approval::ApprovalInbox) -> Pending {
    crate::fake::question(inbox).await
}

/// No question reaches the inbox; a request refused unasked is reported to it as a `Refused` item, and
/// that is not a question.
async fn nothing(inbox: &mut e_voice_daemon::talk::approval::ApprovalInbox) {
    let seen = crate::fake::asked(inbox).await;
    assert!(
        seen.is_none(),
        "the inbox saw a question: {:?}",
        seen.map(|p| p.question)
    );
}

#[test]
fn test_the_behavior_texts_are_exactly_claude_codes_two_shapes() {
    assert_eq!(Verdict::Allow.behavior(), r#"{"behavior":"allow"}"#);
    assert_eq!(deny("x").behavior(), r#"{"behavior":"deny","message":"x"}"#);
    assert_eq!(
        deny("a \"q\" \n ñ").behavior(),
        r#"{"behavior":"deny","message":"a \"q\" \n ñ"}"#
    );
    let parsed: Value = serde_json::from_str(&deny("m").behavior()).unwrap();
    assert_eq!(parsed.as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn test_with_no_desk_a_request_is_denied_at_once_whoever_asks() {
    let approvals = Approvals::default();
    let started = std::time::Instant::now();
    for token in [None, Some(""), Some(TOKEN), Some("other")] {
        assert_eq!(
            approvals.ask(token, "Bash", &bash("ls")).await,
            deny("no voice conversation is active"),
            "{token:?}"
        );
    }
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[tokio::test]
async fn test_a_wrong_missing_empty_or_look_alike_secret_is_denied_and_the_inbox_never_sees_it() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let same_length_wrong = "tok-0123456789abcdeX";
    assert_eq!(same_length_wrong.len(), TOKEN.len());
    let wrong: [Option<&str>; 9] = [
        None,
        Some(""),
        Some("other"),
        Some(same_length_wrong),
        Some("${EVOICE_APPROVER}"),
        Some("tok-0123456789abcde"),
        Some("tok-0123456789abcdef0"),
        Some(" tok-0123456789abcdef"),
        Some("TOK-0123456789ABCDEF"),
    ];
    for token in wrong {
        assert_eq!(
            approvals.ask(token, "Bash", &bash("ls")).await,
            deny("not the voice conversation's own agent"),
            "{token:?}"
        );
    }
    nothing(&mut inbox).await;
}

#[tokio::test]
async fn test_a_wrong_secret_does_not_disturb_the_right_ones_request() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let asking = approvals.clone();
    let right = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls")).await });
    let pending = next(&mut inbox).await;
    assert_eq!(
        approvals.ask(Some("intruso"), "Bash", &bash("rm")).await,
        deny("not the voice conversation's own agent")
    );
    pending.answer(Verdict::Allow);
    assert_eq!(right.await.unwrap(), Verdict::Allow);
}

#[tokio::test]
async fn test_the_right_secret_gets_the_exact_question_and_the_verdict_comes_back() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let asking = approvals.clone();
    let allowed = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls -la | head")).await });
    let pending = next(&mut inbox).await;
    assert_eq!(pending.question, "¿Ejecuto: ls, guion la, barra vertical, head?");
    pending.answer(Verdict::Allow);
    assert_eq!(allowed.await.unwrap(), Verdict::Allow);
    let asking = approvals.clone();
    let denied = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls")).await });
    next(&mut inbox).await.answer(deny("x"));
    assert_eq!(denied.await.unwrap(), deny("x"));
}

#[tokio::test]
async fn test_the_question_is_asked_in_the_conversations_language() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::En, cwd()).unwrap();
    let asking = approvals.clone();
    let task = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls | wc")).await });
    let pending = next(&mut inbox).await;
    assert_eq!(pending.question, "Shall I run: ls, pipe, wc?");
    pending.answer(Verdict::Allow);
    task.await.unwrap();
}

#[tokio::test]
async fn test_refusals_without_asking_are_denied_with_their_reason_and_never_reach_the_inbox() {
    use e_voice_daemon::talk::question::{EMPTY, LONG, OUTSIDE, SPOKEN_LONG, UNREAD};
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let cases: Vec<(&str, Value, &str)> = vec![
        ("Bash", bash(&"a".repeat(161)), LONG),
        ("Bash", bash("ls\nrm -rf /"), LONG),
        ("Bash", bash("echo …"), UNREAD),
        ("Bash", bash("echo  two"), UNREAD),
        ("Bash", bash(&"|".repeat(160)), SPOKEN_LONG),
        ("Bash", json!({}), EMPTY),
        ("Edit", json!({"file_path": "/etc/passwd"}), OUTSIDE),
        ("Write", json!({"file_path": "../x"}), OUTSIDE),
        ("Edit", json!({}), OUTSIDE),
    ];
    for (tool, input, reason) in cases {
        assert_eq!(
            approvals.ask(Some(TOKEN), tool, &input).await,
            deny(reason),
            "{tool} {input}"
        );
    }
    nothing(&mut inbox).await;
}

#[tokio::test(start_paused = true)]
async fn test_an_unanswered_question_is_denied_at_sixty_seconds_never_before() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let asking = approvals.clone();
    let task = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls")).await });
    let held = next(&mut inbox).await;
    tokio::time::sleep(Duration::from_secs(59)).await;
    assert!(!task.is_finished(), "denied before the bound");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(task.await.unwrap(), deny("no answer in time"));
    drop(held);
}

#[tokio::test]
async fn test_a_dropped_pending_is_a_denial_and_so_is_a_dropped_inbox_mid_wait() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let asking = approvals.clone();
    let task = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls")).await });
    drop(next(&mut inbox).await);
    assert!(matches!(task.await.unwrap(), Verdict::Deny(message) if !message.is_empty()));
    let asking = approvals.clone();
    let waiting = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("pwd")).await });
    let held = next(&mut inbox).await;
    drop(inbox);
    drop(held);
    assert_eq!(waiting.await.unwrap(), deny("the voice conversation ended"));
    assert_eq!(
        approvals.ask(Some(TOKEN), "Bash", &bash("ls")).await,
        deny("no voice conversation is active")
    );
}

#[tokio::test]
async fn test_a_question_sent_into_a_closing_inbox_is_denied_ended() {
    let approvals = Approvals::default();
    let inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let asking = approvals.clone();
    let task = tokio::spawn(async move { asking.ask(Some(TOKEN), "Bash", &bash("ls")).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(inbox);
    assert_eq!(task.await.unwrap(), deny("the voice conversation ended"));
}

#[tokio::test]
async fn test_two_questions_never_overlap_the_second_waits_for_the_first() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let (one, two) = (approvals.clone(), approvals.clone());
    let first = tokio::spawn(async move { one.ask(Some(TOKEN), "Bash", &bash("uno")).await });
    let held = next(&mut inbox).await;
    assert_eq!(held.question, "¿Ejecuto: uno?");
    let second = tokio::spawn(async move { two.ask(Some(TOKEN), "Bash", &bash("dos")).await });
    nothing(&mut inbox).await;
    held.answer(Verdict::Allow);
    assert_eq!(first.await.unwrap(), Verdict::Allow);
    let next_up = next(&mut inbox).await;
    assert_eq!(
        next_up.question, "¿Ejecuto: dos?",
        "an allowed first leaves the second to be asked"
    );
    next_up.answer(Verdict::Allow);
    assert_eq!(second.await.unwrap(), Verdict::Allow);
}

#[tokio::test]
async fn test_a_request_that_waited_behind_a_refused_one_is_denied_unasked_and_a_later_one_is_asked() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let (one, two) = (approvals.clone(), approvals.clone());
    let first = tokio::spawn(async move { one.ask(Some(TOKEN), "Bash", &bash("uno")).await });
    let held = next(&mut inbox).await;
    let second = tokio::spawn(async move { two.ask(Some(TOKEN), "Bash", &bash("dos")).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    held.answer(deny("no"));
    assert_eq!(first.await.unwrap(), deny("no"));
    assert_eq!(second.await.unwrap(), deny(QUEUED));
    nothing(&mut inbox).await;
    let later = approvals.clone();
    let third = tokio::spawn(async move { later.ask(Some(TOKEN), "Bash", &bash("tres")).await });
    let asked = next(&mut inbox).await;
    assert_eq!(asked.question, "¿Ejecuto: tres?");
    asked.answer(Verdict::Allow);
    assert_eq!(third.await.unwrap(), Verdict::Allow);
}

#[tokio::test]
async fn test_a_timeout_or_a_dropped_pending_also_refuses_those_waiting() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let (one, two) = (approvals.clone(), approvals.clone());
    let first = tokio::spawn(async move { one.ask(Some(TOKEN), "Bash", &bash("uno")).await });
    let held = next(&mut inbox).await;
    let second = tokio::spawn(async move { two.ask(Some(TOKEN), "Bash", &bash("dos")).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    drop(held);
    assert!(matches!(first.await.unwrap(), Verdict::Deny(_)));
    assert_eq!(second.await.unwrap(), deny(QUEUED));
}

#[tokio::test]
async fn test_many_concurrent_requests_are_asked_one_at_a_time_in_order_with_none_lost() {
    let approvals = Approvals::default();
    let mut inbox = approvals.open(TOKEN.to_owned(), Lang::Es, cwd()).unwrap();
    let mut tasks = Vec::new();
    for n in 0..8 {
        let asking = approvals.clone();
        tasks.push(tokio::spawn(async move {
            asking.ask(Some(TOKEN), "Bash", &bash(&format!("c{n}"))).await
        }));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut order = Vec::new();
    for _ in 0..8 {
        let pending = next(&mut inbox).await;
        order.push(pending.question.clone());
        nothing_else_queued(&mut inbox).await;
        pending.answer(Verdict::Allow);
    }
    let mut sorted = order.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 8, "{order:?}");
    for task in tasks {
        assert_eq!(task.await.unwrap(), Verdict::Allow);
    }
}

async fn nothing_else_queued(inbox: &mut e_voice_daemon::talk::approval::ApprovalInbox) {
    let seen = crate::fake::asked_within(inbox, Duration::from_millis(30)).await;
    assert!(seen.is_none(), "two questions were open at once");
}

#[tokio::test]
async fn test_the_desk_is_held_by_one_conversation_and_freed_when_its_inbox_drops() {
    let approvals = Approvals::default();
    let held = approvals.open(TOKEN.to_owned(), Lang::Es, cwd());
    assert!(held.is_some());
    assert!(approvals.open("otro".to_owned(), Lang::En, cwd()).is_none());
    assert!(
        approvals.clone().open(TOKEN.to_owned(), Lang::Es, cwd()).is_none(),
        "clones share one desk"
    );
    drop(held);
    let again = approvals.open("otro".to_owned(), Lang::En, cwd());
    assert!(again.is_some());
    assert_eq!(
        approvals.ask(Some(TOKEN), "Bash", &bash("ls")).await,
        deny("not the voice conversation's own agent"),
        "the old secret no longer opens the desk"
    );
}
