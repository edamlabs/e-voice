use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::speech::base::SpeechRequest;
use serde_json::{Value, json};

use crate::fake::{Device, FakeSpeech, FakeVoices, Wired, wired_with};
use crate::running::Running;

type Seen = Arc<Mutex<Vec<SpeechRequest>>>;

fn table() -> BTreeMap<String, String> {
    BTreeMap::from([("lens".to_owned(), "narrator".to_owned())])
}

fn rig(speech: FakeSpeech, lang: Lang) -> (Wired, Seen) {
    let seen = Arc::clone(&speech.requests);
    let mut w = wired_with(
        speech,
        Device::new(0, 0),
        Duration::from_secs(5),
        FakeVoices::default(),
        table(),
    );
    w.state.lang = lang;
    (w, seen)
}

struct Answer {
    status: u16,
    body: String,
    length: Option<String>,
}

async fn hook(daemon: &Running, which: &str, body: Vec<u8>, headers: &[(&str, &str)]) -> Answer {
    let mut request = reqwest::Client::new().post(format!("http://127.0.0.1:{}/hooks/{which}", daemon.port));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.body(body).send().await.unwrap();
    Answer {
        status: response.status().as_u16(),
        length: response
            .headers()
            .get("content-length")
            .map(|v| v.to_str().unwrap().to_owned()),
        body: response.text().await.unwrap(),
    }
}

fn stop_body(message: &str) -> Vec<u8> {
    json!({"session_id": "abc", "hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": message})
        .to_string()
        .into_bytes()
}

fn notification_body(kind: &str) -> Vec<u8> {
    json!({"session_id": "abc", "hook_event_name": "Notification", "notification_type": kind, "message": "ignored"})
        .to_string()
        .into_bytes()
}

async fn spoken(seen: &Seen, count: usize) -> Vec<SpeechRequest> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while seen.lock().unwrap().len() < count && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    seen.lock().unwrap().clone()
}

