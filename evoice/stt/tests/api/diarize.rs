use std::sync::Arc;

use e_voice_core::schema::error::{DiarizeStage, NodeError};
use e_voice_stt::schema::segment::SegmentSpan;
use e_voice_stt::schema::speaker::{SpeakerId, Turn};
use e_voice_stt::workflow::nodes::Nodes;
use reqwest::multipart::{Form, Part};
use serde_json::Value;

use crate::app::{self, wav};
use crate::fake::{FakeDiarize, batch, diarizing, nodes};

const LOUD: f32 = 0.9;

fn speech() -> Vec<u8> {
    wav(
        16_000,
        &[
            (0.0, 16_000),
            (LOUD, 8_000),
            (0.0, 16_000),
            (LOUD, 16_000),
            (0.0, 8_000),
        ],
    )
}

/// Segments sit at 1.0-1.5 s and 2.5-3.5 s; the backend ids (5, 3) are renumbered by first appearance.
fn voices() -> Vec<Turn> {
    let turn = |start, end, speaker| Turn {
        span: SegmentSpan { start, end },
        speaker: SpeakerId(speaker),
    };
    vec![turn(0, 32_000, 5), turn(32_000, 64_000, 3)]
}

fn with_node() -> (Arc<Nodes>, Arc<FakeDiarize>) {
    let fake = FakeDiarize::new(Ok(voices()));
    (diarizing(batch(0), &fake), fake)
}

fn without_node() -> Arc<Nodes> {
    nodes(batch(0), None, None)
}

