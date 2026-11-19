use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::http::{free_port, post_json, request, wait_ready};

fn evoiced() -> Command {
    Command::new(env!("CARGO_BIN_EXE_evoiced"))
}

struct Daemon(Child);

impl Daemon {
    /// SIGTERM, then the exit status: a clean exit is what lets llvm-cov write the profile.
    fn stop(&mut self) -> Option<i32> {
        Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.0.kill().ok();
        self.0.wait().ok();
        None
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            self.stop();
        }
    }
}

fn config(name: &str, text: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("evoiced-bin-{}-{name}.toml", std::process::id()));
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn test_version_prints_the_release_version() {
    let output = evoiced().arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("evoiced {}", env!("E_VOICE_VERSION"))
    );
}

#[test]
fn test_a_missing_config_exits_1_with_the_evoiced_prefix() {
    let output = evoiced()
        .args(["serve", "--config", "definitely-missing.toml"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("evoiced: "),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_port_zero_exits_1_naming_the_key() {
    let path = config("zero", "[daemon]\nport = 0\n");
    let output = evoiced().arg("serve").arg("--config").arg(&path).output().unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("evoiced: ") && stderr.contains("daemon.port"),
        "{stderr}"
    );
}

#[test]
fn test_serve_answers_health_on_the_port_of_the_environment_override() {
    let port = free_port();
    let path = config("env", "[daemon]\nport = 5799\n");
    let mut daemon = Daemon(
        evoiced()
            .arg("serve")
            .arg("--config")
            .arg(&path)
            .env("EVOICE_DAEMON__PORT", port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(port);
    let (status, body) = request(port, "GET", "/health");
    let code = daemon.stop();
    std::fs::remove_file(&path).ok();
    assert_eq!(code, Some(0));
    assert_eq!(status, 200);
    assert!(body.contains(r#""status":"ok""#), "{body}");
}

#[test]
fn test_a_held_port_exits_1_naming_the_address() {
    let held = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = held.local_addr().unwrap().port();
    let path = config("held", &format!("[daemon]\nport = {port}\n"));
    let mut child = evoiced()
        .arg("serve")
        .arg("--config")
        .arg(&path)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(status.code(), Some(1));
    assert!(
        stderr.starts_with(&format!("evoiced: cannot bind 127.0.0.1:{port}: ")),
        "{stderr}"
    );
}

#[test]
fn test_sigterm_stops_the_daemon_cleanly_with_exit_0() {
    let port = free_port();
    let path = config("sigterm", &format!("[daemon]\nport = {port}\n"));
    let mut child = evoiced()
        .arg("serve")
        .arg("--config")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_ready(port);
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let status = child.wait().unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn test_a_tts_url_that_is_not_ws_exits_1_naming_the_key() {
    let path = config("wss", "[daemon]\ntts = \"wss://tts.local\"\n");
    let output = evoiced().arg("serve").arg("--config").arg(&path).output().unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("evoiced: ") && stderr.contains("daemon.tts"),
        "{stderr}"
    );
}

/// The real binary, an output device that does not exist: `/health` answers without any device, and
/// `/speak` is a 503 naming the device, with no sound made.
#[test]
fn test_serve_without_an_output_device_answers_health_and_503_on_speak() {
    let port = free_port();
    let path = config(
        "nodevice",
        &format!("[daemon]\nport = {port}\noutput = \"no-such-output-xyz\"\n"),
    );
    let mut daemon = Daemon(
        evoiced()
            .arg("serve")
            .arg("--config")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(port);
    assert_eq!(request(port, "GET", "/health").0, 200);
    let (status, body) = post_json(port, "/speak", r#"{"text":"hola"}"#);
    let code = daemon.stop();
    std::fs::remove_file(&path).ok();
    assert_eq!(code, Some(0));
    assert_eq!(status, 503);
    assert_eq!(body, r#"{"error":"no output device","outcome":"failed"}"#);
}

/// The real binary, an input device that does not exist: `/health` answers, `/listen` is a 503 naming
/// the device before any recognizer is contacted, and the microphone is tried again on the next call.
#[test]
fn test_serve_without_an_input_device_answers_health_and_503_on_listen() {
    let port = free_port();
    let path = config(
        "noinput",
        &format!("[daemon]\nport = {port}\ninput = \"no-such-device-evoice-test\"\n"),
    );
    let mut daemon = Daemon(
        evoiced()
            .arg("serve")
            .arg("--config")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(port);
    assert_eq!(request(port, "GET", "/health").0, 200);
    let first = post_json(port, "/listen", "{}");
    let second = post_json(port, "/listen", r#"{"timeout":"1s"}"#);
    let code = daemon.stop();
    std::fs::remove_file(&path).ok();
    assert_eq!(code, Some(0));
    for (status, body) in [first, second] {
        assert_eq!(status, 503, "{body}");
        assert_eq!(
            body,
            r#"{"error":"no input device matching \"no-such-device-evoice-test\""}"#
        );
    }
}

/// The `/speak` twin with the name the daemon's maintainers use: the device name is not echoed, the
/// outcome is `failed`.
#[test]
fn test_serve_with_a_named_missing_output_is_a_503_failed_and_exits_0_on_sigterm() {
    let port = free_port();
    let path = config(
        "namedout",
        &format!("[daemon]\nport = {port}\noutput = \"no-such-device-evoice-test\"\n"),
    );
    let mut daemon = Daemon(
        evoiced()
            .arg("serve")
            .arg("--config")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(port);
    let (status, body) = post_json(port, "/speak", r#"{"text":"x"}"#);
    let code = daemon.stop();
    std::fs::remove_file(&path).ok();
    assert_eq!(code, Some(0));
    assert_eq!(status, 503, "{body}");
    assert!(
        body.contains(r#""outcome":"failed""#) && body.contains(r#""error":""#),
        "{body}"
    );
}

/// A supervisor's SIGTERM must stop the daemon whatever clients hold open: a subscriber on `/events` is a
/// connection that never ends by itself.
#[test]
fn test_sigterm_with_an_open_events_subscriber_exits_0_within_ten_seconds() {
    let port = free_port();
    let path = config("subscribed", &format!("[daemon]\nport = {port}\n"));
    let mut daemon = Daemon(
        evoiced()
            .arg("serve")
            .arg("--config")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_ready(port);
    let key = format!("{}==", "A".repeat(22));
    let mut socket = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
    std::io::Write::write_all(
        &mut socket,
        format!(
            "GET /events HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let mut head = [0u8; 12];
    socket.read_exact(&mut head).unwrap();
    assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    let started = Instant::now();
    let code = daemon.stop();
    std::fs::remove_file(&path).ok();
    assert_eq!(
        code,
        Some(0),
        "the daemon did not stop on SIGTERM with a subscriber attached"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}
