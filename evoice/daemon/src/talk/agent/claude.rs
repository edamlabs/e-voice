use std::ffi::OsString;
use std::fmt::{self, Debug};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;

use crate::config::talk::ClaudeConfig;
use crate::talk::agent::base::{Agent, AgentError, AgentEvent, Ended, Turn};

/// The daemon's MCP server as the session knows it: `approve` is `mcp__evoice__approve`.
const SERVER: &str = "evoice";

/// The tool Claude Code asks before anything that needs a yes: the daemon's, which asks aloud.
const PROMPT_TOOL: &str = "mcp__evoice__approve";

/// The daemon's tools the model must not call in a voice session: it is already being spoken to.
const DENIED: [&str; 4] = [
    "mcp__evoice__speak",
    "mcp__evoice__stop",
    "mcp__evoice__listen",
    "mcp__evoice__list_voices",
];

/// Built-in tools that only read: they run unasked. Every other tool of the session is asked.
const READS: [&str; 4] = ["Read", "Glob", "Grep", "LSP"];

/// The permission mode `manual` reports in `system/init`.
const MANUAL: &str = "default";

/// The environment the child inherits, by exact name: what `claude` needs to run, find its login and reach
/// the network. Nothing else passes, so a daemon started inside a Claude Code session never hands its child
/// that session's identity (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, its messaging socket and token…).
const PASSED: &[&str] = &[
    "HOME",
    "PATH",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLOUD_ML_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// The environment the child inherits, by prefix: locale, the API's own keys, a cloud provider's.
const PREFIXES: [&str; 3] = ["LC_", "ANTHROPIC_", "AWS_"];

/// The longest line read from the child: tool results travel on stdout too. A longer line is skipped.
const LINE: usize = 8 * 1024 * 1024;

/// The longest the child may take to send its first line, connecting to the daemon's MCP server
/// included (Claude Code waits up to 30 s for it).
const START: Duration = Duration::from_secs(60);

/// The longest a cancelled turn may take to end; past it the session is killed and the next turn resumes
/// it from its transcript.
const CANCEL: Duration = Duration::from_secs(5);

/// The longest a session asked to stop may take before it is killed.
const GRACE: Duration = Duration::from_secs(5);

/// One line of the child's stream-json output, read as far as the driver needs.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeLine {
    System {
        subtype: String,
        #[serde(default, rename = "permissionMode")]
        permission_mode: Option<String>,
        #[serde(default)]
        mcp_servers: Vec<McpServerStatus>,
    },
    StreamEvent {
        event: Value,
        parent_tool_use_id: Option<String>,
    },
    Result {
        subtype: String,
        #[serde(default)]
        terminal_reason: Option<String>,
        #[serde(default)]
        result: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct McpServerStatus {
    name: String,
    status: String,
}

/// The running `claude`: its own process group, its pipes, and whether its `system/init` was checked.
struct ClaudeSession {
    child: Child,
    group: Option<Pid>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    pending: Vec<u8>,
    skipping: bool,
    verified: bool,
    stderr: JoinHandle<()>,
}

impl Debug for ClaudeSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeSession")
            .field("verified", &self.verified)
            .finish_non_exhaustive()
    }
}

impl ClaudeSession {
    /// The next whole line, `None` once the child closed its output. A line over [`LINE`] is skipped whole.
    /// Cancel-safe: what was read of a line stays in `pending`.
    async fn line(&mut self) -> Result<Option<Vec<u8>>, AgentError> {
        loop {
            if self.pending.len() >= LINE {
                tracing::warn!(cap = LINE, "claude.line.skipped");
                self.skipping = true;
                self.pending.clear();
            }
            let room = u64::try_from(LINE.saturating_sub(self.pending.len())).unwrap_or(u64::MAX);
            let read = (&mut self.stdout)
                .take(room)
                .read_until(b'\n', &mut self.pending)
                .await
                .map_err(|error| AgentError::Protocol(error.to_string()))?;
            let whole = self.pending.ends_with(b"\n");
            match (read, whole, self.skipping) {
                (0, false, _) => return Ok(None),
                (_, true, false) => return Ok(Some(std::mem::take(&mut self.pending))),
                (_, true, true) => {
                    self.skipping = false;
                    self.pending.clear();
                }
                (_, false, _) => {}
            }
        }
    }

