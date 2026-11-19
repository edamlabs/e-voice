use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One request as the fake daemon received it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub body: String,
}

impl Seen {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|error| panic!("{error}: {}", self.body))
    }
}

/// What a recording answers once it is ended from outside.
#[derive(Debug, Clone, Copy)]
pub enum Ending {
    /// 200 with a learned voice.
    Learned,
    /// 409, as the daemon answers a recording that was stopped.
    Stopped,
    /// 409, as the daemon answers a voice that exists.
    Exists,
}

type Shared = Arc<(Mutex<Vec<Seen>>, Condvar)>;

/// A daemon for `ecli voice add`: the script and the speech answer at once; `POST /voices` is held open
/// until the client finishes or stops it, as the real recording is.
pub struct Fake {
    pub address: SocketAddr,
    shared: Shared,
}

fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut request = head.lines().next()?.split_whitespace();
    let (method, path) = (request.next()?.to_owned(), request.next()?.to_owned());
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        path,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    let text = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(text.as_bytes()).ok();
}

impl Fake {
    pub fn start(ending: Ending) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let shared: Shared = Arc::default();
        let state = Arc::clone(&shared);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let state = Arc::clone(&state);
                std::thread::spawn(move || Self::serve(stream, &state, ending));
            }
        });
        Self { address, shared }
    }

    fn serve(mut stream: TcpStream, state: &Shared, ending: Ending) {
        let Some(request) = read_request(&mut stream) else {
            return;
        };
        let (seen, signal) = &**state;
        seen.lock().unwrap().push(request.clone());
        signal.notify_all();
        let path = request.path.split('?').next().unwrap_or_default().to_owned();
        match (request.method.as_str(), path.as_str()) {
            ("GET", "/voices/script") => {
                let (lang, text, sample) = if request.path.ends_with("lang=en") {
                    ("en", "Read this in English.", "Hello, new voice.")
                } else {
                    ("es", "Lee esto en español.", "Hola, voz nueva.")
                };
                reply(
                    &mut stream,
                    200,
                    &format!(r#"{{"lang":"{lang}","text":"{text}","sample":"{sample}"}}"#),
                );
            }
            ("POST", "/voices") => {
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut held = seen.lock().unwrap();
                while !held
                    .iter()
                    .any(|seen| seen.path == "/voices/finish" || seen.path == "/stop")
                {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return;
                    }
                    held = signal.wait_timeout(held, left).unwrap().0;
                }
                drop(held);
                match ending {
                    Ending::Learned => reply(
                        &mut stream,
                        200,
                        r#"{"id":"ana","seconds":2.5,"text":"lo leído","transcript":"lo oído"}"#,
                    ),
                    Ending::Stopped => reply(&mut stream, 409, r#"{"error":"the recording was stopped"}"#),
                    Ending::Exists => reply(&mut stream, 409, r#"{"error":"the voice \"ana\" exists"}"#),
                }
            }
            ("POST", "/voices/finish") => reply(&mut stream, 200, r#"{"finished":true}"#),
            ("POST", "/stop") => reply(&mut stream, 200, r#"{"stopped":null,"dropped":0}"#),
            ("POST", "/speak") => reply(&mut stream, 200, r#"{"id":0,"outcome":"done"}"#),
            _ => reply(&mut stream, 404, r#"{"error":"not found"}"#),
        }
    }

    /// An exit that does not wait: a refusal ends the recording as soon as it is asked.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.shared
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.path != "/events")
            .cloned()
            .collect()
    }

    /// Waits until a request to `path` has arrived.
    pub fn until(&self, path: &str) {
        let (seen, signal) = &*self.shared;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut held = seen.lock().unwrap();
        while !held.iter().any(|seen| seen.path.split('?').next() == Some(path)) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no request to {path}; saw {held:?}");
            held = signal.wait_timeout(held, left).unwrap().0;
        }
    }
}
