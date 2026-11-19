use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use e_voice_core::schema::lang::Lang;
use serde_json::Value;

use crate::api::state::AppState;
use crate::speech::base::SpeechRequest;
use crate::speech::digest::SpeechDigest;
use crate::speech::notice::Notice;
use crate::speech::table::CLIENT;

/// The largest hook input read: 1 MiB. A reply longer than that is not one to read aloud; past it the body
/// is dropped unread and nothing is said.
const BODY: usize = 1_048_576;

/// Claude Code's HTTP hooks. Each answers 200 with an empty body at once — or, for an input over 1 MiB,
/// the connection is closed once the daemon stops reading at the cap; either way nothing is said and
/// Claude Code goes on. A 2xx with no body is the one answer it reads as a plain success, and it waits for
/// it, so nothing here may slow it. What is said goes to the speech queue without waiting for it; a full
/// queue, a dead speech service or an input that does not parse is logged, never answered.
#[derive(Debug)]
pub struct Hook;

impl Hook {
    // ##### PRIVATE #####

    /// The hook input, read leniently: Claude Code adds fields with its versions, so only what is needed
    /// is looked up later, and an input that is not JSON, or is over [`BODY`], is no input.
    async fn read(body: Body) -> Option<Value> {
        let bytes = axum::body::to_bytes(body, BODY)
            .await
            .map_err(|error| tracing::warn!(%error, "hook.unread"))
            .ok()?;
        serde_json::from_slice(&bytes)
            .map_err(|error| tracing::warn!(%error, "hook.unparsed"))
            .ok()
    }

    /// Queues `text` in the caller's voice, without waiting for it to be spoken.
    fn speak(state: &AppState, headers: &HeaderMap, text: String, lang: Lang) {
        let caller = headers.get(CLIENT).and_then(|value| value.to_str().ok());
        let voice = state.table.voice(None, caller);
        let request = SpeechRequest {
            text,
            voice,
            lang: Some(lang),
        };
        if let Err(error) = state.speech.submit(request) {
            tracing::warn!(%error, "hook.dropped");
        }
    }
}

/// `POST /hooks/stop`: the opening of Claude's last reply (`last_assistant_message`), as [`SpeechDigest`]
/// reduces it, in the language it is written in.
pub async fn stop(State(state): State<AppState>, headers: HeaderMap, body: Body) -> StatusCode {
    let digest = Hook::read(body)
        .await
        .and_then(|input| input.get("last_assistant_message")?.as_str().map(str::to_owned))
        .and_then(|message| SpeechDigest::of(&message, state.lang));
    if let Some((text, lang)) = digest {
        Hook::speak(&state, &headers, text, lang);
    }
    StatusCode::OK
}

/// `POST /hooks/notification`: a short phrase for a notice that waits on the user (`permission_prompt`,
/// `idle_prompt`), in `[daemon] lang`; any other notification type says nothing.
pub async fn notification(State(state): State<AppState>, headers: HeaderMap, body: Body) -> StatusCode {
    let notice = Hook::read(body)
        .await
        .and_then(|input| serde_json::from_value::<Notice>(input.get("notification_type")?.clone()).ok());
    if let Some(notice) = notice {
        Hook::speak(&state, &headers, notice.text(state.lang).to_owned(), state.lang);
    }
    StatusCode::OK
}