    /// Asks the turn under way to stop; its `result` follows.
    async fn interrupt(&mut self) -> Result<(), AgentError> {
        let id = uuid::Uuid::new_v4().to_string();
        let interrupt = json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}});
        self.write(&interrupt).await
    }

    /// Ends a turn nobody reads anymore: interrupts it and reads to its `result`, so none of its reply
    /// reaches the next turn. False when it does not end within [`CANCEL`].
    async fn settle(&mut self) -> bool {
        if self.interrupt().await.is_err() {
            return false;
        }
        let drained = tokio::time::timeout(CANCEL, async {
            while let Ok(Some(line)) = self.line().await {
                if matches!(
                    serde_json::from_slice::<ClaudeLine>(&line),
                    Ok(ClaudeLine::Result { .. })
                ) {
                    return true;
                }
            }
            false
        });
        drained.await.unwrap_or(false)
    }

    async fn write(&mut self, message: &Value) -> Result<(), AgentError> {
        let line = format!("{message}\n");
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|_| AgentError::Gone)?;
        self.stdin.flush().await.map_err(|_| AgentError::Gone)
    }

    /// Asks the session's group to end, waits up to [`GRACE`], then kills it. Once `claude` is reaped its
    /// group id may be reused, so it is not signalled again.
    async fn close(mut self) {
        if let Some(group) = self.group {
            kill_process_group(group, Signal::TERM).ok();
        }
        let waited = tokio::time::timeout(GRACE, self.child.wait()).await;
        if matches!(waited, Ok(Ok(_))) {
            self.group = None;
        }
    }
}

impl Drop for ClaudeSession {
    /// Whatever ends the session, its whole process group goes with it: `claude` and what it started (tool
    /// commands, MCP servers). Never a group of 1 or 0, which would reach every process allowed.
    fn drop(&mut self) {
        if let Some(group) = self.group {
            kill_process_group(group, Signal::KILL).ok();
        }
        self.stderr.abort();
    }
}

/// The session behind the agent: none until the first turn, and none again after it was killed. `owing`
/// marks a turn dropped before its `result`: the rest of its reply is still on its way, and must not reach
/// the next turn.
#[derive(Debug, Default)]
struct ClaudeState {
    session: Option<ClaudeSession>,
    started: bool,
    owing: bool,
}

/// Claude Code as the agent: one headless `claude` per conversation, its turns fed through stdin as
/// stream-json. The session never loads the user's settings, runs in manual mode with the daemon's
/// `approve` as its only way to a yes, sees only the configured tools and the daemon's MCP server, asks
/// for every tool that changes something, and starts with a scrubbed environment. Its startup is checked
/// in `system/init` and refused when it is not what was asked. The session transcript is Claude Code's:
/// the driver never reads it.
pub struct ClaudeAgent {
    config: ClaudeConfig,
    mcp: String,
    prompt: String,
    id: String,
    state: Arc<Mutex<ClaudeState>>,
}

