use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use serde_json::{Value, json};

use crate::fake::{FakeSpeech, Step, TalkState::*, Verdict, delta};
use crate::harness::Rig;

fn ask(command: &str) -> Step {
    Step::Ask("Bash".to_owned(), json!({"command": command}))
}

fn bash(command: &str) -> Value {
    json!({"command": command})
}

fn turn_with_one_question() -> Vec<Step> {
    vec![
        delta("Primero. "),
        ask("ls"),
        Step::Verdicts(1),
        delta("Listo. "),
        Step::Done,
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_yes_allows_the_question_is_queued_after_the_earlier_sentences_and_the_states_follow() {
    let mut rig = Rig::marked(3);
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "haz algo").await;
    rig.until(Asking).await;
    rig.says(2, "sí").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Allow]);
    assert_eq!(rig.spoken(), ["Primero.", "¿Ejecuto: ls?", "Listo."]);
    assert_eq!(
        rig.states,
        [Listening, Thinking, Speaking, Asking, Thinking, Speaking, Listening]
    );
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_no_denies_with_the_users_own_words_and_the_conversation_goes_on() {
    let mut rig = Rig::marked(3);
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "haz algo").await;
    rig.until(Asking).await;
    rig.says(2, "No.").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Deny("the user said no".to_owned())]);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_unclear_answer_is_asked_once_more_and_the_second_answer_decides() {
    let mut rig = Rig::marked(4);
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "haz algo").await;
    rig.until(Asking).await;
    rig.says(2, "quizá").await;
    rig.says(3, "vale").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Allow]);
    assert_eq!(
        rig.spoken(),
        ["Primero.", "¿Ejecuto: ls?", "¿Sí o no? ¿Ejecuto: ls?", "Listo."]
    );
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_two_unclear_answers_deny_with_no_clear_answer() {
    let mut rig = Rig::marked(4);
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "haz algo").await;
    rig.until(Asking).await;
    rig.says(2, "mmm").await;
    rig.says(3, "quizás").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Deny("no clear answer".to_owned())]);
    assert_eq!(
        rig.spoken().iter().filter(|text| text.starts_with("¿Sí o no?")).count(),
        1,
        "asked once more, not twice"
    );
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_silence_within_the_answer_bound_denies_with_no_answer_heard() {
    let mut rig = Rig::bounded(
        FakeSpeech::marked(),
        Lang::Es,
        3,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_millis(400),
    );
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "haz algo").await;
    rig.until(Asking).await;
    let started = std::time::Instant::now();
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Deny("no answer heard".to_owned())]);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "answered after the desk's own bound: {:?}",
        started.elapsed()
    );
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_answer_in_english_is_read_in_the_conversations_language() {
    let mut rig = Rig::new(FakeSpeech::marked(), Lang::En, 3);
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "do something").await;
    rig.until(Asking).await;
    rig.says(2, "go ahead").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Allow]);
    assert_eq!(rig.spoken()[1], "Shall I run: ls?");
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_no_in_the_middle_of_a_sentence_is_a_no_the_safe_side() {
    let mut rig = Rig::marked(3);
    rig.w.script.turn(turn_with_one_question());
    rig.talk.start().await.unwrap();
    rig.says(1, "haz algo").await;
    rig.until(Asking).await;
    rig.says(2, "sí, no hay problema").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Deny("the user said no".to_owned())]);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_two_requests_are_asked_one_at_a_time_in_order() {
    let mut rig = Rig::marked(4);
    rig.w
        .script
        .turn(vec![ask("uno"), ask("dos"), Step::Verdicts(2), Step::Done]);
    rig.talk.start().await.unwrap();
    rig.says(1, "haz").await;
    rig.until(Asking).await;
    rig.says(2, "sí").await;
    rig.says(3, "sí").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.verdicts(), [Verdict::Allow, Verdict::Allow]);
    let said = rig.spoken();
    let first = said.iter().position(|text| text == "¿Ejecuto: uno?").unwrap();
    let second = said.iter().position(|text| text == "¿Ejecuto: dos?").unwrap();
    assert!(first < second, "{said:?}");
    rig.talk.end().await;
}

