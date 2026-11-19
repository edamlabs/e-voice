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

/// A POST whose body and content type are the caller's: `None` sends neither.
#[allow(dead_code)]
pub fn post_typed(port: u16, path: &str, content_type: Option<&str>, body: Option<&str>) -> (u16, String) {
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(10)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let kind = content_type.map_or_else(String::new, |kind| format!("Content-Type: {kind}\r\n"));
    let length = body.map_or_else(String::new, |body| format!("Content-Length: {}\r\n", body.len()));
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{kind}{length}\r\n{}",
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

fn send(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(10)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
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
