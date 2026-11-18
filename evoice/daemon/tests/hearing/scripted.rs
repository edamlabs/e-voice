use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// What the fake recognizer does, in order, once a client has connected.
#[derive(Debug, Clone)]
pub enum Step {
    Text(String),
    Binary(Vec<u8>),
    Ping,
    Wait(Duration),
    Close(u16, &'static str),
    Drop,
}

pub fn text(json: &str) -> Step {
    Step::Text(json.to_owned())
}

/// What the client sent: the request target and every binary frame, in order.
#[derive(Debug, Default, Clone)]
pub struct Seen {
    pub target: String,
    pub frames: Vec<Vec<u8>>,
}

pub struct Scripted {
    pub address: SocketAddr,
    pub seen: Arc<Mutex<Seen>>,
}

impl Scripted {
    #[allow(clippy::result_large_err)]
    pub async fn start(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let record = Arc::clone(&seen);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let target = Arc::clone(&record);
            let socket = tokio_tungstenite::accept_hdr_async(stream, move |request: &Request, response: Response| {
                target.lock().unwrap().target = request.uri().to_string();
                Ok(response)
            })
            .await
            .unwrap();
            let (mut sink, mut stream) = socket.split();
            let frames = Arc::clone(&record);
            let reader = tokio::spawn(async move {
                while let Some(Ok(message)) = stream.next().await {
                    if let Message::Binary(bytes) = message {
                        frames.lock().unwrap().frames.push(bytes.to_vec());
                    }
                }
            });
            for step in steps {
                let sent = match step {
                    Step::Text(json) => sink.send(Message::text(json)).await,
                    Step::Binary(bytes) => sink.send(Message::Binary(bytes.into())).await,
                    Step::Ping => sink.send(Message::Ping(Vec::new().into())).await,
                    Step::Wait(pause) => {
                        tokio::time::sleep(pause).await;
                        Ok(())
                    }
                    Step::Close(code, reason) => {
                        let frame = CloseFrame {
                            code: CloseCode::from(code),
                            reason: reason.into(),
                        };
                        sink.send(Message::Close(Some(frame))).await
                    }
                    Step::Drop => {
                        reader.abort();
                        return;
                    }
                };
                if sent.is_err() {
                    reader.abort();
                    return;
                }
            }
            reader.await.ok();
        });
        Self { address, seen }
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.address)
    }

    pub fn target(&self) -> String {
        self.seen.lock().unwrap().target.clone()
    }

    pub fn frames(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().frames.clone()
    }
}
