use std::fmt::Debug;
use std::future::Future;
use std::num::NonZeroU32;

use e_voice_core::schema::lang::Lang;
use futures_util::future::BoxFuture;

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

/// The voices a speech service can speak with: the ids its `voice` accepts. Object-safe, because every
/// door shares one behind an `Arc`.
pub trait Voices: Debug + Send + Sync {
    fn list(&self) -> BoxFuture<'_, Result<Vec<String>, SpeechError>>;
}

/// A voice to learn: its id, mono samples at `rate` of one speaker, and what they say when it is known.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceClip {
    pub id: String,
    pub rate: NonZeroU32,
    pub samples: Vec<f32>,
    pub text: Option<String>,
}

/// The capability of learning a voice from a clip, usable as `voice` once learned; a voice with the same
/// id is replaced. Object-safe, like [`Voices`].
pub trait Learning: Debug + Send + Sync {
    fn learn(&self, clip: VoiceClip) -> BoxFuture<'_, Result<(), SpeechError>>;
}