async fn openai(nodes: Arc<Nodes>, fields: &[(&str, &str)]) -> reqwest::Response {
    let (address, _) = app::serve(nodes, 1 << 20, false).await;
    let form = fields.iter().fold(
        Form::new().part("file", Part::bytes(speech()).file_name("speech.wav")),
        |form, (name, value)| form.text(name.to_string(), value.to_string()),
    );
    reqwest::Client::new()
        .post(format!("http://{address}/v1/audio/transcriptions"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

async fn deepgram(nodes: Arc<Nodes>, query: &str) -> reqwest::Response {
    let (address, _) = app::serve(nodes, 1 << 20, false).await;
    reqwest::Client::new()
        .post(format!("http://{address}/v1/listen?language=es{query}"))
        .header("content-type", "audio/wav")
        .body(speech())
        .send()
        .await
        .unwrap()
}

async fn steady(response: reqwest::Response) -> Value {
    let mut body: Value = response.json().await.unwrap();
    body["metadata"]["request_id"] = Value::Null;
    body["results"]["utterances"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .for_each(|utterance| utterance["id"] = Value::Null);
    body
}

async fn scribe(nodes: Arc<Nodes>, fields: &[(&str, &str)]) -> reqwest::Response {
    let (address, _) = app::serve(nodes, 1 << 20, false).await;
    let form = fields.iter().fold(
        Form::new().part("file", Part::bytes(speech()).file_name("speech.wav")),
        |form, (name, value)| form.text(name.to_string(), value.to_string()),
    );
    reqwest::Client::new()
        .post(format!("http://{address}/v1/speech-to-text"))
        .multipart(form)
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openai_diarized_json_names_speakers_a_then_b() {
    let (nodes, fake) = with_node();
    let response = openai(nodes, &[("model", "whisper-1"), ("response_format", "diarized_json")]).await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["task"], "transcribe", "{body}");
    assert_eq!(body["duration"], 4.0);
    assert_eq!(body["text"], "batch:8000 batch:16000");
    let segments = body["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 2, "{body}");
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment["speaker"].as_str())
            .collect::<Vec<_>>(),
        [Some("A"), Some("B")]
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment["type"] == "transcript.text.segment" && segment["id"].is_string())
    );
    assert_eq!(
        (
            segments[0]["start"].as_f64(),
            segments[0]["end"].as_f64(),
            segments[1]["text"].as_str()
        ),
        (Some(1.0), Some(1.5), Some("batch:16000"))
    );
    assert_eq!(fake.calls(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openai_diarized_json_with_node_off_is_400() {
    let response = openai(
        without_node(),
        &[("model", "whisper-1"), ("response_format", "diarized_json")],
    )
    .await;
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error", "{body}");
    assert_eq!(body["error"]["param"], "response_format");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("diariz"),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openai_without_the_ask_is_byte_identical_and_never_runs() {
    let (nodes, fake) = with_node();
    for format in ["json", "verbose_json", "text", "srt", "vtt"] {
        let fields = [("model", "whisper-1"), ("response_format", format)];
        let plain = openai(without_node(), &fields).await.bytes().await.unwrap();
        let loaded = openai(Arc::clone(&nodes), &fields).await.bytes().await.unwrap();
        assert_eq!(plain, loaded, "{format}");
    }
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openai_stream_carries_speaker_segments_before_done() {
    let (nodes, fake) = with_node();
    let body = openai(
        nodes,
        &[
            ("model", "whisper-1"),
            ("response_format", "diarized_json"),
            ("stream", "true"),
        ],
    )
    .await
    .text()
    .await
    .unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    let kinds: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    assert_eq!(kinds.last(), Some(&"transcript.text.done"), "{kinds:?}");
    let segments: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "transcript.text.segment")
        .collect();
    assert_eq!(
        segments
            .iter()
            .map(|event| event["speaker"].as_str())
            .collect::<Vec<_>>(),
        [Some("A"), Some("B")],
        "{kinds:?}"
    );
    assert!(
        events
            .iter()
            .filter(|event| event["type"] == "transcript.text.delta")
            .all(|event| event.get("speaker").is_none())
    );
    let last_segment = kinds
        .iter()
        .rposition(|kind| *kind == "transcript.text.segment")
        .unwrap();
    assert!(last_segment < kinds.len() - 1, "{kinds:?}");
    assert_eq!(fake.calls(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openai_plain_stream_without_the_ask_is_byte_identical() {
    let (nodes, fake) = with_node();
    let fields = [("model", "whisper-1"), ("stream", "true")];
    let plain = openai(without_node(), &fields).await.bytes().await.unwrap();
    let loaded = openai(nodes, &fields).await.bytes().await.unwrap();
    assert_eq!(plain, loaded);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openai_diarization_failure_after_the_ask_is_not_a_silent_success() {
    let fake = FakeDiarize::new(Err(NodeError::Diarize(DiarizeStage::Process)));
    let response = openai(
        diarizing(batch(0), &fake),
        &[("model", "whisper-1"), ("response_format", "diarized_json")],
    )
    .await;
    assert!(response.status().is_server_error(), "{}", response.status());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_diarize_numbers_utterances_from_zero_and_leaves_words_empty() {
    for query in ["&diarize=true", "&diarize_model=latest"] {
        let (nodes, fake) = with_node();
        let response = deepgram(nodes, query).await;
        assert_eq!(response.status(), 200, "{query}");
        let body: Value = response.json().await.unwrap();
        let utterances = body["results"]["utterances"].as_array().unwrap();
        assert_eq!(
            utterances
                .iter()
                .map(|utterance| utterance["speaker"].as_u64())
                .collect::<Vec<_>>(),
            [Some(0), Some(1)],
            "{query}: {body}"
        );
        assert_eq!(
            body["results"]["channels"][0]["alternatives"][0]["words"],
            serde_json::json!([]),
            "{query}"
        );
        assert_eq!(fake.calls(), 1, "{query}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_deepgram_without_the_ask_is_identical_and_never_runs() {
    let (nodes, fake) = with_node();
    let plain = steady(deepgram(without_node(), "").await).await;
    assert_eq!(plain, steady(deepgram(Arc::clone(&nodes), "").await).await);
    assert_eq!(plain, steady(deepgram(without_node(), "&diarize=false").await).await);
    assert_eq!(fake.calls(), 0);
    let ignored = deepgram(without_node(), "&diarize=true").await;
    assert_eq!(ignored.status(), 200);
    let body = steady(ignored).await;
    assert_eq!(body, plain);
    assert!(
        body["results"]["utterances"]
            .as_array()
            .unwrap()
            .iter()
            .all(|utterance| utterance.get("speaker").is_none())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scribe_diarize_labels_words_speaker_n() {
    let (nodes, fake) = with_node();
    let body: Value = scribe(nodes, &[("model_id", "scribe_v1"), ("diarize", "true")])
        .await
        .json()
        .await
        .unwrap();
    let words = body["words"].as_array().unwrap();
    assert_eq!(
        words.iter().map(|word| word["speaker_id"].as_str()).collect::<Vec<_>>(),
        [Some("speaker_0"), Some("speaker_1")],
        "{body}"
    );
    assert_eq!(fake.calls(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scribe_without_the_ask_is_byte_identical_and_never_runs() {
    let (nodes, fake) = with_node();
    let fields = [("model_id", "scribe_v1")];
    let plain = scribe(without_node(), &fields).await.bytes().await.unwrap();
    let loaded = scribe(Arc::clone(&nodes), &fields).await.bytes().await.unwrap();
    assert_eq!(plain, loaded);
    let false_ask = scribe(nodes, &[("model_id", "scribe_v1"), ("diarize", "false")])
        .await
        .bytes()
        .await
        .unwrap();
    assert_eq!(plain, false_ask);
    assert_eq!(fake.calls(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scribe_num_speakers_and_threshold_are_accepted_and_ignored() {
    let (nodes, _) = with_node();
    let response = scribe(
        nodes,
        &[
            ("model_id", "scribe_v1"),
            ("diarize", "true"),
            ("num_speakers", "2"),
            ("diarization_threshold", "0.3"),
        ],
    )
    .await;
    assert_eq!(response.status(), 200);
}

fn failing() -> Arc<Nodes> {
    diarizing(
        batch(0),
        &FakeDiarize::new(Err(NodeError::Diarize(DiarizeStage::Process))),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn test_control_openai_ask_differs_under_the_raw_bytes_comparator() {
    let (nodes, _) = with_node();
    let plain = openai(without_node(), &[("model", "x"), ("response_format", "json")])
        .await
        .bytes()
        .await
        .unwrap();
    let asked = openai(
        Arc::clone(&nodes),
        &[("model", "x"), ("response_format", "diarized_json")],
    )
    .await
    .bytes()
    .await
    .unwrap();
    assert_ne!(plain, asked);
    let stream = [("model", "x"), ("stream", "true")];
    let plain = openai(without_node(), &stream).await.bytes().await.unwrap();
    let asked = openai(
        nodes,
        &[("model", "x"), ("stream", "true"), ("response_format", "diarized_json")],
    )
    .await
    .bytes()
    .await
    .unwrap();
    assert_ne!(plain, asked);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_control_deepgram_ask_differs_under_the_masked_comparator() {
    let (nodes, _) = with_node();
    let plain = steady(deepgram(without_node(), "").await).await;
    let asked = steady(deepgram(nodes, "&diarize=true").await).await;
    assert_ne!(plain, asked);
    assert_eq!(
        asked["results"]["utterances"][0]["speaker"], 0,
        "the mask must not swallow speaker: {asked}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_control_scribe_ask_differs_under_the_raw_bytes_comparator() {
    let (nodes, _) = with_node();
    let plain = scribe(without_node(), &[("model_id", "scribe_v1")])
        .await
        .bytes()
        .await
        .unwrap();
    let asked = scribe(nodes, &[("model_id", "scribe_v1"), ("diarize", "true")])
        .await
        .bytes()
        .await
        .unwrap();
    assert_ne!(plain, asked);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failure_openai_json_is_a_500_server_error() {
    let response = openai(failing(), &[("model", "x"), ("response_format", "diarized_json")]).await;
    assert_eq!(response.status(), 500);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "server_error", "{body}");
    assert_eq!(body["error"]["message"], "diarization failed at process");
    assert!(body["error"]["param"].is_null() && body["error"]["code"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failure_openai_stream_ends_on_an_error_event_without_done() {
    let response = openai(
        failing(),
        &[("model", "x"), ("response_format", "diarized_json"), ("stream", "true")],
    )
    .await;
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    let events: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    let kinds: Vec<&str> = events.iter().map(|event| event["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        ["transcript.text.delta", "transcript.text.delta", "error"],
        "{text}"
    );
    let last = events.last().unwrap();
    assert_eq!(last["error"]["type"], "server_error");
    assert_eq!(last["error"]["message"], "diarization failed at process");
    assert!(!text.contains("transcript.text.done") && !text.contains("transcript.text.segment"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failure_deepgram_is_a_500_in_deepgram_error_shape() {
    let response = deepgram(failing(), "&diarize=true").await;
    assert_eq!(response.status(), 500);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["err_code"], "Internal Server Error", "{body}");
    assert_eq!(body["err_msg"], "diarization failed at process");
    assert!(body["request_id"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failure_scribe_is_a_500_in_elevenlabs_detail_shape() {
    let response = scribe(failing(), &[("model_id", "scribe_v1"), ("diarize", "true")]).await;
    assert_eq!(response.status(), 500);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"]["status"], "internal_error", "{body}");
    assert_eq!(body["detail"]["message"], "diarization failed at process");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failure_not_asked_is_still_a_success_on_every_surface() {
    assert_eq!(openai(failing(), &[("model", "x")]).await.status(), 200);
    assert_eq!(deepgram(failing(), "").await.status(), 200);
    assert_eq!(scribe(failing(), &[("model_id", "scribe_v1")]).await.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_failure_openai_json_when_the_backend_panics_is_a_500_with_its_message() {
    let response = openai(
        crate::fake::panicking(batch(0)),
        &[("model", "x"), ("response_format", "diarized_json")],
    )
    .await;
    assert_eq!(response.status(), 500);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "server_error", "{body}");
    assert_eq!(body["error"]["message"], "backend failure: diarize panicked");
}