fn empty(answer: &Answer) {
    assert_eq!(answer.status, 200);
    assert_eq!(answer.body, "");
    assert!(answer.length.as_deref().is_none_or(|n| n == "0"), "{:?}", answer.length);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_speaks_the_digest_in_its_language_and_answers_200_empty() {
    let (w, seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    let answer = hook(
        &daemon,
        "stop",
        stop_body("I fixed the queue. It works now. Third one."),
        &[],
    )
    .await;
    empty(&answer);
    let said = spoken(&seen, 1).await;
    daemon.stop().await;
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].text, "I fixed the queue. It works now.");
    assert_eq!(said[0].lang, Some(Lang::En));
    assert_eq!(said[0].voice, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_falls_back_to_the_configured_lang_when_the_words_do_not_decide() {
    for lang in [Lang::Es, Lang::En] {
        let (w, seen) = rig(FakeSpeech::marked(), lang);
        let daemon = Running::start(w.state.clone()).await;
        hook(&daemon, "stop", stop_body("ñandú perro gato."), &[]).await;
        let said = spoken(&seen, 1).await;
        daemon.stop().await;
        assert_eq!(said[0].lang, Some(lang), "{lang:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_hook_answers_before_the_speech_service_is_contacted() {
    let (gated, gate) = FakeSpeech::marked().gated();
    let (w, seen) = rig(gated, Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    let started = Instant::now();
    let stop = hook(&daemon, "stop", stop_body("Hecho. Listo."), &[]).await;
    let notice = hook(&daemon, "notification", notification_body("permission_prompt"), &[]).await;
    let took = started.elapsed();
    empty(&stop);
    empty(&notice);
    assert!(took < Duration::from_millis(500), "{took:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the first text reached the service, which holds it"
    );
    gate.add_permits(10);
    let said = spoken(&seen, 2).await;
    daemon.stop().await;
    assert_eq!(said.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_full_queue_still_answers_200_and_drops_the_text() {
    let (gated, gate) = FakeSpeech::marked().gated();
    let (w, seen) = rig(gated, Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    let mut answers = Vec::new();
    for n in 0..20 {
        answers.push(hook(&daemon, "stop", stop_body(&format!("Mensaje número {n}.")), &[]).await);
    }
    assert!(answers.iter().all(|a| a.status == 200 && a.body.is_empty()));
    gate.add_permits(100);
    let said = spoken(&seen, 17).await;
    daemon.stop().await;
    assert_eq!(said.len(), 17, "one speaking plus sixteen waiting; the rest dropped");
    assert_eq!(said[0].text, "Mensaje número 0.");
    assert_eq!(said[16].text, "Mensaje número 16.");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_body_over_one_mebibyte_is_unread_a_body_at_it_is_read() {
    let (w, seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    let padded = |size: usize| {
        let head = r#"{"last_assistant_message":"Hola. Adiós.","pad":""#;
        let tail = r#""}"#;
        format!("{head}{}{tail}", "a".repeat(size - head.len() - tail.len())).into_bytes()
    };
    empty(&hook(&daemon, "stop", padded(1_048_577), &[]).await);
    assert_eq!(spoken(&seen, 1).await.len(), 0, "over the cap, nothing is said");
    empty(&hook(&daemon, "stop", padded(1_048_576), &[]).await);
    let said = spoken(&seen, 1).await;
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].text, "Hola. Adiós.");
    let huge = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/hooks/stop", daemon.port))
        .body(vec![b'{'; 20_000_000])
        .send()
        .await;
    assert!(
        huge.is_err() || huge.is_ok_and(|r| r.status() == 200),
        "a 20 MB body must be 200 or a hung-up connection"
    );
    assert_eq!(spoken(&seen, 1).await.len(), 1, "nothing more was said");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_input_that_is_not_a_usable_stop_says_nothing_and_still_answers_200() {
    let (w, seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    let bodies: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"not json".to_vec(),
        b"{".to_vec(),
        b"[]".to_vec(),
        b"null".to_vec(),
        b"\"texto\"".to_vec(),
        vec![0xff, 0xfe, 0x00],
        b"{}".to_vec(),
        json!({"last_assistant_message": null}).to_string().into_bytes(),
        json!({"last_assistant_message": 7}).to_string().into_bytes(),
        json!({"last_assistant_message": ["Hola."]}).to_string().into_bytes(),
        json!({"last_assistant_message": ""}).to_string().into_bytes(),
        json!({"last_assistant_message": "   "}).to_string().into_bytes(),
        json!({"last_assistant_message": "```\nsolo código\n```"})
            .to_string()
            .into_bytes(),
        json!({"last_assistant_message": "🎉"}).to_string().into_bytes(),
    ];
    for body in bodies {
        let shown = String::from_utf8_lossy(&body).into_owned();
        let answer = hook(&daemon, "stop", body, &[]).await;
        assert_eq!((answer.status, answer.body.as_str()), (200, ""), "{shown}");
    }
    assert_eq!(spoken(&seen, 1).await.len(), 0);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_content_type_does_not_matter() {
    let (w, seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    for kind in [
        None,
        Some("application/json"),
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
    ] {
        let headers: Vec<(&str, &str)> = kind.map(|k| vec![("Content-Type", k)]).unwrap_or_default();
        empty(&hook(&daemon, "stop", stop_body("Hecho. Listo."), &headers).await);
    }
    assert_eq!(spoken(&seen, 4).await.len(), 4);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_each_known_notification_type_says_its_exact_phrase_in_the_configured_lang() {
    let phrases = [
        ("permission_prompt", Lang::Es, "Necesito tu permiso."),
        ("permission_prompt", Lang::En, "I need your permission."),
        ("idle_prompt", Lang::Es, "Estoy esperando."),
        ("idle_prompt", Lang::En, "I'm waiting for you."),
    ];
    for (kind, lang, phrase) in phrases {
        let (w, seen) = rig(FakeSpeech::marked(), lang);
        let daemon = Running::start(w.state.clone()).await;
        empty(&hook(&daemon, "notification", notification_body(kind), &[]).await);
        let said = spoken(&seen, 1).await;
        daemon.stop().await;
        assert_eq!(said.len(), 1, "{kind} {lang:?}");
        assert_eq!((said[0].text.as_str(), said[0].lang), (phrase, Some(lang)), "{kind}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_other_or_missing_notification_types_say_nothing_and_answer_200() {
    let (w, seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    let bodies: Vec<Vec<u8>> = vec![
        notification_body("auth_success"),
        notification_body("agent_needs_input"),
        notification_body("elicitation_dialog"),
        notification_body(""),
        b"{}".to_vec(),
        b"not json".to_vec(),
        b"".to_vec(),
        json!({"notification_type": null}).to_string().into_bytes(),
        json!({"notification_type": 3}).to_string().into_bytes(),
        json!({"notification_type": ["idle_prompt"]}).to_string().into_bytes(),
        json!({"message": "Claude needs your permission"})
            .to_string()
            .into_bytes(),
    ];
    for body in bodies {
        let shown = String::from_utf8_lossy(&body).into_owned();
        let answer = hook(&daemon, "notification", body, &[]).await;
        assert_eq!((answer.status, answer.body.as_str()), (200, ""), "{shown}");
    }
    assert_eq!(spoken(&seen, 1).await.len(), 0);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_voice_is_the_one_the_header_names_in_the_table_and_none_without_it() {
    let (w, seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    hook(&daemon, "stop", stop_body("Primero."), &[("X-Evoice-Client", "lens")]).await;
    hook(&daemon, "stop", stop_body("Segundo."), &[]).await;
    hook(
        &daemon,
        "stop",
        stop_body("Tercero."),
        &[("x-evoice-client", "stranger")],
    )
    .await;
    hook(
        &daemon,
        "notification",
        notification_body("idle_prompt"),
        &[("X-EVOICE-CLIENT", "lens")],
    )
    .await;
    hook(
        &daemon,
        "notification",
        notification_body("idle_prompt"),
        &[("X-Evoice-Client", "LENS")],
    )
    .await;
    let said = spoken(&seen, 5).await;
    daemon.stop().await;
    let voices: Vec<Option<&str>> = said.iter().map(|r| r.voice.as_deref()).collect();
    assert_eq!(voices, [Some("narrator"), None, None, Some("narrator"), None]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_hook_text_takes_an_id_in_the_one_queue_and_a_failing_one_does_not_wedge_it() {
    let mut broken = FakeSpeech::marked();
    broken.fail_say = Some(e_voice_daemon::speech::base::SpeechError::Server("down".to_owned()));
    let (w, seen) = rig(broken, Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    empty(&hook(&daemon, "stop", stop_body("Hecho. Listo."), &[]).await);
    assert_eq!(spoken(&seen, 1).await.len(), 1);
    let (status, body) = daemon.speak(r#"{"text":"x"}"#).await;
    daemon.stop().await;
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("speech service: down"), "{body}");
    assert_eq!(w.state.speech.state(), e_voice_daemon::schema::event::SpeechState::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_hook_speech_and_http_speech_share_ids_and_hooks_are_post_only() {
    let (w, _seen) = rig(FakeSpeech::marked(), Lang::Es);
    let daemon = Running::start(w.state.clone()).await;
    hook(&daemon, "stop", stop_body("Hecho. Listo."), &[]).await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while w.state.speech.state() != e_voice_daemon::schema::event::SpeechState::Idle && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (status, body) = daemon.speak(r#"{"text":"1"}"#).await;
    assert_eq!((status, body.as_str()), (200, r#"{"id":1,"outcome":"done"}"#));
    let client = reqwest::Client::new();
    for which in ["stop", "notification"] {
        let url = format!("http://127.0.0.1:{}/hooks/{which}", daemon.port);
        assert_eq!(client.get(&url).send().await.unwrap().status(), 405);
    }
    let unknown = client
        .post(format!("http://127.0.0.1:{}/hooks/other", daemon.port))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
    daemon.stop().await;
    let _: Option<Value> = None;
}
