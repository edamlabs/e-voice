use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

use crate::client::Mcp;
use crate::fake::{Device, FakeSpeech, FakeVoices, wired_with};
use crate::running::Running;

fn arguments(value: &Value) -> rmcp::model::JsonObject {
    value.as_object().unwrap().clone()
}

async fn rig() -> (
    Running,
    std::sync::Arc<std::sync::Mutex<Vec<e_voice_daemon::speech::base::SpeechRequest>>>,
) {
    let fake = FakeSpeech::marked();
    let seen = std::sync::Arc::clone(&fake.requests);
    let table = std::collections::BTreeMap::from([("probe".to_owned(), "p".to_owned())]);
    let w = wired_with(
        fake,
        Device::new(0, 0),
        Duration::from_secs(5),
        FakeVoices::default(),
        table,
    );
    (Running::start(w.state.clone()).await, seen)
}

async fn session(
    daemon: &Running,
    version: ProtocolVersion,
    name: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, ClientConfig> {
    let uri = format!("http://127.0.0.1:{}/mcp", daemon.port);
    let transport = StreamableHttpClientTransport::from_config(StreamableHttpClientTransportConfig::with_uri(uri));
    let info =
        ClientConfig::new(ClientCapabilities::default(), Implementation::new(name, "1")).with_protocol_version(version);
    tokio::time::timeout(Duration::from_secs(10), info.serve(transport))
        .await
        .expect("initialize hung")
        .expect("initialize failed")
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_client_on_each_older_protocol_version_initializes_lists_and_calls_speak() {
    for version in [
        ProtocolVersion::V_2024_11_05,
        ProtocolVersion::V_2025_03_26,
        ProtocolVersion::V_2025_06_18,
        ProtocolVersion::V_2025_11_25,
    ] {
        let (daemon, _) = rig().await;
        let client = session(&daemon, version.clone(), "probe").await;
        let tools = client.list_all_tools().await.unwrap();
        let mut names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
        names.sort();
        assert_eq!(
            names,
            ["approve", "list_voices", "listen", "speak", "stop"],
            "{version:?}"
        );
        let result = client
            .call_tool(CallToolRequestParams::new("speak").with_arguments(arguments(&json!({"text": "4"}))))
            .await
            .unwrap_or_else(|error| panic!("{version:?}: {error}"));
        assert_eq!(result.is_error, Some(false), "{version:?}");
        assert_eq!(
            result.structured_content,
            Some(json!({"id": 0, "outcome": "done"})),
            "{version:?}"
        );
        client.cancel().await.ok();
        daemon.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_raw_initialize_then_call_on_each_older_version_without_a_session() {
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let (daemon, _) = rig().await;
        let mcp = Mcp::on(daemon.port);
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "probe", "version": "1"}}});
        let answer = mcp.post(&init, &[]).await;
        assert_eq!(answer.status, 200, "{version}: {}", answer.text);
        assert!(
            answer.json()["result"]["protocolVersion"].is_string(),
            "{version}: {}",
            answer.text
        );
        let call = mcp
            .call_with("speak", &json!({"text": "4"}), &[("MCP-Protocol-Version", version)])
            .await;
        assert_eq!(call.status, 200, "{version}: {}", call.text);
        assert_eq!(
            call.result()["structuredContent"],
            json!({"id": 0, "outcome": "done"}),
            "{version}"
        );
        daemon.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_no_session_id_is_ever_issued_or_required() {
    let (daemon, _) = rig().await;
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/mcp", daemon.port);
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "probe", "version": "1"}}});
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init)
        .send()
        .await
        .unwrap();
    assert!(
        response.headers().get("mcp-session-id").is_none(),
        "{:?}",
        response.headers()
    );
    let stale = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", "00000000-0000-0000-0000-000000000000")
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    println!("stale session id -> {}", stale.status());
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_legacy_client_is_told_apart_by_the_header_alone_never_by_the_name_it_declared() {
    let versions = [
        ProtocolVersion::V_2024_11_05,
        ProtocolVersion::V_2025_03_26,
        ProtocolVersion::V_2025_06_18,
        ProtocolVersion::V_2025_11_25,
    ];
    for version in versions {
        let (daemon, seen) = rig().await;
        let client = session(&daemon, version.clone(), "probe").await;
        let speak = || CallToolRequestParams::new("speak").with_arguments(arguments(&json!({"text": "4"})));
        client.call_tool(speak()).await.unwrap();
        let declared = seen.lock().unwrap().last().unwrap().voice.clone();
        client.cancel().await.ok();
        assert_eq!(
            declared, None,
            "{version:?}: no header, no table voice, whatever the initialize said"
        );

        let mcp = Mcp::on(daemon.port);
        let named = mcp
            .call_with(
                "speak",
                &json!({"text": "5"}),
                &[("X-Evoice-Client", "probe"), ("MCP-Protocol-Version", version.as_str())],
            )
            .await;
        assert_eq!(named.result()["isError"], false, "{version:?}");
        let voice = seen.lock().unwrap().last().unwrap().voice.clone();
        assert_eq!(
            voice.as_deref(),
            Some("p"),
            "{version:?}: the header names the registration"
        );
        daemon.stop().await;
    }
}
