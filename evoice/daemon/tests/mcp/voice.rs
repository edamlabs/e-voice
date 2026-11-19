use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use e_voice_daemon::speech::base::SpeechRequest;
use serde_json::{Value, json};

use crate::client::Mcp;
use crate::fake::{Device, FakeSpeech, FakeVoices, wired_with};
use crate::running::Running;

const NEGOTIATED: [(&str, &str); 3] = [
    ("MCP-Protocol-Version", "2026-07-28"),
    ("Mcp-Method", "tools/call"),
    ("Mcp-Name", "speak"),
];

fn table() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("lens".to_owned(), "narrator".to_owned()),
        ("probe".to_owned(), "p".to_owned()),
    ])
}

async fn rig() -> (Running, Mcp, Arc<Mutex<Vec<SpeechRequest>>>) {
    let fake = FakeSpeech::marked();
    let seen = Arc::clone(&fake.requests);
    let w = wired_with(
        fake,
        Device::new(0, 0),
        Duration::from_secs(5),
        FakeVoices::default(),
        table(),
    );
    let daemon = Running::start(w.state.clone()).await;
    let mcp = Mcp::on(daemon.port);
    (daemon, mcp, seen)
}

fn last_voice(seen: &Arc<Mutex<Vec<SpeechRequest>>>) -> Option<String> {
    seen.lock().unwrap().last().unwrap().voice.clone()
}

/// A self-contained call that tells who the client is in its own metadata (the 2026-07-28 form).
fn declared(name: &str, arguments: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {
        "name": "speak", "arguments": arguments,
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": name, "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    }})
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_calls_own_voice_wins_over_the_clients_voice() {
    let (daemon, mcp, seen) = rig().await;
    mcp.call_with(
        "speak",
        &json!({"text": "1", "voice": "ef_dora"}),
        &[("X-Evoice-Client", "lens")],
    )
    .await
    .result();
    assert_eq!(last_voice(&seen).as_deref(), Some("ef_dora"));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_header_picks_the_voice_the_table_gives_that_client() {
    let (daemon, mcp, seen) = rig().await;
    mcp.call_with("speak", &json!({"text": "1"}), &[("X-Evoice-Client", "lens")])
        .await
        .result();
    assert_eq!(last_voice(&seen).as_deref(), Some("narrator"));
    mcp.call_with(
        "speak",
        &json!({"text": "2", "voice": null}),
        &[("x-evoice-client", "lens")],
    )
    .await
    .result();
    assert_eq!(last_voice(&seen).as_deref(), Some("narrator"));
    mcp.call_with("speak", &json!({"text": "3"}), &[("X-EVOICE-CLIENT", "probe")])
        .await
        .result();
    assert_eq!(last_voice(&seen).as_deref(), Some("p"));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_without_a_header_the_name_the_client_declares_picks_the_voice() {
    let (daemon, mcp, seen) = rig().await;
    mcp.post(&declared("probe", &json!({"text": "1"})), &NEGOTIATED)
        .await
        .result();
    assert_eq!(last_voice(&seen).as_deref(), Some("p"));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_an_unknown_caller_gets_no_voice_so_the_service_default() {
    let (daemon, mcp, seen) = rig().await;
    mcp.call_with("speak", &json!({"text": "1"}), &[("X-Evoice-Client", "stranger")])
        .await
        .result();
    assert_eq!(last_voice(&seen), None);
    mcp.call_with("speak", &json!({"text": "2"}), &[]).await.speak_ok();
    assert_eq!(last_voice(&seen), None);
    mcp.post(&declared("nobody", &json!({"text": "3"})), &NEGOTIATED)
        .await
        .result();
    assert_eq!(last_voice(&seen), None);
    mcp.call_with("speak", &json!({"text": "4"}), &[("X-Evoice-Client", "LENS")])
        .await
        .result();
    assert_eq!(last_voice(&seen), None, "the table is case-sensitive");
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_header_outranks_the_declared_name_even_when_the_header_is_unknown() {
    let (daemon, mcp, seen) = rig().await;
    let body = declared("probe", &json!({"text": "1"}));
    mcp.post(
        &body,
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "speak"),
            ("X-Evoice-Client", "lens"),
        ],
    )
    .await
    .result();
    assert_eq!(last_voice(&seen).as_deref(), Some("narrator"));
    mcp.post(
        &body,
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "speak"),
            ("X-Evoice-Client", "other"),
        ],
    )
    .await
    .result();
    assert_eq!(last_voice(&seen), None);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_voice_is_never_the_language_and_the_lang_is_never_set() {
    let (daemon, mcp, seen) = rig().await;
    mcp.call_with("speak", &json!({"text": "1", "lang": "en"}), &[])
        .await
        .result();
    let request = seen.lock().unwrap().last().unwrap().clone();
    assert_eq!((request.voice, request.lang.is_none()), (None, true));
    daemon.stop().await;
}

trait SpeakOk {
    fn speak_ok(self) -> serde_json::Value;
}

impl SpeakOk for crate::client::Answer {
    /// The result of a `speak` that must have been spoken: a tool error here would hide behind an empty log.
    fn speak_ok(self) -> serde_json::Value {
        let result = self.result();
        assert_eq!(result["isError"], false, "the speak failed: {result}");
        result
    }
}
