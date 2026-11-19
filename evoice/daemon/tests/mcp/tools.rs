use std::time::{Duration, Instant};

use e_voice_core::schema::error::NodeError;
use e_voice_daemon::hearing::base::{HearingError, Recognized};
use e_voice_daemon::schema::event::SpeechState;
use e_voice_daemon::speech::base::SpeechError;
use serde_json::{Value, json};

use crate::client::{Mcp, failure};
use crate::fake::{Device, FakeSpeech, Wired, heard, long, wired};
use crate::running::Running;

async fn up(w: &Wired) -> (Running, Mcp) {
    let daemon = Running::start(w.state.clone()).await;
    let mcp = Mcp::on(daemon.port);
    (daemon, mcp)
}

fn plain() -> Wired {
    wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_secs(5))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_tool_list_is_pinned_names_descriptions_and_input_schemas() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let listed = mcp.tools().await;
    daemon.stop().await;
    let expected = json!({"tools": [
        {
            "name": "approve",
            "description": "Claude Code's permission prompt for a voice conversation: the listener is asked aloud, and the answer\nis Claude Code's own text block. Answers the conversation's own agent alone; anyone else is refused.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "properties": {
                    "input": {"description": "That tool's input."},
                    "tool_name": {"description": "The tool asked about.", "type": "string"}
                },
                "required": ["tool_name", "input"],
                "type": "object"
            }
        },
        {
            "name": "list_voices",
            "description": "The voices `speak` accepts: {voices: [id, …]}.",
            "inputSchema": {"properties": {}, "type": "object"}
        },
        {
            "name": "listen",
            "description": "Listen until someone speaks and answer with what they said: {text, lang, emotion}.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "properties": {"timeout_seconds": {
                    "description": "Seconds to wait for someone to start speaking, 1 to 300; the daemon's default when omitted.",
                    "format": "uint64",
                    "minimum": 0,
                    "type": ["integer", "null"]
                }},
                "type": "object"
            }
        },
        {
            "name": "speak",
            "description": "Say a text aloud, in turn with every other, and answer once it has played: {id, outcome}.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "properties": {
                    "text": {"description": "What to say, at most 4096 characters.", "type": "string"},
                    "voice": {
                        "description": "A voice id from `list_voices`; when omitted, the voice configured for this client's\n`X-Evoice-Client`, else the speech service's default.",
                        "type": ["string", "null"]
                    }
                },
                "required": ["text"],
                "type": "object"
            }
        },
        {
            "name": "stop",
            "description": "Silence what is being said and drop every text waiting: {stopped: id or null, dropped}.",
            "inputSchema": {"properties": {}, "type": "object"}
        }
    ]});
    assert_eq!(listed, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_succeeds_with_the_id_and_the_outcome_as_text_and_structured_content() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let first = mcp.call("speak", &json!({"text": "4"})).await;
    let second = mcp.call("speak", &json!({"text": "5", "voice": "ef_dora"})).await;
    daemon.stop().await;
    assert_eq!(
        first,
        json!({"content": [{"type": "text", "text": "{\"id\":0,\"outcome\":\"done\"}"}], "structuredContent": {"id": 0, "outcome": "done"}, "isError": false})
    );
    assert_eq!(second["structuredContent"], json!({"id": 1, "outcome": "done"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stop_answers_stopped_and_dropped_idle_and_busy() {
    let w = wired(long(300, 1.0), Device::new(0, 0), Duration::from_secs(5));
    let (daemon, mcp) = up(&w).await;
    let idle = mcp.call("stop", &json!({})).await;
    assert_eq!(idle["structuredContent"], json!({"stopped": null, "dropped": 0}));
    assert_eq!(idle["isError"], false);
    let speaker = mcp.clone();
    let speaking = tokio::spawn(async move { speaker.call("speak", &json!({"text": "x"})).await });
    while w.state.speech.state() != (SpeechState::Speaking { id: 0 }) {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let busy = mcp.call("stop", &json!({})).await;
    assert_eq!(busy["structuredContent"], json!({"stopped": 0, "dropped": 0}));
    let cut = speaking.await.unwrap();
    assert_eq!(cut["structuredContent"], json!({"id": 0, "outcome": "cancelled"}));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listen_succeeds_with_text_lang_and_emotion() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let feed = w.hearing.session();
    let asking = mcp.clone();
    let call = tokio::spawn(async move { asking.call("listen", &json!({"timeout_seconds": 10})).await });
    assert!(w.mic.live().await);
    feed.say(Recognized::Started);
    feed.say(Recognized::Final(heard("hola"), None));
    let result = call.await.unwrap();
    daemon.stop().await;
    assert_eq!(
        result["structuredContent"],
        json!({"text": "hola", "lang": "es", "emotion": {"label": "unknown", "scores": {}, "model": null}})
    );
    assert_eq!(result["isError"], false);
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_voices_succeeds_with_the_ids_exact() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let result = mcp.call("list_voices", &json!({})).await;
    daemon.stop().await;
    assert_eq!(result["structuredContent"], json!({"voices": ["ef_dora", "em_alex"]}));
    assert_eq!(result["isError"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_speak_failures_are_tool_errors_with_the_message_and_the_http_status() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let empty = mcp.call("speak", &json!({"text": "   "})).await;
    assert_eq!(failure(&empty), ("text is empty".to_owned(), 400));
    let long_text = "a".repeat(4097);
    let over = mcp.call("speak", &json!({"text": long_text})).await;
    assert_eq!(
        failure(&over),
        ("text is too long: 4097 characters, at most 4096".to_owned(), 400)
    );
    let exact = mcp.call("speak", &json!({"text": "b".repeat(4096)})).await;
    assert_eq!(exact["isError"], false, "{exact}");
    daemon.stop().await;

    let mut broken = FakeSpeech::marked();
    broken.fail_say = Some(SpeechError::Server("cut".to_owned()));
    let w = wired(broken, Device::new(0, 0), Duration::from_secs(5));
    let (daemon, mcp) = up(&w).await;
    let bad_gateway = mcp.call("speak", &json!({"text": "x"})).await;
    daemon.stop().await;
    assert_eq!(failure(&bad_gateway), ("speech service: cut".to_owned(), 502));

    let w = wired(FakeSpeech::marked(), Device::new(0, usize::MAX), Duration::from_secs(5));
    let (daemon, mcp) = up(&w).await;
    let no_device = mcp.call("speak", &json!({"text": "x"})).await;
    daemon.stop().await;
    assert_eq!(failure(&no_device), ("no output device".to_owned(), 503));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_full_queue_is_a_tool_error_with_429() {
    let (fake, gate) = long(300, 1.0).gated();
    let w = wired(fake, Device::new(0, 0), Duration::from_secs(5));
    let (daemon, mcp) = up(&w).await;
    let calls: Vec<_> = (0..17)
        .map(|_| {
            let mcp = mcp.clone();
            tokio::spawn(async move { mcp.call("speak", &json!({"text": "x"})).await })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    while w.state.speech.state() != (SpeechState::Speaking { id: 0 }) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let refused = mcp.call("speak", &json!({"text": "una más"})).await;
    assert_eq!(
        failure(&refused),
        ("speech queue is full: 16 requests already waiting".to_owned(), 429)
    );
    w.state.speech.stop().await.unwrap();
    gate.add_permits(100);
    for call in calls {
        call.await.unwrap();
    }
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listen_failures_are_tool_errors_with_the_message_and_the_http_status() {
    let w = wired(FakeSpeech::marked(), Device::new(0, 0), Duration::from_millis(150));
    let (daemon, mcp) = up(&w).await;
    for (seconds, shown) in [(0, "0"), (301, "301")] {
        let refused = mcp.call("listen", &json!({"timeout_seconds": seconds})).await;
        assert_eq!(
            failure(&refused),
            (
                format!("a listen timeout is above zero and at most 300 s, not {shown} s"),
                400
            )
        );
    }
    assert_eq!(w.mic.opened(), 0);
    let _silent = w.hearing.session();
    let timeout = mcp.call("listen", &json!({})).await;
    assert_eq!(
        failure(&timeout),
        ("nobody started speaking within 0.15 s".to_owned(), 408)
    );

    let feed = w.hearing.session();
    feed.fail(HearingError::Server("closed with 1011".to_owned()));
    let recognizer = mcp.call("listen", &json!({"timeout_seconds": 5})).await;
    assert_eq!(
        failure(&recognizer),
        ("speech recognizer: closed with 1011".to_owned(), 502)
    );

    let _busy_feed = w.hearing.session();
    let first = mcp.clone();
    let running = tokio::spawn(async move { first.call("listen", &json!({"timeout_seconds": 5})).await });
    assert!(w.mic.live().await);
    let busy = mcp.call("listen", &json!({})).await;
    assert_eq!(failure(&busy), ("someone is already listening".to_owned(), 409));
    running.abort();
    assert!(w.mic.freed().await);

    w.mic.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let no_mic = mcp.call("listen", &json!({})).await;
    daemon.stop().await;
    assert_eq!(failure(&no_mic), ("no input device".to_owned(), 503));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_failed_final_is_skipped_and_the_next_one_is_the_answer() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let feed = w.hearing.session();
    let asking = mcp.clone();
    let call = tokio::spawn(async move { asking.call("listen", &json!({"timeout_seconds": 10})).await });
    assert!(w.mic.live().await);
    feed.say(Recognized::Final(heard("perdido"), Some(NodeError::Timeout)));
    feed.say(Recognized::Final(heard("bien"), None));
    let result = call.await.unwrap();
    daemon.stop().await;
    assert_eq!(result["structuredContent"]["text"], "bien");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_voices_failure_is_a_tool_error_with_502() {
    let w = plain();
    *w.voices.answer.lock().unwrap() = Err(SpeechError::Connect("refused".to_owned()));
    let (daemon, mcp) = up(&w).await;
    let result = mcp.call("list_voices", &json!({})).await;
    daemon.stop().await;
    assert_eq!(
        failure(&result),
        ("cannot reach the speech service: refused".to_owned(), 502)
    );
    assert_eq!(w.voices.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_bad_arguments_are_a_tool_error_that_says_what_was_wrong_and_nothing_runs() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let cases = [
        ("speak", json!({}), "missing field `text`"),
        (
            "speak",
            json!({"text": 7}),
            "invalid type: integer `7`, expected a string",
        ),
        ("speak", json!({"text": "x", "voice": 3}), "expected a string"),
        (
            "listen",
            json!({"timeout_seconds": "x"}),
            "invalid type: string \"x\", expected u64",
        ),
        (
            "listen",
            json!({"timeout_seconds": -1}),
            "invalid value: integer `-1`, expected u64",
        ),
        (
            "listen",
            json!({"timeout_seconds": 1.5}),
            "invalid type: floating point `1.5`, expected u64",
        ),
    ];
    for (tool, arguments, why) in cases {
        let result = mcp.call(tool, &arguments).await;
        assert_eq!(result["isError"], true, "{tool} {arguments}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with("failed to deserialize parameters: ") && text.contains(why),
            "{tool} {arguments}: {text}"
        );
        assert!(result.get("structuredContent").is_none(), "{result}");
    }
    assert_eq!(w.state.speech.state(), SpeechState::Idle);
    assert_eq!(w.mic.opened(), 0);
    assert_eq!(w.voices.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_arguments_that_are_not_an_object_and_unknown_tools_and_methods_are_json_rpc_errors() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let not_object = mcp.call_with("speak", &json!("texto"), &[]).await;
    assert!(not_object.json().get("result").is_none(), "{}", not_object.text);
    assert!(not_object.error_code() < 0);
    let unknown = mcp.call_with("fly", &json!({}), &[]).await;
    assert_eq!(unknown.error_code(), -32602, "{}", unknown.text);
    let method = mcp
        .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "nope"}), &[])
        .await;
    assert_eq!(method.error_code(), -32601, "{}", method.text);
    let malformed = mcp.post_raw("{not json".to_owned(), &[]).await;
    assert!(
        matches!(malformed.status, 400..=499),
        "{} {}",
        malformed.status,
        malformed.text
    );
    assert_eq!(w.state.speech.state(), SpeechState::Idle);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_listen_whose_caller_goes_away_releases_the_microphone_and_closes_on_the_bus() {
    let w = plain();
    let mut bus = w.events.subscribe();
    let (daemon, mcp) = up(&w).await;
    let _feed = w.hearing.session();
    let asking = mcp.clone();
    let call = tokio::spawn(async move { asking.call("listen", &json!({"timeout_seconds": 60})).await });
    assert!(w.mic.live().await);
    call.abort();
    assert!(w.mic.freed().await, "the microphone stayed open after the client left");
    let seen = crate::fake::drained(&mut bus);
    let states: Vec<String> = seen.iter().map(|event| serde_json::to_string(event).unwrap()).collect();
    assert_eq!(
        states,
        [
            r#"{"type":"listening","state":"open"}"#,
            r#"{"type":"listening","state":"closed"}"#
        ],
        "{states:?}"
    );
    let again = w.hearing.session();
    again.say(Recognized::Final(heard("libre"), None));
    let next = mcp.call("listen", &json!({"timeout_seconds": 5})).await;
    assert_eq!(next["structuredContent"]["text"], "libre");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_post_over_64_kib_is_a_413_and_one_just_under_is_read() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let padded = |size: usize| {
        let head = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"stop","arguments":{},"pad":""#;
        let tail = r#""}}"#;
        format!("{head}{}{tail}", "a".repeat(size - head.len() - tail.len()))
    };
    let over = mcp.post_raw(padded(65_537), &[]).await;
    assert_eq!(over.status, 413, "{}", over.text);
    let exact = mcp.post_raw(padded(65_536), &[]).await;
    assert_eq!(exact.status, 200, "{}", exact.text);
    let under = mcp.post_raw(padded(60_000), &[]).await;
    assert_eq!(under.status, 200, "{}", under.text);
    let huge = mcp.try_post_raw(padded(5_000_000), &[]).await;
    assert!(huge.is_none_or(|answer| answer.status == 413), "a 5 MB body was taken");
    assert_eq!(w.state.speech.state(), SpeechState::Idle);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_foreign_host_header_is_refused_and_loopback_names_are_served() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let body = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    for host in [
        "evil.example",
        "evil.example:5700",
        "127.0.0.1.evil.example",
        "10.0.0.5:5700",
    ] {
        let answer = mcp.post(&body, &[("Host", host)]).await;
        assert_eq!(answer.status, 403, "{host}: {}", answer.text);
        assert_eq!(answer.text, "Forbidden: Host header is not allowed", "{host}");
    }
    for host in [
        "localhost",
        "localhost:5700",
        "127.0.0.1",
        "127.0.0.1:5700",
        "[::1]:5700",
    ] {
        let answer = mcp.post(&body, &[("Host", host)]).await;
        assert_eq!(answer.status, 200, "{host}: {}", answer.text);
    }
    assert_eq!(w.state.speech.state(), SpeechState::Idle);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_and_delete_on_mcp_are_not_a_session() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let client = reqwest::Client::new();
    let get = client
        .get(&mcp.url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert!(get.status().is_client_error(), "{}", get.status());
    let delete = client.delete(&mcp.url).send().await.unwrap();
    assert!(delete.status().is_client_error(), "{}", delete.status());
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_tool_list_carries_cache_hints_only_for_the_protocol_that_has_them() {
    let w = plain();
    let (daemon, mcp) = up(&w).await;
    let negotiated = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "probe", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    }}});
    let new = mcp
        .post(
            &negotiated,
            &[("MCP-Protocol-Version", "2026-07-28"), ("Mcp-Method", "tools/list")],
        )
        .await
        .result();
    let mut keys: Vec<&str> = new.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["cacheScope", "resultType", "tools", "ttlMs"]);
    assert_eq!(
        (&new["ttlMs"], &new["cacheScope"], &new["resultType"]),
        (&json!(0), &json!("public"), &json!("complete"))
    );
    assert_eq!(new["tools"].as_array().unwrap().len(), 5);

    let legacy = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let old = mcp
        .post(&legacy, &[("MCP-Protocol-Version", "2025-06-18")])
        .await
        .result();
    let keys: Vec<&str> = old.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["tools"]);
    assert_eq!(old["tools"], new["tools"], "the same tools whatever the protocol");
    daemon.stop().await;
}
