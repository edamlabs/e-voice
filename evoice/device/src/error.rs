use cpal::SampleFormat;
use e_voice_core::audio::AudioError;

/// Why an audio device could not be listed or opened.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no input device{}", .0.as_deref().map(|name| format!(" matching {name:?}")).unwrap_or_default())]
    Input(Option<String>),
    #[error("no output device")]
    Output,
    #[error("unsupported sample format {0}")]
    Format(SampleFormat),
    #[error("audio backend: {0}")]
    Backend(#[from] cpal::Error),
    #[error("resampling: {0}")]
    Resample(#[from] AudioError),
}
