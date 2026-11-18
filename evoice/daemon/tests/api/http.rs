use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// A port nothing holds right now.
pub fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One blocking HTTP/1.1 request; returns the status and the body.
pub fn request(port: u16, method: &str, path: &str) -> (u16, String) {
    send(port, method, path, None)
}

/// A request with a JSON body.
pub fn post_json(port: u16, path: &str, body: &str) -> (u16, String) {
    send(port, "POST", path, Some(body))
}

fn send(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(2)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let headers = body.map_or_else(String::new, |body| {
        format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len())
    });
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n{}",
        body.unwrap_or_default()
    )
    .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// Waits until `/health` answers 200 on `port`.
pub fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let answered = std::panic::catch_unwind(|| request(port, "GET", "/health").0 == 200).unwrap_or(false);
        if answered {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("nothing answered on {port}");
}
