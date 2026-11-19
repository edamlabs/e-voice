use std::time::{Duration, Instant};

use serde_json::json;

use crate::fake::{FakeSpeech, Verdict};
use crate::fake::{Step, TalkState::*, delta, long};
use crate::harness::Rig;
use e_voice_core::schema::lang::Lang;

#[tokio::test(flavor = "multi_thread")]
async fn test_ending_while_listening_leaves_every_resource_free() {
    let mut rig = Rig::new(FakeSpeech::marked(), Lang::Es, 1);
    rig.w.mic.opened();
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    let started = Instant::now();
    rig.talk.end().await;
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    rig.until(Idle).await;
    assert_eq!(rig.states, [Listening, Idle]);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ending_while_thinking_cancels_the_turn_closes_the_agent_and_frees_everything() {
    let mut rig = Rig::marked(1);
    rig.w.script.turn(vec![Step::Block]);
    rig.talk.start().await.unwrap();
    rig.says(1, "piensa").await;
    rig.until(Thinking).await;
    rig.talk.end().await;
    rig.until(Idle).await;
    assert_eq!(rig.w.script.cancels(), 1);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ending_while_speaking_cuts_the_speech_cancels_and_frees_everything() {
    let mut rig = Rig::new(long(40, 1.0), Lang::Es, 1);
    rig.w.script.turn(vec![delta("Una frase muy larga. "), Step::Block]);
    rig.talk.start().await.unwrap();
    rig.says(1, "habla").await;
    rig.until(Speaking).await;
    rig.w.state.speech.stop().await.unwrap();
    rig.talk.end().await;
    rig.until(Idle).await;
    assert_eq!(rig.w.script.cancels(), 1);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ending_while_asking_answers_the_question_denied_and_frees_everything() {
    let mut rig = Rig::marked(2);
    rig.w.script.turn(vec![
        Step::Ask("Bash".to_owned(), json!({"command": "ls"})),
        Step::Verdicts(1),
        Step::Block,
    ]);
    rig.talk.start().await.unwrap();
    rig.says(1, "haz").await;
    rig.until(Asking).await;
    rig.hearing(2).await;
    rig.talk.end().await;
    rig.until(Idle).await;
    assert_eq!(
        rig.answered(1).await,
        [Verdict::Deny("the voice conversation ended".to_owned())]
    );
    assert_eq!(rig.w.script.cancels(), 1);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_every_exit_says_its_last_notice_before_the_conversation_is_idle() {
    let mut rig = Rig::marked(1);
    rig.talk.start().await.unwrap();
    rig.says(1, "adiós").await;
    rig.until(Idle).await;
    assert_eq!(rig.spoken(), ["Hasta luego."]);
    assert_eq!(
        rig.w.state.speech.state(),
        e_voice_daemon::schema::event::SpeechState::Idle
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_conversation_can_start_again_after_each_way_it_ends_and_each_gets_a_fresh_agent_and_secret() {
    let mut rig = Rig::marked(3);
    rig.talk.start().await.unwrap();
    rig.says(1, "adiós").await;
    rig.until(Idle).await;
    rig.states.clear();
    rig.talk.start().await.unwrap();
    rig.hearing(2).await;
    rig.talk.end().await;
    rig.until(Idle).await;
    rig.states.clear();
    rig.talk.start().await.unwrap();
    rig.hearing(3).await;
    rig.talk.end().await;
    rig.until(Idle).await;
    let tokens = rig.w.script.tokens();
    assert_eq!(tokens.len(), 3);
    let distinct: std::collections::BTreeSet<&String> = tokens.iter().collect();
    assert_eq!(distinct.len(), 3, "a secret was reused: {tokens:?}");
    for token in &tokens {
        assert_eq!(token.len(), 32, "{token}");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()), "{token}");
    }
    assert_eq!(rig.w.script.closes(), 3);
    rig.assert_free(3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_desk_belongs_to_the_conversation_while_it_runs_and_its_secret_is_the_agents_own() {
    let rig = Rig::marked(1);
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    let token = rig.w.script.tokens().remove(0);
    assert!(
        rig.w
            .approvals
            .open("other".to_owned(), Lang::Es, std::path::PathBuf::from("/w"))
            .is_none(),
        "the desk is the conversation's"
    );
    let wrong = rig
        .w
        .approvals
        .ask(Some("intruso"), "Bash", &json!({"command": "ls"}))
        .await;
    assert_eq!(
        wrong,
        Verdict::Deny("not the voice conversation's own agent".to_owned())
    );
    assert_eq!(token.len(), 32);
    rig.talk.end().await;
    let after = rig
        .w
        .approvals
        .ask(Some(&token), "Bash", &json!({"command": "ls"}))
        .await;
    assert_eq!(
        after,
        Verdict::Deny("no voice conversation is active".to_owned()),
        "the old secret opens nothing once it ended"
    );
}

const RESTARTS: usize = 50;

#[tokio::test(flavor = "multi_thread")]
async fn test_a_restart_right_after_idle_always_succeeds_after_a_goodbye() {
    let mut rig = Rig::marked(RESTARTS);
    for n in 1..=RESTARTS {
        rig.talk
            .start()
            .await
            .unwrap_or_else(|error| panic!("start {n}: {error}"));
        rig.says(n, "adiós").await;
        rig.until(Idle).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_restart_right_after_idle_always_succeeds_after_a_door_ends_it() {
    let mut rig = Rig::marked(RESTARTS);
    for n in 1..=RESTARTS {
        rig.talk
            .start()
            .await
            .unwrap_or_else(|error| panic!("start {n}: {error}"));
        rig.hearing(n).await;
        rig.talk.end().await;
        rig.until(Idle).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_restart_right_after_idle_always_succeeds_after_silence() {
    let mut rig = Rig::bounded(
        FakeSpeech::marked(),
        Lang::Es,
        RESTARTS,
        Duration::from_millis(30),
        Duration::from_secs(3),
        Duration::from_millis(600),
    );
    for n in 1..=RESTARTS {
        rig.talk
            .start()
            .await
            .unwrap_or_else(|error| panic!("start {n}: {error}"));
        rig.until(Idle).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_restart_right_after_idle_always_succeeds_after_a_refused_agent() {
    let mut rig = Rig::marked(RESTARTS);
    for n in 1..=RESTARTS {
        rig.w
            .script
            .turn(vec![Step::Err(e_voice_daemon::talk::agent::base::AgentError::Refused(
                "x".to_owned(),
            ))]);
        rig.talk
            .start()
            .await
            .unwrap_or_else(|error| panic!("start {n}: {error}"));
        rig.says(n, "uno").await;
        rig.until(Idle).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_restart_right_after_idle_always_succeeds_after_a_failed_listen() {
    let mut rig = Rig::marked(RESTARTS);
    for n in 1..=RESTARTS {
        rig.talk
            .start()
            .await
            .unwrap_or_else(|error| panic!("start {n}: {error}"));
        rig.hearing(n).await;
        rig.feeds[n - 1].fail(e_voice_daemon::hearing::base::HearingError::Server("1011".to_owned()));
        rig.until(Idle).await;
    }
}