fn too_long() -> Step {
    ask(&"a".repeat(200))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_unasked_refusals_are_silent_twice_and_the_third_in_a_row_is_said() {
    let mut rig = Rig::marked(3);
    rig.w.script.turn(vec![
        too_long(),
        too_long(),
        Step::Wait(Duration::from_millis(150)),
        Step::Done,
    ]);
    rig.talk.start().await.unwrap();
    rig.says(1, "haz").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(
        rig.spoken(),
        Vec::<String>::new(),
        "two refusals are for the agent to rephrase, not to say"
    );
    rig.w.script.turn(vec![
        too_long(),
        too_long(),
        too_long(),
        Step::Wait(Duration::from_millis(150)),
        Step::Done,
    ]);
    rig.says(2, "otra vez").await;
    rig.until_count(Listening, 3).await;
    assert_eq!(rig.spoken(), ["Le he pedido que lo simplifique; no podía leértelo."]);
    assert_eq!(rig.w.script.verdicts().len(), 5);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_refusal_count_resets_on_an_asked_question_and_at_each_turns_start() {
    let mut rig = Rig::marked(4);
    rig.w.script.turn(vec![
        too_long(),
        too_long(),
        ask("ls"),
        Step::Verdicts(3),
        too_long(),
        too_long(),
        Step::Wait(Duration::from_millis(150)),
        Step::Done,
    ]);
    rig.talk.start().await.unwrap();
    rig.says(1, "haz").await;
    rig.until(Asking).await;
    rig.says(2, "sí").await;
    rig.until_count(Listening, 2).await;
    assert!(
        !rig.spoken().iter().any(|text| text.contains("simplifique")),
        "a question in between resets the count: {:?}",
        rig.spoken()
    );
    rig.w.script.turn(vec![
        too_long(),
        too_long(),
        Step::Wait(Duration::from_millis(150)),
        Step::Done,
    ]);
    rig.says(3, "otra").await;
    rig.until_count(Listening, 3).await;
    assert!(
        !rig.spoken().iter().any(|text| text.contains("simplifique")),
        "a new turn starts the count again: {:?}",
        rig.spoken()
    );
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_halt_while_asking_denies_stopped_cancels_the_turn_and_listens_again() {
    let mut rig = Rig::new(crate::fake::long(30, 1.0), Lang::Es, 3);
    rig.w.script.turn(vec![ask("ls"), Step::Verdicts(1), Step::Block]);
    rig.talk.start().await.unwrap();
    rig.says(1, "haz").await;
    rig.until(Asking).await;
    rig.talk.stop().await.unwrap();
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.answered(1).await, [Verdict::Deny("stopped".to_owned())]);
    assert_eq!(rig.w.script.cancels(), 1);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_request_arriving_between_turns_with_the_wrong_secret_never_opens_a_question() {
    let mut rig = Rig::marked(1);
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    let verdict = rig.w.approvals.ask(Some("no-es"), "Bash", &bash("rm -rf x")).await;
    assert_eq!(
        verdict,
        Verdict::Deny("not the voice conversation's own agent".to_owned())
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        rig.drain().to_vec(),
        [Listening],
        "a stranger's request moved the conversation"
    );
    assert_eq!(rig.spoken(), Vec::<String>::new());
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_halt_nobody_polls_still_halts() {
    let mut rig = Rig::new(crate::fake::long(30, 1.0), Lang::Es, 3);
    rig.w.script.turn(vec![ask("ls"), Step::Verdicts(1), Step::Block]);
    rig.talk.start().await.unwrap();
    rig.says(1, "haz").await;
    rig.until(Asking).await;
    drop(rig.talk.halt());
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.answered(1).await, [Verdict::Deny("stopped".to_owned())]);
    assert_eq!(rig.w.script.cancels(), 1);
    rig.talk.end().await;
}
