use std::future::Future;
use std::num::NonZeroU32;

use e_voice_core::schema::emotion::Emotion;
use e_voice_core::schema::error::NodeError;
use e_voice_core::schema::lang::Lang;
use serde::Serialize;

/// What one segment of speech said: its text, the language it was heard in, and how it sounded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Heard {
    pub text: String,
    pub lang: Lang,
    pub emotion: Emotion,
}

/// What the recognizer says while it listens: someone started speaking, the growing text of a segment,
/// then its final, with the error that kept the recognizer from finishing it, if one did.
#[derive(Debug, Clone, PartialEq)]
pub enum Recognized {
    Started,
    Partial(String),
    Final(Heard, Option<NodeError>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HearingError {
    #[error("cannot reach the speech recognizer: {0}")]
    Connect(String),
    #[error("speech recognizer: {0}")]
    Server(String),
    #[error("recognition protocol: {0}")]
    Protocol(String),
}

/// The capability of turning live audio into text.
pub trait Hearing: Send + Sync + 'static {
    type Transcription: Transcription;

    /// Opens a session for mono audio at `rate`; samples go in through it and what is recognized comes out.
    fn open(&self, rate: NonZeroU32) -> impl Future<Output = Result<Self::Transcription, HearingError>> + Send;
}

/// One live session. [`Transcription::next`] is cancel-safe: a `select!` may drop it to send samples and
/// call it again without losing what was recognized.
pub trait Transcription: Send + 'static {
    /// Sends one block of samples at the session's rate.
    fn send(&mut self, samples: &[f32]) -> impl Future<Output = Result<(), HearingError>> + Send;

    /// The next thing recognized; `None` once the recognizer has closed the session.
    fn next(&mut self) -> impl Future<Output = Option<Result<Recognized, HearingError>>> + Send;
}
