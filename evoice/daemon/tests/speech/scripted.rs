use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// What the fake TTS server does after the client has sent its text and `close`.
#[derive(Debug, Clone)]
pub enum Step {
    Text(String),
    Binary(Vec<u8>),
    Ping,
    Close,
    Drop,
}

pub fn text(json: &str) -> Step {
    Step::Text(json.to_owned())
}

pub fn floats(values: &[f32]) -> Step {
    Step::Binary(values.iter().flat_map(|value| value.to_le_bytes()).collect())
}

/// What the client sent: the request target and every text frame, in order.
#[derive(Debug, Default, Clone)]
pub struct Seen {
    pub target: String,
    pub messages: Vec<String>,
}

pub struct Scripted {
    pub address: SocketAddr,
    pub seen: Arc<Mutex<Seen>>,
}

impl Scripted {
    /// Serves one connection: records the target and the client's frames up to its `close`, then plays `steps`.
    #[allow(clippy::result_large_err)]
    pub async fn start(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let record = Arc::clone(&seen);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let target = Arc::clone(&record);
            let mut socket =
                tokio_tungstenite::accept_hdr_async(stream, move |request: &Request, response: Response| {
                    target.lock().unwrap().target = request.uri().to_string();
                    Ok(response)
                })
                .await
                .unwrap();
            while let Some(Ok(message)) = socket.next().await {
                if let Message::Text(text) = message {
                    let closing = text.contains(r#""type":"close""#);
                    record.lock().unwrap().messages.push(text.to_string());
                    if closing {
                        break;
                    }
                }
            }
            for step in steps {
                let sent = match step {
                    Step::Text(json) => socket.send(Message::text(json)).await,
                    Step::Binary(bytes) => socket.send(Message::Binary(bytes.into())).await,
                    Step::Ping => socket.send(Message::Ping(Vec::new().into())).await,
                    Step::Close => {
                        let frame = CloseFrame {
                            code: CloseCode::Normal,
                            reason: "bye".into(),
                        };
                        socket.send(Message::Close(Some(frame))).await
                    }
                    Step::Drop => return,
                };
                if sent.is_err() {
                    return;
                }
            }
            while let Some(Ok(message)) = socket.next().await {
                if let Message::Text(text) = message {
                    record.lock().unwrap().messages.push(text.to_string());
                }
            }
        });
        Self { address, seen }
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.address)
    }

    pub fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone()
    }
}
