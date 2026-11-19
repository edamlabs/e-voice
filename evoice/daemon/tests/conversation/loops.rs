use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::HearingError;
use e_voice_daemon::schema::event::SpeechState;
use e_voice_daemon::talk::agent::base::AgentError;
use e_voice_daemon::talk::runner::TalkError;

use crate::fake::{FakeSpeech, Step, TalkState::*, delta, long};
use crate::harness::Rig;

#[tokio::test(flavor = "multi_thread")]
async fn test_a_turn_walks_listening_thinking_speaking_listening_and_speaks_the_replys_sentences() {
    let mut rig = Rig::marked(2);
    rig.w.script.turn(vec![
        delta("Hola. ¿Qué"),
        delta(" tal? "),
        Step::Wait(Duration::from_millis(80)),
        Step::Done,
    ]);
    assert_eq!(rig.talk.start().await, Ok(Listening));
    rig.says(1, "hola agente").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.states, [Listening, Thinking, Speaking, Listening]);
    rig.says(2, "adiós").await;
    rig.until(Idle).await;
    assert_eq!(rig.states, [Listening, Thinking, Speaking, Listening, Idle]);
    assert_eq!(rig.spoken(), ["Hola.", "¿Qué tal?", "Hasta luego."]);
    assert_eq!(rig.w.script.texts(), ["hola agente"]);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_reply_is_chunked_into_sentences_and_a_sentence_without_a_mark_is_said_at_the_end_of_the_turn() {
    let mut rig = Rig::marked(2);
    rig.w.script.turn(vec![delta("Uno. Dos"), delta(" y tres"), Step::Done]);
    rig.talk.start().await.unwrap();
    rig.says(1, "cuenta").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.spoken(), ["Uno.", "Dos y tres"]);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_first_listen_waits_for_what_the_conversation_said_to_play_and_markdown_is_not_read() {
    let mut rig = Rig::marked(2);
    rig.w.script.turn(vec![
        delta("# Título\n"),
        delta("- uno\n- dos\n"),
        delta("```\nSECRETO\n```\n"),
        delta("Fin. "),
        Step::Done,
    ]);
    rig.talk.start().await.unwrap();
    rig.says(1, "lista").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.spoken(), ["uno", "dos", "Fin."]);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_microphone_is_never_opened_while_the_agent_thinks_or_speaks() {
    let mut rig = Rig::new(long(8, 1.0), Lang::Es, 2);
    rig.w.script.turn(vec![
        Step::Wait(Duration::from_millis(300)),
        delta("Uno. "),
        Step::Wait(Duration::from_millis(300)),
        delta("Dos. "),
        Step::Done,
    ]);
    rig.talk.start().await.unwrap();
    rig.says(1, "habla").await;
    rig.until(Thinking).await;
    let opens = rig.w.mic.opened();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.w.mic.opened(), opens, "the opener ran while the agent was thinking");
    assert!(!rig.w.mic.is_open(), "the microphone is open while the agent thinks");
    rig.until(Speaking).await;
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if rig.w.state.speech.state() == SpeechState::Idle {
            break;
        }
        assert_eq!(
            rig.w.mic.opened(),
            opens,
            "the opener ran while the conversation was speaking"
        );
        assert!(
            !rig.w.mic.is_open(),
            "the microphone is open while the conversation speaks"
        );
    }
    rig.until_count(Listening, 2).await;
    rig.hearing(opens + 1).await;
    assert_eq!(rig.w.mic.opened(), opens + 1, "exactly one listen after the turn");
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_nothing_is_listened_for_until_what_was_queued_has_played() {
    let mut rig = Rig::new(long(12, 1.0), Lang::Es, 2);
    rig.w.script.turn(vec![delta("Una frase larga. "), Step::Done]);
    rig.talk.start().await.unwrap();
    rig.says(1, "habla").await;
    rig.until(Speaking).await;
    let opens = rig.w.mic.opened();
    let started = Instant::now();
    rig.until_count(Listening, 2).await;
    rig.hearing(opens + 1).await;
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "the listen opened before the speech ended: {:?}",
        started.elapsed()
    );
    assert_eq!(rig.w.state.speech.state(), SpeechState::Idle);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_goodbyes_are_whole_utterances_in_es_and_en_and_end_without_a_turn() {
    for (lang, text) in [
        (Lang::Es, "adiós"),
        (Lang::Es, "Adiós."),
        (Lang::Es, "¡Adiós!"),
        (Lang::Es, "  hasta luego  "),
        (Lang::Es, "Terminamos."),
        (Lang::En, "goodbye"),
        (Lang::En, "Goodbye!"),
        (Lang::En, "That’s all."),
        (Lang::En, "bye"),
    ] {
        let mut rig = Rig::new(FakeSpeech::marked(), lang, 1);
        rig.talk.start().await.unwrap();
        rig.says(1, text).await;
        rig.until(Idle).await;
        assert_eq!(rig.w.script.texts(), Vec::<String>::new(), "{text:?} reached the agent");
        let said = if lang == Lang::Es { "Hasta luego." } else { "Goodbye." };
        assert_eq!(rig.spoken(), [said], "{text:?}");
        rig.assert_free(1).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_what_only_starts_like_a_goodbye_is_a_turn() {
    for (lang, text) in [
        (Lang::Es, "adiós a todos"),
        (Lang::Es, "para"),
        (Lang::Es, "hasta luego mañana lo vemos"),
        (Lang::Es, "no te digo adiós"),
        (Lang::En, "goodbye everyone"),
        (Lang::En, "stop"),
        (Lang::En, "bye bye"),
    ] {
        let mut rig = Rig::new(FakeSpeech::marked(), lang, 2);
        rig.talk.start().await.unwrap();
        rig.says(1, text).await;
        rig.until_count(Listening, 2).await;
        assert_eq!(rig.w.script.texts(), [text], "{text:?} was not a turn");
        rig.talk.end().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_nobody_speaking_for_the_idle_bound_ends_it_with_a_goodbye() {
    let mut rig = Rig::marked(1);
    let started = Instant::now();
    rig.talk.start().await.unwrap();
    rig.until(Idle).await;
    assert!(
        started.elapsed() >= Duration::from_millis(550),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(rig.spoken(), ["Hasta luego."]);
    assert_eq!(rig.states, [Listening, Idle]);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_listen_that_fails_says_so_and_ends() {
    let mut rig = Rig::marked(1);
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    rig.feeds[0].fail(HearingError::Server("closed with 1011".to_owned()));
    rig.until(Idle).await;
    assert_eq!(rig.spoken(), ["Algo ha fallado en el agente."]);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_lost_agent_is_said_and_the_next_turn_reaches_the_agent_again() {
    let mut rig = Rig::marked(3);
    rig.w.script.turn(vec![Step::Err(AgentError::Gone)]);
    rig.talk.start().await.unwrap();
    rig.says(1, "uno").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.spoken(), ["Se ha cortado el agente; sigo."]);
    rig.says(2, "dos").await;
    rig.until_count(Listening, 3).await;
    assert_eq!(rig.w.script.texts(), ["uno", "dos"]);
    assert_eq!(rig.spoken().last().map(String::as_str), Some("Ok."));
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_refused_agent_says_so_ends_and_takes_no_more_turns() {
    let mut rig = Rig::marked(1);
    rig.w
        .script
        .turn(vec![Step::Err(AgentError::Refused("mode auto".to_owned()))]);
    rig.talk.start().await.unwrap();
    rig.says(1, "uno").await;
    rig.until(Idle).await;
    assert_eq!(rig.spoken(), ["El agente no arrancó como debía; termino."]);
    assert_eq!(rig.w.script.texts(), ["uno"]);
    assert_eq!(rig.w.mic.opened(), 1, "no second listen after a refused agent");
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_start_or_protocol_error_says_failed_and_ends_while_a_failed_turn_says_it_and_goes_on() {
    for error in [AgentError::Start("x".to_owned()), AgentError::Protocol("x".to_owned())] {
        let mut rig = Rig::marked(1);
        rig.w.script.turn(vec![Step::Err(error.clone())]);
        rig.talk.start().await.unwrap();
        rig.says(1, "uno").await;
        rig.until(Idle).await;
        assert_eq!(rig.spoken(), ["Algo ha fallado en el agente."], "{error:?}");
        rig.assert_free(1).await;
    }
    let mut rig = Rig::marked(3);
    rig.w
        .script
        .turn(vec![Step::Err(AgentError::Agent("error_max_turns: x".to_owned()))]);
    rig.talk.start().await.unwrap();
    rig.says(1, "uno").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.spoken(), ["Algo ha fallado en el agente."]);
    rig.says(2, "dos").await;
    rig.until_count(Listening, 3).await;
    assert_eq!(rig.w.script.texts(), ["uno", "dos"]);
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_agent_that_cannot_start_a_turn_is_handled_like_a_failed_one() {
    let mut rig = Rig::marked(1);
    rig.w.script.fails(AgentError::Start("no binary".to_owned()));
    rig.talk.start().await.unwrap();
    rig.says(1, "uno").await;
    rig.until(Idle).await;
    assert_eq!(rig.spoken(), ["Algo ha fallado en el agente."]);
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_turn_past_its_bound_is_cut_the_speech_stopped_and_it_listens_again() {
    let mut rig = Rig::bounded(
        FakeSpeech::marked(),
        Lang::Es,
        2,
        Duration::from_secs(5),
        Duration::from_millis(400),
        Duration::from_secs(1),
    );
    rig.w.script.turn(vec![Step::Block]);
    rig.talk.start().await.unwrap();
    rig.says(1, "tarda").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.cancels(), 1);
    assert_eq!(rig.spoken(), ["Llevaba demasiado tiempo; lo he cortado."]);
    rig.says(2, "adiós").await;
    rig.until(Idle).await;
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_second_start_is_refused_running_and_so_is_a_start_while_a_listen_holds_the_microphone() {
    let rig = Rig::marked(2);
    rig.talk.start().await.unwrap();
    assert_eq!(rig.talk.start().await, Err(TalkError::Running));
    rig.hearing(1).await;
    rig.talk.end().await;
    let listen = std::sync::Arc::clone(&rig.w.state.listen);
    let feed = &rig.feeds[1];
    let holding = tokio::spawn(async move { listen.listen(Some(Duration::from_secs(5))).await });
    rig.hearing(2).await;
    assert_eq!(rig.talk.start().await, Err(TalkError::Microphone));
    feed.say(e_voice_daemon::hearing::base::Recognized::Started(
        std::time::Duration::ZERO,
    ));
    feed.say(e_voice_daemon::hearing::base::Recognized::Final(
        crate::fake::heard("hola"),
        None,
    ));
    holding.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_listen_while_a_conversation_holds_the_microphone_is_busy() {
    let rig = Rig::marked(1);
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    let refused = rig.w.state.listen.listen(Some(Duration::from_secs(1))).await;
    assert!(
        matches!(refused, Err(e_voice_daemon::workflow::listen::ListenError::Busy)),
        "{refused:?}"
    );
    rig.talk.end().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_halt_while_listening_leaves_the_next_goodbye_said() {
    let mut rig = Rig::marked(1);
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    rig.talk.stop().await.unwrap();
    rig.says(1, "adiós").await;
    rig.until(Idle).await;
    assert_eq!(rig.spoken(), ["Hasta luego."]);
    assert_eq!(rig.w.script.texts(), Vec::<String>::new());
    rig.assert_free(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_halt_while_listening_leaves_the_next_turn_spoken() {
    let mut rig = Rig::marked(2);
    rig.talk.start().await.unwrap();
    rig.hearing(1).await;
    rig.talk.stop().await.unwrap();
    rig.says(1, "hola").await;
    rig.until_count(Listening, 2).await;
    assert_eq!(rig.w.script.texts(), ["hola"]);
    assert_eq!(rig.spoken(), ["Ok."]);
    rig.talk.end().await;
}
