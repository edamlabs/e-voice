use std::sync::Arc;

use crate::workflow::listen::Listen;
use crate::workflow::queue::SpeechQueue;

/// What every handler shares, built once by the composition root.
#[derive(Debug, Clone)]
pub struct AppState {
    pub speech: SpeechQueue,
    pub listen: Arc<dyn Listen>,
}
