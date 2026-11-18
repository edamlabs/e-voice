use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One request as the fake daemon received it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub body: String,
}

impl Seen {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|error| panic!("{error}: {}", self.body))
    }
}

/// A daemon that answers every request the same way and remembers what it was asked.
pub struct Fake {
    pub address: SocketAddr,
    pub seen: Arc<Mutex<Vec<Seen>>>,
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
    let mut lines = head.lines();
    let mut request = lines.next()?.split_whitespace();
    let (method, path) = (request.next()?.to_owned(), request.next()?.to_owned());
    let header = |name: &str| {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
        })
    };
    let length: usize = header("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        path,
        content_type: header("content-type"),
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

impl Fake {
    pub fn answering(status: u16, content_type: &'static str, body: &'static str) -> Self {
        Self::delayed(Duration::ZERO, status, content_type, body)
    }

    pub fn json(status: u16, body: &'static str) -> Self {
        Self::answering(status, "application/json", body)
    }

    pub fn delayed(delay: Duration, status: u16, content_type: &'static str, body: &'static str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                record.lock().unwrap().push(request);
                std::thread::sleep(delay);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).ok();
            }
        });
        Self { address, seen }
    }

    /// A server whose answer promises 50 bytes and stops after five.
    pub fn truncating() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                read_request(&mut stream);
                stream
                    .write_all(b"HTTP/1.1 200 X\r\nContent-Length: 50\r\nConnection: close\r\n\r\n{\"id\"")
                    .ok();
            }
        });
        Self {
            address,
            seen: Arc::default(),
        }
    }

    /// A server that accepts and hangs up without a word.
    pub fn hanging_up() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        Self {
            address,
            seen: Arc::default(),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// A loopback port nothing listens on.
pub fn closed_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct Out {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl From<Output> for Out {
    fn from(output: Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

/// `ecli` with `args`, `ECLI_DAEMON_URL` unset.
pub fn ecli(args: &[&str]) -> Out {
    ecli_with(args, None)
}

pub fn ecli_with(args: &[&str], url: Option<&str>) -> Out {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ecli"));
    command.args(args).env_remove("ECLI_DAEMON_URL");
    if let Some(url) = url {
        command.env("ECLI_DAEMON_URL", url);
    }
    command.output().unwrap().into()
}
