use std::future::Future;
use std::num::NonZeroU32;

use e_voice_core::schema::lang::Lang;

/// What to say; `voice` and `lang` fall back to the speech service's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequest {
    pub text: String,
    pub voice: Option<String>,
    pub lang: Option<Lang>,
}

/// Mono samples at `rate`, as the service produced them.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub rate: NonZeroU32,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpeechError {
    #[error("cannot reach the speech service: {0}")]
    Connect(String),
    #[error("speech service: {0}")]
    Server(String),
    #[error("speech protocol: {0}")]
    Protocol(String),
}

/// The capability of turning text into audio.
pub trait Speech: Send + Sync + 'static {
    type Utterance: Utterance;

    /// Starts synthesizing `request`; its audio arrives through the returned utterance.
    fn say(&self, request: SpeechRequest) -> impl Future<Output = Result<Self::Utterance, SpeechError>> + Send;
}

/// One text being synthesized.
pub trait Utterance: Send + 'static {
    /// The next frame; `None` once the service has finished the text.
    fn frame(&mut self) -> impl Future<Output = Option<Result<Frame, SpeechError>>> + Send;

    /// Tells the service to stop this text; no frame is read after it.
    fn cancel(&mut self) -> impl Future<Output = Result<(), SpeechError>> + Send;
}
