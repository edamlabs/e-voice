use std::sync::Arc;

use tokio::sync::{Semaphore, broadcast};

use crate::schema::event::DaemonEvent;
use crate::workflow::listen::Listen;
use crate::workflow::queue::SpeechQueue;

/// What every handler shares, built once by the composition root. `events` is the sender the queue and
/// the listener announce on, kept here to subscribe from; `subscribers` holds one permit per `/events`
/// socket allowed at once.
#[derive(Debug, Clone)]
pub struct AppState {
    pub speech: SpeechQueue,
    pub listen: Arc<dyn Listen>,
    pub events: broadcast::Sender<DaemonEvent>,
    pub subscribers: Arc<Semaphore>,
}
