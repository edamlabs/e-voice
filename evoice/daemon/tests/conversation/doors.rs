use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::api::state::AppState;
use e_voice_daemon::schema::event::SpeechState;
use serde_json::{Value, json};

use crate::fake::{FakeSpeech, Step, TalkState::*, Verdict, delta, long};
use crate::harness::Rig;
use crate::running::Running;

async fn door(rig: &Rig) -> Running {
    let mcp = e_voice_daemon::mcp::server::McpServer::new(
        rig.w.state.speech.clone(),
        std::sync::Arc::clone(&rig.w.state.listen),
        std::sync::Arc::new(rig.w.voices.clone()),
        e_voice_daemon::speech::table::VoiceTable::new(std::collections::BTreeMap::new()),
        rig.w.approvals.clone(),
        std::sync::Arc::clone(&rig.talk),
    );
    let state = AppState {
        talk: std::sync::Arc::clone(&rig.talk),
        mcp,
        ..rig.w.state.clone()
    };
    Running::start(state).await
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_post_talk_starts_listening_and_a_second_post_is_a_409_with_the_error() {
    let mut rig = Rig::marked(1);
    let daemon = door(&rig).await;
    let (status, answer) = daemon.send("POST", "/talk").await;
    assert_eq!((status, answer.as_str()), (200, r#"{"state":"listening"}"#));
    rig.until(Listening).await;
    let (status, answer) = daemon.send("POST", "/talk").await;
    assert_eq!(status, 409);
    assert_eq!(
        body(&answer),
        json!({"error": "a voice conversation is already running"})
    );
    let (status, answer) = daemon.send("DELETE", "/talk").await;
    assert_eq!((status, answer.as_str()), (200, r#"{"state":"idle"}"#));
    rig.until(Idle).await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_post_talk_while_a_listen_holds_the_microphone_is_a_409() {
    let rig = Rig::marked(2);
    let daemon = door(&rig).await;
    let feed = &rig.feeds[0];
    let listening = rig.w.state.listen.clone();
    let holding = tokio::spawn(async move { listening.listen(Some(Duration::from_secs(5))).await });
    rig.hearing(1).await;
    let (status, answer) = daemon.send("POST", "/talk").await;
    assert_eq!(status, 409);
    assert_eq!(body(&answer), json!({"error": "the microphone is held by a listen"}));
    feed.say(e_voice_daemon::hearing::base::Recognized::Started);
    feed.say(e_voice_daemon::hearing::base::Recognized::Final(
        crate::fake::heard("hola"),
        None,
    ));
    holding.await.unwrap().unwrap();
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_talk_with_no_conversation_is_a_200_idle_and_so_is_every_repeat() {
    let rig = Rig::marked(0);
    let daemon = door(&rig).await;
    for _ in 0..3 {
        let (status, answer) = daemon.send("DELETE", "/talk").await;
        assert_eq!((status, answer.as_str()), (200, r#"{"state":"idle"}"#));
    }
    assert_eq!(rig.w.script.closes(), 0);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_talk_mid_turn_cancels_closes_and_a_new_start_gets_a_fresh_agent_and_secret() {
    let mut rig = Rig::marked(2);
    rig.w.script.turn(vec![Step::Block]);
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.says(1, "piensa").await;
    rig.until(Thinking).await;
    let started = Instant::now();
    let (status, _) = daemon.send("DELETE", "/talk").await;
    assert_eq!(status, 200);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    rig.until(Idle).await;
    assert_eq!((rig.w.script.cancels(), rig.w.script.closes()), (1, 1));
    let (status, answer) = daemon.send("POST", "/talk").await;
    assert_eq!((status, answer.as_str()), (200, r#"{"state":"listening"}"#));
    rig.hearing(2).await;
    let tokens = rig.w.script.tokens();
    assert_eq!(tokens.len(), 2);
    assert_ne!(tokens[0], tokens[1], "the secret was reused");
    assert_eq!(rig.w.script.log.lock().unwrap().made, 2);
    daemon.send("DELETE", "/talk").await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_speaking_cuts_the_speech_cancels_the_turn_and_listens_again() {
    let mut rig = Rig::new(long(40, 1.0), Lang::Es, 2);
    rig.w
        .script
        .turn(vec![delta("Una respuesta larguísima. "), Step::Block]);
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.says(1, "habla").await;
    rig.until(Speaking).await;
    let (status, answer) = daemon.send("POST", "/stop").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(body(&answer)["stopped"], 0, "the speech was cut");
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.cancels(), 1);
    assert_eq!(rig.w.state.speech.state(), SpeechState::Idle);
    assert_eq!(rig.states, [Listening, Thinking, Speaking, Listening]);
    rig.talk.end().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_listening_is_a_no_op_the_same_listen_returns_the_users_final() {
    let mut rig = Rig::marked(2);
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.hearing(1).await;
    let opened = rig.w.mic.opened();
    for _ in 0..3 {
        let (status, _) = daemon.send("POST", "/stop").await;
        assert_eq!(status, 200);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        rig.drain().to_vec(),
        [Listening],
        "a stop while listening moved the conversation"
    );
    assert_eq!(rig.w.script.cancels(), 0);
    assert_eq!(rig.w.mic.opened(), opened, "the listen was reopened");
    rig.says(1, "sigo aquí").await;
    rig.until(Thinking).await;
    assert_eq!(
        rig.w.script.texts(),
        ["sigo aquí"],
        "the same listen returned the user's final"
    );
    rig.until_count(Listening, 2).await;
    rig.talk.end().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_thinking_with_an_empty_queue_cancels_the_turn_and_listens_again() {
    let mut rig = Rig::marked(2);
    rig.w.script.turn(vec![Step::Block]);
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.says(1, "piensa").await;
    rig.until(Thinking).await;
    let (status, answer) = daemon.send("POST", "/stop").await;
    assert_eq!(status, 200);
    assert_eq!(body(&answer), json!({"stopped": null, "dropped": 0}));
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.cancels(), 1);
    assert_eq!(rig.states, [Listening, Thinking, Listening]);
    rig.talk.end().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_with_no_conversation_is_the_same_as_before_the_conversation_existed() {
    let rig = Rig::marked(0);
    let daemon = door(&rig).await;
    let (status, answer) = daemon.send("POST", "/stop").await;
    assert_eq!(status, 200);
    assert_eq!(body(&answer), json!({"stopped": null, "dropped": 0}));
    daemon.stop().await;
}

fn sentences(count: usize) -> Vec<Step> {
    (1..=count).map(|n| delta(&format!("S{n}. "))).collect()
}

fn streaming(count: usize, then: Vec<Step>) -> Vec<Step> {
    sentences(count).into_iter().chain(then).collect()
}

/// A speech service that never plays: its queue fills (the worker waits on the service, 16 more behind it)
/// and the conversation's next text must wait for room.
fn wedged() -> (crate::fake::FakeSpeech, std::sync::Arc<tokio::sync::Semaphore>) {
    crate::fake::FakeSpeech::marked().gated()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_the_queue_held_full_mid_turn_delete_ends_the_conversation_at_once() {
    let (speech, _gate) = wedged();
    let mut rig = Rig::new(speech, Lang::Es, 1);
    rig.w.script.turn(streaming(25, vec![Step::Block]));
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.says(1, "habla").await;
    rig.until(Speaking).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    let (status, answer) = daemon.send("DELETE", "/talk").await;
    assert_eq!((status, answer.as_str()), (200, r#"{"state":"idle"}"#));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "ending parked behind a full queue: {:?}",
        started.elapsed()
    );
    rig.until(Idle).await;
    rig.assert_free(1).await;
    let (status, answer) = daemon.send("POST", "/talk").await;
    assert_eq!(
        (status, answer.as_str()),
        (200, r#"{"state":"listening"}"#),
        "the desk and the microphone are free"
    );
    daemon.send("DELETE", "/talk").await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_the_queue_held_full_mid_turn_stop_cancels_the_turn_and_listens_again() {
    let (speech, _gate) = wedged();
    let mut rig = Rig::new(speech, Lang::Es, 2);
    rig.w.script.turn(streaming(25, vec![Step::Block]));
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.says(1, "habla").await;
    rig.until(Speaking).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, _) = daemon.send("POST", "/stop").await;
    assert_eq!(status, 200);
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.cancels(), 1, "the turn was not cancelled");
    rig.hearing(2).await;
    rig.talk.end().await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_the_queue_held_full_mid_turn_the_turn_bound_cancels_stops_the_speech_and_listens_again() {
    let (speech, _gate) = wedged();
    let mut rig = Rig::bounded(
        speech,
        Lang::Es,
        2,
        Duration::from_secs(5),
        Duration::from_millis(600),
        Duration::from_secs(1),
    );
    rig.w.script.turn(streaming(25, vec![Step::Block]));
    rig.talk.start().await.unwrap();
    rig.says(1, "habla").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.cancels(), 1);
    assert_eq!(rig.w.state.speech.state(), SpeechState::Idle, "the speech was stopped");
    rig.hearing(2).await;
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_with_the_queue_held_full_while_a_question_waits_delete_denies_it_and_frees_everything() {
    let (speech, _gate) = wedged();
    let mut rig = Rig::new(speech, Lang::Es, 1);
    rig.w.script.turn(streaming(
        17,
        vec![
            Step::Ask("Bash".to_owned(), json!({"command": "ls"})),
            Step::Verdicts(1),
            Step::Block,
        ],
    ));
    let daemon = door(&rig).await;
    daemon.send("POST", "/talk").await;
    rig.says(1, "habla").await;
    rig.until(Asking).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    let (status, _) = daemon.send("DELETE", "/talk").await;
    assert_eq!(status, 200);
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    rig.until(Idle).await;
    assert_eq!(
        rig.answered(1).await,
        [Verdict::Deny("the voice conversation ended".to_owned())]
    );
    rig.assert_free(1).await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failure_behind_a_wedged_queue_never_parks_the_conversation() {
    use e_voice_daemon::talk::agent::base::AgentError::{Agent, Gone, Protocol, Refused, Start};
    for (error, listens_again) in [
        (Gone, true),
        (Agent("error_max_turns".to_owned()), true),
        (Refused("auto".to_owned()), false),
        (Start("x".to_owned()), false),
        (Protocol("x".to_owned()), false),
    ] {
        let (speech, _gate) = wedged();
        let mut rig = Rig::bounded(
            speech,
            Lang::Es,
            2,
            Duration::from_secs(5),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        rig.w.script.turn(streaming(17, vec![Step::Err(error.clone())]));
        rig.talk.start().await.unwrap();
        rig.says(1, "habla").await;
        let started = Instant::now();
        let wanted = if listens_again { Listening } else { Idle };
        if listens_again {
            rig.until_count(Listening, 2).await;
            rig.hearing(2).await;
        } else {
            rig.until(wanted).await;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{error:?} parked: {:?}",
            started.elapsed()
        );
        rig.talk.end().await;
        rig.until_count(Idle, 1).await;
        assert!(rig.w.mic.freed().await, "{error:?}: the microphone stayed open");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_multi_sentence_delta_is_speaking_once_its_first_piece_is_queued() {
    let (speech, _gate) = wedged();
    let mut rig = Rig::new(speech, Lang::Es, 1);
    rig.w.script.turn(vec![delta(&"Una frase. ".repeat(25)), Step::Block]);
    rig.talk.start().await.unwrap();
    rig.says(1, "habla").await;
    rig.until(Speaking).await;
    rig.talk.end().await;
}

#[derive(Clone, Copy, Debug)]
enum Via {
    Http,
    Mcp,
}

const DOORS: [Via; 2] = [Via::Http, Via::Mcp];

async fn stopping(via: Via, daemon: &Running) -> Value {
    match via {
        Via::Http => body(&daemon.send("POST", "/stop").await.1),
        Via::Mcp => crate::client::Mcp::on(daemon.port).call("stop", &json!({})).await["structuredContent"].clone(),
    }
}

async fn quiet(rig: &Rig) {
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        rig.w.state.speech.state(),
        SpeechState::Idle,
        "something was queued after the stop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_by_either_door_with_a_wedged_queue_mid_turn_empties_it_and_listens_again() {
    for via in DOORS {
        let (speech, _gate) = wedged();
        let mut rig = Rig::new(speech, Lang::Es, 2);
        rig.w.script.turn(streaming(25, vec![Step::Block]));
        let daemon = door(&rig).await;
        daemon.send("POST", "/talk").await;
        rig.says(1, "habla").await;
        rig.until(Speaking).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let halted = stopping(via, &daemon).await;
        assert!(
            halted["stopped"].is_u64(),
            "{via:?}: the first stop reported nothing cut: {halted}"
        );
        assert!(halted["dropped"].as_u64().unwrap() >= 1, "{via:?}: {halted}");
        rig.until_count(Listening, 2).await;
        assert_eq!(rig.w.script.cancels(), 1, "{via:?}");
        quiet(&rig).await;
        rig.hearing(2).await;
        rig.talk.end().await;
        daemon.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_by_either_door_with_a_wedged_queue_mid_question_denies_it_and_listens_again() {
    for via in DOORS {
        let (speech, _gate) = wedged();
        let mut rig = Rig::new(speech, Lang::Es, 2);
        rig.w.script.turn(streaming(
            17,
            vec![
                Step::Ask("Bash".to_owned(), json!({"command": "ls"})),
                Step::Verdicts(1),
                Step::Block,
            ],
        ));
        let daemon = door(&rig).await;
        daemon.send("POST", "/talk").await;
        rig.says(1, "habla").await;
        rig.until(Asking).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let halted = stopping(via, &daemon).await;
        assert!(halted["stopped"].is_u64(), "{via:?}: {halted}");
        rig.until_count(Listening, 2).await;
        assert_eq!(rig.answered(1).await, [Verdict::Deny("stopped".to_owned())], "{via:?}");
        assert_eq!(rig.w.script.cancels(), 1, "{via:?}");
        quiet(&rig).await;
        rig.hearing(2).await;
        rig.talk.end().await;
        daemon.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_with_no_conversation_answers_the_same_body_by_both_doors() {
    let rig = Rig::marked(0);
    let daemon = door(&rig).await;
    for _ in 0..3 {
        let http = stopping(Via::Http, &daemon).await;
        let mcp = stopping(Via::Mcp, &daemon).await;
        assert_eq!(http, json!({"stopped": null, "dropped": 0}));
        assert_eq!(mcp, http);
    }
    assert_eq!(rig.w.script.cancels(), 0);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_while_a_text_plays_with_no_conversation_has_the_same_shape_by_both_doors() {
    for via in DOORS {
        let rig = Rig::new(long(40, 1.0), Lang::Es, 0);
        let daemon = door(&rig).await;
        let speaking = tokio::spawn({
            let speech = rig.w.state.speech.clone();
            async move {
                speech
                    .say(e_voice_daemon::speech::base::SpeechRequest {
                        text: "hola".to_owned(),
                        voice: None,
                        lang: None,
                    })
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let halted = stopping(via, &daemon).await;
        assert!(halted["stopped"].is_u64(), "{via:?}: {halted}");
        assert_eq!(halted["dropped"], 0, "{via:?}");
        assert_eq!(halted.as_object().unwrap().len(), 2, "{via:?}");
        speaking.await.unwrap().ok();
        daemon.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_answers_within_the_halt_bound_when_the_runner_is_stuck_where_nothing_races_it() {
    for via in DOORS {
        let rig = Rig::marked(1);
        rig.w.script.hangs();
        let daemon = door(&rig).await;
        daemon.send("POST", "/talk").await;
        rig.says(1, "cuelga").await;
        while rig.w.script.texts().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let started = Instant::now();
        let halted = stopping(via, &daemon).await;
        assert!(
            started.elapsed() < Duration::from_millis(2800),
            "{via:?}: {:?}",
            started.elapsed()
        );
        assert_eq!(halted["dropped"], 0, "{via:?}");
        let ended = Instant::now();
        let (status, _) = daemon.send("DELETE", "/talk").await;
        assert_eq!(status, 200);
        assert!(
            ended.elapsed() < Duration::from_secs(12),
            "{via:?}: {:?}",
            ended.elapsed()
        );
        daemon.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_stop_whose_ack_never_comes_leaves_nothing_of_the_halted_turn_queued() {
    for via in DOORS {
        let mut rig = Rig::bounded(
            FakeSpeech::marked(),
            Lang::Es,
            2,
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(1),
        );
        rig.w.script.slow(
            Duration::from_millis(3500),
            vec![delta("Tarde. "), delta("Otra. "), Step::Done],
        );
        let daemon = door(&rig).await;
        daemon.send("POST", "/talk").await;
        rig.says(1, "tarda").await;
        while rig.w.script.texts().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let started = Instant::now();
        stopping(via, &daemon).await;
        assert!(
            started.elapsed() < Duration::from_millis(2800),
            "{via:?}: {:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(rig.spoken(), Vec::<String>::new(), "{via:?}: the halted turn spoke");
        assert_eq!(rig.w.state.speech.state(), SpeechState::Idle, "{via:?}");
        rig.until_count(Listening, 2).await;
        rig.talk.end().await;
        daemon.stop().await;
    }
}
