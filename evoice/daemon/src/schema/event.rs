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

/// Where the voice conversation stands: none; listening for the user's turn; the agent's turn running with
/// nothing of it to say yet; saying it; asking the user's permission for something the agent wants to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TalkState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Asking,
}

/// Whether a listen has begun or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenState {
    Open,
    Closed,
}

/// Where a voice recording stands: recording from the microphone, the first speech heard, the recording
/// ended and the clip on its way to the speech service, the voice learned; or failed, whatever ended it
/// otherwise (a stop, a dropped client, a refusal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordState {
    Started,
    Heard,
    Finished,
    Learned,
    Failed,
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
/// - `talk` — `{"type": "talk", "state": "idle" | "listening" | "thinking" | "speaking" | "asking"}`: every
///   change of the voice conversation's state, from its start (`listening`) to its end (`idle`).
/// - `recording` — `{"type": "recording", "state": "started" | "heard" | "finished" | "learned" |
///   "failed"}`: a `POST /voices` once the microphone is open, then its first speech, the end of the
///   recording, and exactly one of `learned` or `failed`. A recording that fails before the microphone
///   opens sends none.
/// - `lagged` — `{"type": "lagged", "missed": n}`: sent to one subscriber alone, in place of the `n`
///   events it fell too far behind to receive. Never broadcast.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DaemonEvent {
    Speech(SpeechState),
    Listening { state: ListenState },
    Partial { text: String },
    Heard(Heard),
    Talk { state: TalkState },
    Recording { state: RecordState },
    Lagged { missed: u64 },
}
