use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use crate::api::state::AppState;
use crate::schema::event::DaemonEvent;

/// Events the daemon keeps for subscribers that fall behind: one ring shared by all of them, so memory is
/// bounded by it however many subscribe. A subscriber further behind gets `lagged` instead.
pub const EVENTS: usize = 256;

/// Sockets on `/events` at once. The daemon is local and its clients are few (a CLI, an MCP server, a
/// UI); past this a subscription is refused before the upgrade.
pub const SUBSCRIBERS: usize = 16;

const GOING_AWAY: u16 = 1001;
const UNSUPPORTED: u16 = 1003;
const TOO_SLOW: u16 = 1008;
/// RFC 6455 «Message Too Big».
const TOO_BIG: u16 = 1009;

/// The largest message or frame the socket takes: 1 KiB. It only sends; what a client may send is control
/// frames, at most 125 bytes each by RFC 6455.
const MESSAGE: usize = 1_024;

/// The longest one event may take to reach a subscriber. A client that stops reading would otherwise hold
/// its task forever once the kernel's buffers fill.
const SEND: Duration = Duration::from_secs(5);

/// How long a socket closed with 1009 stays open before it is dropped. The refused frame's payload may
/// still be arriving; dropping at once makes the kernel reset the connection, and the reset can reach the
/// client before the close does. Nothing is read meanwhile.
const LINGER: Duration = Duration::from_secs(1);

/// Subscribes to the daemon's events: a WebSocket that sends one JSON text message per [`DaemonEvent`],
/// from the moment of the upgrade on, and takes nothing. 503 `{"error"}` past [`SUBSCRIBERS`]. Closes with
/// 1003 when the client sends data, 1008 when it does not read an event within 5 s, 1009 when a frame
/// passes 1 KiB.
pub async fn events(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let Ok(permit) = Arc::clone(&state.subscribers).try_acquire_owned() else {
        let body = Json(json!({"error": format!("{SUBSCRIBERS} subscribers already")}));
        return (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    };
    let receiver = state.events.subscribe();
    ws.max_message_size(MESSAGE)
        .max_frame_size(MESSAGE)
        .on_upgrade(move |socket| serve(socket, receiver, permit))
}

/// Whether a read failed because the client's message or frame passed [`MESSAGE`].
fn oversized(error: &axum::Error) -> bool {
    std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<tungstenite::Error>())
        .is_some_and(|error| matches!(error, tungstenite::Error::Capacity(_)))
}

/// Forwards events until the client leaves or breaks a rule; the permit goes back when this ends. A client
/// that hung up gets no close frame, the others get theirs, bounded like any event: a client too slow for
/// an event is too slow for its close.
async fn serve(socket: WebSocket, mut receiver: Receiver<DaemonEvent>, _permit: OwnedSemaphorePermit) {
    let (mut sink, mut source) = socket.split();
    let ending = loop {
        tokio::select! {
            message = source.next() => match message {
                Some(Ok(Message::Text(_) | Message::Binary(_))) => break Some((UNSUPPORTED, "this socket only sends")),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Err(error)) if oversized(&error) => break Some((TOO_BIG, "message over 1024 bytes")),
                Some(Ok(Message::Close(_)) | Err(_)) | None => break None,
            },
            event = receiver.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(RecvError::Lagged(missed)) => DaemonEvent::Lagged { missed },
                    Err(RecvError::Closed) => break Some((GOING_AWAY, "the daemon is stopping")),
                };
                let text = Utf8Bytes::from(serde_json::to_string(&event).unwrap_or_default());
                match tokio::time::timeout(SEND, sink.send(Message::Text(text))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => break None,
                    Err(_) => break Some((TOO_SLOW, "an event waited over 5 s")),
                }
            }
        }
    };
    if let Some((code, reason)) = ending {
        let frame = CloseFrame {
            code,
            reason: Utf8Bytes::from_static(reason),
        };
        let close = sink.send(Message::Close(Some(frame)));
        tokio::time::timeout(SEND, close).await.ok();
        if code == TOO_BIG {
            tokio::time::sleep(LINGER).await;
        }
    }
}
