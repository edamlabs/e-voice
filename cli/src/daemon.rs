use std::time::Duration;

use clap::Args;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// The longest a connect to the daemon may take: it is local, so past this it is not running.
const CONNECT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("cannot reach the daemon at {url}: {source}")]
    Connect { url: String, source: reqwest::Error },
    #[error("daemon answered {status}: {message}")]
    Refused { status: u16, message: String },
    #[error("unreadable answer from the daemon: {0}")]
    Answer(String),
}

/// `POST /speak`'s answer.
#[derive(Debug, Deserialize)]
struct Spoken {
    id: u64,
    outcome: String,
}

/// `POST /stop`'s answer.
#[derive(Debug, Deserialize)]
struct Halted {
    stopped: Option<u64>,
    dropped: usize,
}

/// The daemon's HTTP door, shared by every verb that talks to it.
#[derive(Debug, Args)]
pub struct DaemonClient {
    /// Daemon base URL.
    #[arg(long, env = "ECLI_DAEMON_URL", default_value = "http://127.0.0.1:5700")]
    url: String,
}

impl DaemonClient {
    /// POSTs `body` to `path` and reads the answer as `T`. A refusal carries the daemon's own `error`, or
    /// the raw body when it is not the daemon's shape (axum's own rejections). No request timeout: a say
    /// lasts as long as its speech, and the daemon bounds a listen itself.
    async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, DaemonError> {
        let connect = |source| DaemonError::Connect {
            url: self.url.clone(),
            source,
        };
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT)
            .build()
            .map_err(connect)?;
        let response = http
            .post(format!("{}{path}", self.url.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(connect)?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| DaemonError::Answer(error.to_string()))?;
        match status.as_u16() {
            200..=299 => serde_json::from_str(&text).map_err(|_| DaemonError::Answer(text)),
            code => {
                let error = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|answer| answer.get("error")?.as_str().map(str::to_owned));
                Err(DaemonError::Refused {
                    status: code,
                    message: error.unwrap_or(text),
                })
            }
        }
    }
}

#[derive(Debug, Args)]
pub struct SayArgs {
    /// Text to speak.
    text: String,
    /// Voice; the speech service's default when omitted.
    #[arg(long)]
    voice: Option<String>,
    /// Language (es, en); the speech service's default when omitted.
    #[arg(long)]
    lang: Option<String>,
    #[command(flatten)]
    daemon: DaemonClient,
}

impl SayArgs {
    /// Prints `done <id>` once the text has played, `cancelled <id>` once a stop cut it short: both
    /// succeed, a stop from another client is not this one's failure.
    ///
    /// # Errors
    /// The daemon is unreachable, refused the request, or answered in a shape it does not have.
    pub async fn run(self) -> Result<(), DaemonError> {
        let body = json!({"text": self.text, "voice": self.voice, "lang": self.lang});
        let spoken: Spoken = self.daemon.post("/speak", &body).await?;
        println!("{} {}", spoken.outcome, spoken.id);
        Ok(())
    }
}

#[derive(Debug, Args)]
pub struct StopArgs {
    #[command(flatten)]
    daemon: DaemonClient,
}

impl StopArgs {
    /// Prints `stopped <id>, dropped <n>`, or `nothing playing` when the daemon was idle.
    ///
    /// # Errors
    /// The daemon is unreachable, refused the request, or answered in a shape it does not have.
    pub async fn run(self) -> Result<(), DaemonError> {
        let halted: Halted = self.daemon.post("/stop", &json!({})).await?;
        match halted.stopped {
            Some(id) => println!("stopped {id}, dropped {}", halted.dropped),
            None => println!("nothing playing"),
        }
        Ok(())
    }
}

#[derive(Debug, Args)]
pub struct ListenArgs {
    /// The heard segment as one JSON line: text, lang and emotion.
    #[arg(long = "struct")]
    structured: bool,
    /// How long to wait for someone to start speaking (e.g. 8s); the daemon's default when omitted.
    #[arg(long)]
    timeout: Option<String>,
    #[command(flatten)]
    daemon: DaemonClient,
}

impl ListenArgs {
    /// Prints the text alone, or with `--struct` the daemon's whole answer on one line.
    ///
    /// # Errors
    /// The daemon is unreachable, refused the request, or answered in a shape it does not have.
    pub async fn run(self) -> Result<(), DaemonError> {
        let body = json!({"timeout": self.timeout});
        let heard: Value = self.daemon.post("/listen", &body).await?;
        let text = heard
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| DaemonError::Answer(heard.to_string()))?;
        let structured = self.structured;
        let line: String = if structured { heard.to_string() } else { text.into() };
        println!("{line}");
        Ok(())
    }
}
