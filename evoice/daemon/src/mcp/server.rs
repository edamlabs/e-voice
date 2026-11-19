use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::http::request::Parts;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CacheScope, CallToolResult, ContentBlock, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::speech::base::{SpeechRequest, Voices};
use crate::speech::table::{CLIENT, VoiceTable};
use crate::talk::approval::{APPROVER, Approvals};
use crate::talk::runner::Talk;
use crate::workflow::listen::Listen;
use crate::workflow::queue::SpeechQueue;

/// The largest request `/mcp` takes: 64 KiB. A text is at most 4096 characters, about 24 KiB with every
/// character JSON-escaped; past the cap rmcp answers 413 while the body is still arriving.
const BODY: usize = 65_536;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SpeakParams {
    /// What to say, at most 4096 characters.
    text: String,
    /// A voice id from `list_voices`; when omitted, the voice configured for this client's
    /// `X-Evoice-Client`, else the speech service's default.
    voice: Option<String>,
}

/// What Claude Code sends its permission prompt tool: the tool it asks about and that tool's input.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApproveParams {
    /// The tool asked about.
    tool_name: String,
    /// That tool's input.
    input: Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListenParams {
    /// Seconds to wait for someone to start speaking, 1 to 300; the daemon's default when omitted.
    timeout_seconds: Option<u64>,
}

/// The daemon's MCP door: five tools on the very handles the HTTP doors hold, so ids, the queue's and the
/// listener's limits and `/events` are shared by both. Stateless: nothing is kept between calls, so there
/// is no session to bound; what is in flight is bounded by the queue and the listener themselves.
#[derive(Debug, Clone)]
pub struct McpServer {
    speech: SpeechQueue,
    listen: Arc<dyn Listen>,
    voices: Arc<dyn Voices>,
    table: VoiceTable,
    approvals: Approvals,
    talk: Arc<dyn Talk>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    // ##### PRIVATE #####

    /// Who is calling, as the client says of itself: its `X-Evoice-Client` header, which every client can
    /// send; else the name it declares, which counts only when the request itself carries it (protocol
    /// 2026-07-28 and later) — stateless, an older client's `initialize` is never seen again. Unauthenticated:
    /// a preference, never a permission — read for the voice alone, nowhere else.
    fn speak_caller(context: &RequestContext<RoleServer>) -> Option<String> {
        context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.headers.get(CLIENT)?.to_str().ok().map(str::to_owned))
            .or_else(|| context.client_info().map(|info| info.name))
    }

    /// A failure of ours, readable by the model: the message and the HTTP status the same failure has on
    /// the HTTP doors.
    fn common_failure(error: &impl Display, status: StatusCode) -> CallToolResult {
        CallToolResult::structured_error(json!({"error": error.to_string(), "status": status.as_u16()}))
    }

    /// Every outcome of ours serializes; a value that would not is a bug, reported as one.
    fn common_success(value: Result<Value, serde_json::Error>) -> CallToolResult {
        value.map_or_else(
            |error| Self::common_failure(&error, StatusCode::INTERNAL_SERVER_ERROR),
            CallToolResult::structured,
        )
    }

    // ##########################################################

    // ##### PUBLIC #####

    #[must_use]
    pub fn new(
        speech: SpeechQueue,
        listen: Arc<dyn Listen>,
        voices: Arc<dyn Voices>,
        table: VoiceTable,
        approvals: Approvals,
        talk: Arc<dyn Talk>,
    ) -> Self {
        Self {
            speech,
            listen,
            voices,
            table,
            approvals,
            talk,
            tool_router: Self::tool_router(),
        }
    }

    /// The streamable-HTTP service to nest at `/mcp`: stateless, plain JSON answers, the body capped at
    /// [`BODY`], `Host` held to loopback (rmcp's default, against DNS rebinding) even when `[daemon] host`
    /// is widened.
    #[must_use]
    pub fn service(self) -> StreamableHttpService<Self, LocalSessionManager> {
        let config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .with_max_request_body_bytes(BODY);
        StreamableHttpService::new(move || Ok(self.clone()), Arc::default(), config)
    }
}

#[tool_router]
impl McpServer {
    /// Say a text aloud, in turn with every other, and answer once it has played: {id, outcome}.
    #[tool]
    async fn speak(
        &self,
        Parameters(params): Parameters<SpeakParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let caller = Self::speak_caller(&context);
        let voice = self.table.voice(params.voice, caller.as_deref());
        let request = SpeechRequest {
            text: params.text,
            voice,
            lang: None,
        };
        match self.speech.say(request).await {
            Ok((id, outcome)) => Self::common_success(Ok(json!({"id": id, "outcome": outcome}))),
            Err(error) => Self::common_failure(&error, error.status()),
        }
    }

    /// Silence what is being said and drop every text waiting: {stopped: id or null, dropped}. A voice
    /// conversation that is thinking, speaking or asking also drops the agent's turn and listens again.
    #[tool]
    async fn stop(&self) -> CallToolResult {
        match self.talk.stop().await {
            Ok(halted) => Self::common_success(serde_json::to_value(halted)),
            Err(error) => Self::common_failure(&error, error.status()),
        }
    }

    /// Listen until someone speaks and answer with what they said: {text, lang, emotion}.
    #[tool]
    async fn listen(
        &self,
        Parameters(params): Parameters<ListenParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let timeout = params.timeout_seconds.map(Duration::from_secs);
        tokio::select! {
            heard = self.listen.listen(timeout) => match heard {
                Ok(heard) => Self::common_success(serde_json::to_value(heard)),
                Err(error) => Self::common_failure(&error, error.status()),
            },
            () = context.ct.cancelled() => Self::common_failure(&"the call was cancelled", StatusCode::REQUEST_TIMEOUT),
        }
    }

    /// The voices `speak` accepts: {voices: [id, …]}.
    #[tool]
    async fn list_voices(&self) -> CallToolResult {
        match self.voices.list().await {
            Ok(voices) => Self::common_success(Ok(json!({"voices": voices}))),
            Err(error) => Self::common_failure(&error, StatusCode::BAD_GATEWAY),
        }
    }

    /// Claude Code's permission prompt for a voice conversation: the listener is asked aloud, and the answer
    /// is Claude Code's own text block. Answers the conversation's own agent alone; anyone else is refused.
    #[tool]
    async fn approve(
        &self,
        Parameters(ApproveParams { tool_name, input }): Parameters<ApproveParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let token = context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.headers.get(APPROVER)?.to_str().ok().map(str::to_owned));
        let verdict = self.approvals.ask(token.as_deref(), &tool_name, &input).await;
        CallToolResult::success(vec![ContentBlock::text(verdict.behavior())])
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    /// Written here, so `#[tool_handler]` (which writes only what the impl lacks) does not: its own is an
    /// `async fn` with nothing to await, which clippy refuses. Same answer, same cache hints: a client of
    /// protocol 2026-07-28 or later may keep the list, shared, revalidated at once.
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + MaybeSendFuture + '_ {
        let hinted = context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        let listed = ListToolsResult::with_all_items(self.tool_router.list_all());
        let listed = if hinted {
            listed.with_ttl_ms(0).with_cache_scope(CacheScope::Public)
        } else {
            listed
        };
        std::future::ready(Ok(listed))
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("evoiced", env!("E_VOICE_VERSION")))
            .with_instructions("The computer's voice: speak aloud, stop, listen to the user, list voices.")
    }
}
