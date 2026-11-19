use std::io::BufRead;
use std::thread;

use clap::{Args, Subcommand, ValueEnum};
use futures_util::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::daemon::{DaemonClient, DaemonError};

/// The languages the daemon has a reading script in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceLang {
    Es,
    En,
}

impl VoiceLang {
    const fn code(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }
}

/// Where a recording stands, as the daemon's `recording` events say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum VoiceState {
    Started,
    Heard,
    Finished,
    Learned,
    Failed,
}

/// The daemon's events, read as far as a recording needs.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum VoiceEvent {
    Recording {
        state: VoiceState,
    },
    #[serde(other)]
    Other,
}

/// What `ecli voice add` says on its own, in the script's language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoicePrompt {
    Start,
    Recording,
    Heard,
    Sending,
}

impl VoicePrompt {
    /// The phrase, in `lang`. The match is over every pair, so a language without a phrase does not build.
    const fn text(self, lang: VoiceLang) -> &'static str {
        match (self, lang) {
            (Self::Start, VoiceLang::Es) => {
                "Enter para empezar a leer; Enter otra vez para terminar, Ctrl-C para cancelar."
            }
            (Self::Start, VoiceLang::En) => "Enter to start reading; Enter again to finish, Ctrl-C to cancel.",
            (Self::Recording, VoiceLang::Es) => "grabando…",
            (Self::Recording, VoiceLang::En) => "recording…",
            (Self::Heard, VoiceLang::Es) => "te oigo…",
            (Self::Heard, VoiceLang::En) => "hearing you…",
            (Self::Sending, VoiceLang::Es) => "aprendiendo la voz…",
            (Self::Sending, VoiceLang::En) => "learning the voice…",
        }
    }

    const fn of(state: VoiceState) -> Option<Self> {
        match state {
            VoiceState::Started => Some(Self::Recording),
            VoiceState::Heard => Some(Self::Heard),
            VoiceState::Finished => Some(Self::Sending),
            VoiceState::Learned | VoiceState::Failed => None,
        }
    }
}

/// `GET /voices/script`'s answer.
#[derive(Debug, Deserialize)]
struct Script {
    lang: VoiceLang,
    text: String,
    sample: String,
}

/// `POST /voices`'s answer.
#[derive(Debug, Deserialize)]
struct Learned {
    id: String,
    seconds: f64,
    text: Option<String>,
    transcript: String,
}

#[derive(Debug, Args)]
pub struct VoiceArgs {
    #[command(subcommand)]
    action: VoiceAction,
}

#[derive(Debug, Subcommand)]
enum VoiceAction {
    /// Read a text aloud; the daemon records it and the speech service learns the voice.
    Add(AddArgs),
}

#[derive(Debug, Args)]
struct AddArgs {
    /// Voice id: 1-64 chars of a-z, 0-9, '-' or '_'.
    id: String,
    /// Text to read; the daemon's reading script for the language when omitted.
    #[arg(long)]
    text: Option<String>,
    /// Language of the reading script and of the sample; the daemon's when omitted.
    #[arg(long, value_enum)]
    lang: Option<VoiceLang>,
    /// Silence after speech that ends the recording (e.g. 2.5s); the daemon's default when omitted.
    #[arg(long)]
    silence: Option<String>,
    /// Longest clip (e.g. 30s, at most 30 s); the daemon's default when omitted.
    #[arg(long)]
    max: Option<String>,
    /// Replace a voice with the same id.
    #[arg(long)]
    replace: bool,
    #[command(flatten)]
    daemon: DaemonClient,
}

impl AddArgs {
    // ##### PRIVATE #####

    /// Every line typed on stdin, as one signal each. A thread of its own, never joined: a blocking read
    /// cannot be cancelled, and the process ends without waiting for it.
    fn run_lines() -> mpsc::UnboundedReceiver<()> {
        let (sender, lines) = mpsc::unbounded_channel();
        thread::spawn(move || {
            std::io::stdin()
                .lock()
                .lines()
                .map_while(Result::ok)
                .try_for_each(|_| sender.send(()))
        });
        lines
    }

    /// Prints each recording state as it happens. Connected before the recording starts, so `started` is
    /// seen; a daemon without `/events` only loses the live line.
    async fn run_events(&self, lang: VoiceLang) -> Option<JoinHandle<()>> {
        let url = format!(
            "{}/events",
            self.daemon.url().trim_end_matches('/').replacen("http", "ws", 1)
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(url.as_str()).await.ok()?;
        Some(tokio::spawn(async move {
            while let Some(Ok(message)) = socket.next().await {
                if let Message::Text(text) = message
                    && let Ok(VoiceEvent::Recording { state }) = serde_json::from_str(&text)
                    && let Some(prompt) = VoicePrompt::of(state)
                {
                    eprintln!("{}", prompt.text(lang));
                }
            }
        }))
    }

    /// Waits for the recording's answer: a line finishes it, Ctrl-C stops it. A closed stdin only stops
    /// listening for lines.
    async fn run_record(&self, body: &Value, lines: &mut mpsc::UnboundedReceiver<()>) -> Result<Learned, DaemonError> {
        let record = self.daemon.call::<Learned>(Method::POST, "/voices", body);
        tokio::pin!(record);
        let mut open = true;
        loop {
            tokio::select! {
                learned = &mut record => return learned,
                line = lines.recv(), if open => match line {
                    Some(()) => {
                        self.daemon.call::<Value>(Method::POST, "/voices/finish", &json!({})).await?;
                    }
                    None => open = false,
                },
                _ = tokio::signal::ctrl_c() => {
                    self.daemon.call::<Value>(Method::POST, "/stop", &json!({})).await?;
                }
            }
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Prints the text to read, records it through the daemon once Enter is pressed, prints what was
    /// learned, and says the sample sentence in the new voice.
    async fn run(self) -> Result<(), DaemonError> {
        let query = self
            .lang
            .map(|lang| format!("?lang={}", lang.code()))
            .unwrap_or_default();
        let script: Script = self
            .daemon
            .call(Method::GET, &format!("/voices/script{query}"), &json!({}))
            .await?;
        let lang = script.lang;
        let text = self.text.clone().unwrap_or(script.text);
        println!("{text}\n");
        eprintln!("{}", VoicePrompt::Start.text(lang));
        let mut lines = Self::run_lines();
        lines.recv().await;
        let events = self.run_events(lang).await;
        let body = json!({
            "id": self.id,
            "text": text,
            "silence": self.silence,
            "max": self.max,
            "replace": self.replace,
        });
        let learned = self.run_record(&body, &mut lines).await;
        if let Some(events) = events {
            events.abort();
        }
        let learned = learned?;
        println!("learned {} ({:.1} s)", learned.id, learned.seconds);
        println!("text: {}", learned.text.unwrap_or_default());
        println!("transcript: {}", learned.transcript);
        let say = json!({"text": script.sample, "voice": learned.id, "lang": lang});
        self.daemon.call::<Value>(Method::POST, "/speak", &say).await?;
        Ok(())
    }
}

impl VoiceArgs {
    /// # Errors
    /// The daemon is unreachable, refused the request (the microphone is held, the voice exists, nobody
    /// spoke, the speech service refused the clip), or answered in a shape it does not have.
    pub async fn run(self) -> Result<(), DaemonError> {
        match self.action {
            VoiceAction::Add(args) => args.run().await,
        }
    }
}
