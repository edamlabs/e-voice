#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod daemon;
mod stt;
mod tts;
mod voice;
mod wav;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use e_voice_device::host::CpalHost;

use crate::daemon::{DaemonError, ListenArgs, SayArgs, StopArgs, TalkArgs};
use crate::stt::{SttArgs, SttError};
use crate::tts::{TtsArgs, TtsError};
use crate::voice::VoiceArgs;

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Stt(#[from] SttError),
    #[error(transparent)]
    Tts(#[from] TtsError),
    #[error(transparent)]
    Daemon(#[from] DaemonError),
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Stream speech to the gateway and print transcripts as they arrive.
    Stt(SttArgs),
    /// Stream text to the TTS gateway and play the voice as it is generated.
    Tts(TtsArgs),
    /// List audio inputs; `*` marks the system default.
    Devices,
    /// Speak a text through the daemon and wait until it has played.
    Say(SayArgs),
    /// Silence the daemon's voice and drop every text waiting.
    Stop(StopArgs),
    /// Listen once through the daemon and print what was heard.
    Listen(ListenArgs),
    /// Start or stop a voice conversation with the agent through the daemon.
    Talk(TalkArgs),
    /// Learn a voice: read a text aloud, the daemon records it and the speech service learns it.
    Voice(VoiceArgs),
}

/// Terminal tester for the e-voice gateway.
#[derive(Debug, Parser)]
#[command(name = "ecli", version = env!("E_VOICE_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[tokio::main]
async fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        CliCommand::Stt(args) => args.run().await.map_err(CliError::from),
        CliCommand::Tts(args) => args.run().await.map_err(CliError::from),
        CliCommand::Devices => CpalHost::inputs()
            .map_err(|error| CliError::from(SttError::from(error)))
            .map(|devices| {
                for (name, chosen) in devices {
                    println!("{} {name}", if chosen { "*" } else { " " });
                }
            }),
        CliCommand::Say(args) => args.run().await.map_err(CliError::from),
        CliCommand::Stop(args) => args.run().await.map_err(CliError::from),
        CliCommand::Listen(args) => args.run().await.map_err(CliError::from),
        CliCommand::Talk(args) => args.run().await.map_err(CliError::from),
        CliCommand::Voice(args) => args.run().await.map_err(CliError::from),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ecli: {error}");
            ExitCode::FAILURE
        }
    }
}