impl Debug for ClaudeAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeAgent")
            .field("bin", &self.config.bin)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl ClaudeAgent {
    // ##### PRIVATE #####

    /// The child's arguments. Variadic flags take their values as separate arguments and nothing
    /// positional is ever passed (the turns go through stdin), so no value can be swallowed. The session
    /// id is chosen here, so a killed session is resumed by it.
    fn turn_args(&self, resume: bool) -> Vec<String> {
        let ask: Vec<&str> = self
            .config
            .tools
            .iter()
            .map(String::as_str)
            .filter(|tool| !READS.contains(tool))
            .chain(["mcp__*"])
            .collect();
        let settings = json!({"permissions": {"ask": ask, "deny": DENIED}});
        let mcp = json!({"mcpServers": {SERVER: {"type": "http", "url": self.mcp}}});
        let session = if resume { "--resume" } else { "--session-id" };
        let fixed = [
            "-p",
            "--input-format=stream-json",
            "--output-format=stream-json",
            "--verbose",
            "--include-partial-messages",
            "--setting-sources=project",
            "--permission-mode=manual",
            "--permission-prompts=host",
            "--strict-mcp-config",
        ];
        let model = self.config.model.iter().map(|model| format!("--model={model}"));
        fixed
            .into_iter()
            .map(str::to_owned)
            .chain([format!("--permission-prompt-tool={PROMPT_TOOL}")])
            .chain([format!("--append-system-prompt={}", self.prompt)])
            .chain([format!("--settings={settings}"), format!("--mcp-config={mcp}")])
            .chain([format!("{session}={}", self.id)])
            .chain(model)
            .chain(["--tools".to_owned()])
            .chain(self.config.tools.iter().cloned())
            .collect()
    }

    /// What the child inherits: only the allowed names and prefixes of the daemon's own environment.
    fn turn_env() -> Vec<(OsString, OsString)> {
        std::env::vars_os()
            .filter(|(name, _)| {
                name.to_str().is_some_and(|name| {
                    PASSED.contains(&name) || PREFIXES.iter().any(|prefix| name.starts_with(*prefix))
                })
            })
            .collect()
    }

    /// Starts `claude` in its own process group, with nothing inherited but the allowed environment.
    fn turn_spawn(&self, resume: bool) -> Result<ClaudeSession, AgentError> {
        let mut command = Command::new(&self.config.bin);
        command
            .args(self.turn_args(resume))
            .env_clear()
            .envs(Self::turn_env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(cwd) = &self.config.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|error| AgentError::Start(error.to_string()))?;
        let missing = || AgentError::Start("a pipe was not opened".to_owned());
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        let stderr = child.stderr.take().ok_or_else(missing)?;
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id > 1)
            .and_then(Pid::from_raw);
        Ok(ClaudeSession {
            child,
            group,
            stdin,
            stdout: BufReader::new(stdout),
            pending: Vec::new(),
            skipping: false,
            verified: false,
            stderr: tokio::spawn(Self::turn_stderr(stderr)),
        })
    }

    /// The child's stderr, logged chunk by chunk, so its pipe never fills and no line is ever buffered whole.
    async fn turn_stderr(mut stderr: ChildStderr) {
        let mut chunk = vec![0u8; 4_096];
        while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
            let text = String::from_utf8_lossy(chunk.get(..read).unwrap_or_default());
            tracing::debug!(%text, "claude.stderr");
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// `mcp` is the daemon's MCP endpoint (`http://127.0.0.1:<port>/mcp`); `prompt` is appended to the
    /// agent's system prompt (the voice prompt, in the conversation's language).
    #[must_use]
    pub fn new(config: ClaudeConfig, mcp: String, prompt: String) -> Self {
        Self {
            config,
            mcp,
            prompt,
            id: uuid::Uuid::new_v4().to_string(),
            state: Arc::new(Mutex::new(ClaudeState::default())),
        }
    }

    /// Ends the session, letting it close for a moment before it is killed.
    pub async fn close(&self) {
        let session = self.state.lock().await.session.take();
        if let Some(session) = session {
            session.close().await;
        }
    }
}

impl Agent for ClaudeAgent {
    type Turn = ClaudeTurn;

    /// The first turn starts the session, and a turn after a killed one resumes it by its id. A turn left
    /// unfinished is settled first; one that cannot be, or was not checked at startup yet, ends the session.
    async fn turn(&self, text: String) -> Result<ClaudeTurn, AgentError> {
        let mut state = Arc::clone(&self.state).lock_owned().await;
        if std::mem::take(&mut state.owing) {
            let settled = match state.session.as_mut() {
                Some(session) if session.verified => session.settle().await,
                Some(_) | None => false,
            };
            if !settled {
                state.session = None;
            }
        }
        if state.session.is_none() {
            let resume = state.started;
            state.session = Some(self.turn_spawn(resume)?);
            state.started = true;
        }
        let message = json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null});
        let session = state.session.as_mut().ok_or(AgentError::Gone)?;
        if let Err(error) = session.write(&message).await {
            state.session = None;
            return Err(error);
        }
        Ok(ClaudeTurn { state, done: false })
    }
}

/// One turn of the session, holding it until the turn is dropped.
pub struct ClaudeTurn {
    state: OwnedMutexGuard<ClaudeState>,
    done: bool,
}

impl Debug for ClaudeTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeTurn")
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl Drop for ClaudeTurn {
    /// A turn dropped before its `result` (a barge-in in a `select!`, an error path) leaves its reply on
    /// the way; the next turn settles it first.
    fn drop(&mut self) {
        if !self.done {
            self.state.owing = true;
        }
    }
}

impl ClaudeTurn {
    // ##### PRIVATE #####

    /// What one line means for the turn: an event to pass on, an error that ends it, or nothing.
    fn next_line(&mut self, line: &[u8]) -> Option<Result<AgentEvent, AgentError>> {
        let parsed = serde_json::from_slice::<ClaudeLine>(line);
        match parsed {
            Ok(ClaudeLine::System {
                subtype,
                permission_mode,
                mcp_servers,
            }) if subtype == "init" => self.next_init(permission_mode.as_deref(), &mcp_servers),
            Ok(ClaudeLine::StreamEvent {
                event,
                parent_tool_use_id: None,
            }) => self.next_delta(&event),
            Ok(ClaudeLine::Result {
                subtype,
                terminal_reason,
                result,
            }) => Some(self.next_result(&subtype, terminal_reason.as_deref(), result)),
            Ok(_) => None,
            Err(error) => {
                tracing::debug!(%error, "claude.line.unread");
                None
            }
        }
    }

