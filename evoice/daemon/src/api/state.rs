use crate::workflow::queue::SpeechQueue;

/// What every handler shares, built once by the composition root.
#[derive(Debug, Clone)]
pub struct AppState {
    pub speech: SpeechQueue,
}
