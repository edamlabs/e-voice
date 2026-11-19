use std::sync::Arc;

use e_voice_core::schema::lang::Lang;
use tokio::sync::{Semaphore, broadcast};

use crate::mcp::server::McpServer;
use crate::schema::event::DaemonEvent;
use crate::speech::table::VoiceTable;
use crate::talk::runner::Talk;
use crate::workflow::listen::Listen;
use crate::workflow::queue::SpeechQueue;
use crate::workflow::record::Record;

/// What every handler shares, built once by the composition root. `events` is the sender the queue and
/// the listener announce on, kept here to subscribe from; `subscribers` holds one permit per `/events`
/// socket allowed at once; `mcp` is the MCP door, built on the same handles; `table` and `lang` are the
/// hooks' voice per caller and their fallback language; `talk` is the voice conversation; `record` records
/// and learns a voice.
#[derive(Debug, Clone)]
pub struct AppState {
    pub speech: SpeechQueue,
    pub listen: Arc<dyn Listen>,
    pub events: broadcast::Sender<DaemonEvent>,
    pub subscribers: Arc<Semaphore>,
    pub mcp: McpServer,
    pub table: VoiceTable,
    pub lang: Lang,
    pub talk: Arc<dyn Talk>,
    pub record: Arc<dyn Record>,
}