    /// The session's startup, checked: the mode asked for, and the daemon's server as the only one,
    /// connected. Anything else ends the session before a word of it is said.
    fn next_init(&mut self, mode: Option<&str>, servers: &[McpServerStatus]) -> Option<Result<AgentEvent, AgentError>> {
        let ours = matches!(servers, [server] if server.name == SERVER && server.status == "connected");
        let refused = match (mode == Some(MANUAL), ours) {
            (true, true) => None,
            (false, _) => Some(format!("permission mode {mode:?}, not manual")),
            (true, false) => Some("MCP servers are not exactly the daemon's, connected".to_owned()),
        };
        let Some(reason) = refused else {
            if let Some(session) = self.state.session.as_mut() {
                session.verified = true;
            }
            return None;
        };
        self.state.session = None;
        self.done = true;
        Some(Err(AgentError::Refused(reason)))
    }

    /// The session ends with the turn: it is dropped, its process group killed, and the next turn starts
    /// it again, resumed.
    fn next_gone(&mut self, error: AgentError) -> AgentError {
        self.state.session = None;
        self.done = true;
        error
    }

    /// A main-thread text delta; text before the startup was checked ends the session.
    fn next_delta(&mut self, event: &Value) -> Option<Result<AgentEvent, AgentError>> {
        let kind = event.pointer("/delta/type").and_then(Value::as_str);
        let text = event
            .pointer("/delta/text")
            .and_then(Value::as_str)
            .filter(|_| kind == Some("text_delta"))?;
        let verified = self.state.session.as_ref().is_some_and(|session| session.verified);
        if verified {
            return Some(Ok(AgentEvent::Delta(text.to_owned())));
        }
        let early = AgentError::Refused("text before the startup was checked".to_owned());
        Some(Err(self.next_gone(early)))
    }

    /// The turn's `result`: answered, cancelled (an interrupt's `aborted_streaming`), or failed.
    fn next_result(
        &mut self,
        subtype: &str,
        reason: Option<&str>,
        result: Option<String>,
    ) -> Result<AgentEvent, AgentError> {
        self.done = true;
        match (subtype, reason) {
            ("success", _) => Ok(AgentEvent::Done(Ended::Answered)),
            ("error_during_execution", Some("aborted_streaming")) => Ok(AgentEvent::Done(Ended::Cancelled)),
            (failure, _) => Err(AgentError::Agent(format!("{failure}: {}", result.unwrap_or_default()))),
        }
    }
}

impl Turn for ClaudeTurn {
    /// Reads the session until something for the turn arrives. The child closing its output ends the
    /// session; the next turn resumes it. The first line must come within [`START`].
    async fn next(&mut self) -> Option<Result<AgentEvent, AgentError>> {
        while !self.done {
            let verified = self.state.session.as_ref()?.verified;
            let session = self.state.session.as_mut()?;
            let late = || Err(AgentError::Start("no output within the start bound".to_owned()));
            let read = if verified {
                session.line().await
            } else {
                tokio::time::timeout(START, session.line())
                    .await
                    .unwrap_or_else(|_| late())
            };
            let line = match read {
                Ok(Some(line)) => line,
                Ok(None) => return Some(Err(self.next_gone(AgentError::Gone))),
                Err(error) => return Some(Err(self.next_gone(error))),
            };
            if let Some(event) = self.next_line(&line) {
                return Some(event);
            }
        }
        None
    }

    /// Sends the interrupt and reads to the turn's `result`, dropping what comes before it. A session
    /// that does not end the turn within [`CANCEL`] is killed; the next turn resumes it.
    async fn cancel(&mut self) -> Result<(), AgentError> {
        if self.done {
            return Ok(());
        }
        let session = self.state.session.as_mut().ok_or(AgentError::Gone)?;
        session.interrupt().await?;
        let drained = tokio::time::timeout(CANCEL, async {
            while let Some(event) = self.next().await {
                if matches!(event, Ok(AgentEvent::Done(_)) | Err(_)) {
                    break;
                }
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!("claude.cancel.killed");
            self.state.session = None;
        }
        self.done = true;
        Ok(())
    }
}
