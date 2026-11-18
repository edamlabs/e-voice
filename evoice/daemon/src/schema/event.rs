use serde::Serialize;

use crate::hearing::base::Heard;

/// Where the speech queue stands: idle, or busy with the text `id` — speaking while the service sends
/// audio, draining while the device plays the rest, cancelling while a stop silences both. On the wire,
/// `{"state": "idle"}` or `{"state": "speaking" | "draining" | "cancelling", "id": n}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum SpeechState {
    Idle,
    Speaking { id: u64 },
    Draining { id: u64 },
    Cancelling { id: u64 },
}

/// Whether a listen has begun or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenState {
    Open,
    Closed,
}

/// Everything `GET /events` sends: the daemon's public wire contract, one JSON text message per event,
/// tagged by `type`.
///
/// - `speech` — every transition of the speech queue, flattened with its [`SpeechState`]:
///   `{"type": "speech", "state": "idle"}`, `{"type": "speech", "state": "speaking", "id": 3}`.
///   Ids are the ones `POST /speak` answers with.
/// - `listening` — `{"type": "listening", "state": "open" | "closed"}`. `open` when a listen is accepted
///   (before the microphone opens), `closed` exactly once when it ends, whatever the ending, also when its
///   caller stopped waiting. A refused listen (400, 409) sends neither.
/// - `partial` — `{"type": "partial", "text": "…"}`: the recognizer's growing text of the current segment.
/// - `heard` — `{"type": "heard", "text": "…", "lang": "es" | "en", "emotion": {"label", "scores",
///   "model"}}`: exactly once per successful listen, before its HTTP answer and before its `closed`; a
///   listen that ends otherwise sends none.
/// - `lagged` — `{"type": "lagged", "missed": n}`: sent to one subscriber alone, in place of the `n`
///   events it fell too far behind to receive. Never broadcast.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DaemonEvent {
    Speech(SpeechState),
    Listening { state: ListenState },
    Partial { text: String },
    Heard(Heard),
    Lagged { missed: u64 },
}
