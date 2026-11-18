use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use e_voice_core::logger::{Logger, LoggerError};
use e_voice_core::settings::SettingsError;
use e_voice_daemon::api::server::{Server, ServerError};
use e_voice_daemon::api::state::AppState;
use e_voice_daemon::core::settings::Settings;
use e_voice_daemon::speech::tts::TtsSpeech;
use e_voice_daemon::workflow::audio::AudioThread;
use e_voice_daemon::workflow::queue::SpeechQueue;
use e_voice_device::playback::cpal::CpalPlayback;

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error(transparent)]
    Logger(#[from] LoggerError),
    #[error(transparent)]
    Server(#[from] ServerError),
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

    /// Resolves on SIGINT or SIGTERM.
    async fn run_shutdown() {
        let interrupt = tokio::signal::ctrl_c();
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        let term = async move {
            match terminate.as_mut() {
                Some(signal) => signal.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = interrupt => {}
            _ = term => {}
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
                let output = settings.daemon.output.clone();
                let audio = AudioThread::spawn(move || {
                    CpalPlayback::open(output.as_deref(), |error| tracing::error!(%error, "speaker.failed"))
                });
                let speech = SpeechQueue::start(TtsSpeech::new(settings.daemon.tts.clone()), audio);
                Server::serve(&settings, AppState { speech }, Self::run_shutdown()).await?;
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
