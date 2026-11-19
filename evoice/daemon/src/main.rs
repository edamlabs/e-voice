use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use e_voice_core::logger::{Logger, LoggerError};
use e_voice_core::settings::SettingsError;
use e_voice_daemon::api::events::{EVENTS, SUBSCRIBERS};
use e_voice_daemon::api::server::{Server, ServerError};
use e_voice_daemon::api::state::AppState;
use e_voice_daemon::core::settings::Settings;
use e_voice_daemon::hearing::stt::SttHearing;
use e_voice_daemon::mcp::server::McpServer;
use e_voice_daemon::schema::event::DaemonEvent;
use e_voice_daemon::speech::base::{Learning, Voices};
use e_voice_daemon::speech::table::VoiceTable;
use e_voice_daemon::speech::tts::TtsSpeech;
use e_voice_daemon::talk::agent::claude::ClaudeAgent;
use e_voice_daemon::talk::approval::Approvals;
use e_voice_daemon::talk::prompt::TalkPrompt;
use e_voice_daemon::talk::runner::{Talk, TalkRunner, TalkSetup};
use e_voice_daemon::workflow::audio::AudioThread;
use e_voice_daemon::workflow::capture::CaptureThread;
use e_voice_daemon::workflow::listen::{Listen, Listener};
use e_voice_daemon::workflow::queue::SpeechQueue;
use e_voice_daemon::workflow::record::{Record, Recorder};
use e_voice_device::capture::cpal::CpalCapture;
use e_voice_device::playback::cpal::CpalPlayback;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::{Semaphore, broadcast};

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error(transparent)]
    Logger(#[from] LoggerError),
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error("cannot install the SIGTERM handler: {0}")]
    Signal(std::io::Error),
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Serve the local gateway on `[daemon] port`.
    Serve,
}

/// e-voice daemon: the one local process every voice client talks to.
#[derive(Debug, Parser)]
#[command(name = "evoiced", version = env!("E_VOICE_VERSION"))]
struct Cli {
    /// Settings file; without it, `evoice.toml` is read if present.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: CliCommand,
}

impl Cli {
    // ##### PRIVATE #####

    /// The voice conversation: a fresh headless Claude Code per conversation, reaching the daemon's own MCP
    /// on loopback, in the configured project (the daemon's directory when none is set).
    fn run_talk(
        settings: &Settings,
        listen: &Arc<dyn Listen>,
        speech: &SpeechQueue,
        record: &Arc<dyn Record>,
        approvals: Approvals,
        events: &broadcast::Sender<DaemonEvent>,
    ) -> Arc<dyn Talk> {
        let claude = settings.daemon.talk.claude.clone();
        let mcp = format!("http://127.0.0.1:{}/mcp", settings.daemon.port);
        let prompt = TalkPrompt::Voice.text(settings.daemon.lang).to_owned();
        let cwd = claude
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let setup = TalkSetup {
            lang: settings.daemon.lang,
            cwd,
            config: settings.daemon.talk.clone(),
        };
        let make = move |token: String| ClaudeAgent::new(claude.clone(), mcp.clone(), prompt.clone(), token);
        let (listen, speech, record, events) = (Arc::clone(listen), speech.clone(), Arc::clone(record), events.clone());
        Arc::new(TalkRunner::new(make, listen, speech, record, approvals, events, setup))
    }

    /// Resolves on SIGINT or on `terminate`, the SIGTERM handler installed before the daemon binds: a
    /// daemon its supervisor cannot stop gracefully (systemd sends SIGTERM) refuses to start.
    async fn run_shutdown(mut terminate: Signal) {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        tracing::info!("server.stopping");
    }

    // ##########################################################

    // ##### PUBLIC #####

    async fn run(self) -> Result<(), MainError> {
        let settings = Settings::load(self.config.as_deref())?;
        let _logger = Logger::init(&settings.server.log)?;
        match self.command {
            CliCommand::Serve => {
                let terminate = signal(SignalKind::terminate()).map_err(MainError::Signal)?;
                let (events, _) = broadcast::channel(EVENTS);
                let output = settings.daemon.output.clone();
                let audio = AudioThread::spawn(move || {
                    CpalPlayback::open(output.as_deref(), |error| tracing::error!(%error, "speaker.failed"))
                });
                let tts = TtsSpeech::new(settings.daemon.tts.clone());
                let voices: Arc<dyn Voices> = Arc::new(tts.clone());
                let learning: Arc<dyn Learning> = Arc::new(tts.clone());
                let speech = SpeechQueue::start(tts, audio, events.clone());
                let input = settings.daemon.input.clone();
                let capture = CaptureThread::new(move |deliver| {
                    let capture = CpalCapture::open(input.as_deref(), deliver, |error| {
                        tracing::error!(%error, "microphone.failed");
                    })?;
                    Ok(Box::new(capture))
                });
                let hearing = SttHearing::new(settings.daemon.stt.clone());
                let listener = Listener::new(hearing.clone(), capture.clone(), settings.daemon.listen, events.clone());
                let listen: Arc<dyn Listen> = Arc::new(listener);
                let record: Arc<dyn Record> = Arc::new(Recorder::new(
                    hearing,
                    capture,
                    Arc::clone(&listen),
                    Arc::clone(&voices),
                    learning,
                    settings.daemon.listen,
                    events.clone(),
                ));
                let subscribers = Arc::new(Semaphore::new(SUBSCRIBERS));
                let table = VoiceTable::new(settings.daemon.voices.clone());
                let approvals = Approvals::default();
                let desk = approvals.clone();
                let talk = Self::run_talk(&settings, &listen, &speech, &record, approvals, &events);
                let mcp = McpServer::new(
                    speech.clone(),
                    Arc::clone(&listen),
                    voices,
                    table.clone(),
                    desk,
                    Arc::clone(&talk),
                );
                let state = AppState {
                    speech,
                    listen,
                    events,
                    subscribers,
                    mcp,
                    table,
                    lang: settings.daemon.lang,
                    talk,
                    record,
                };
                Server::serve(&settings, state, Self::run_shutdown(terminate)).await?;
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match Cli::parse().run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("evoiced: {error}");
            ExitCode::FAILURE
        }
    }
}
