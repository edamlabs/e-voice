use std::io::Read;
use std::process::{Child, Command, Stdio};

use crate::http::{free_port, request, wait_ready};

fn evoiced() -> Command {
    Command::new(env!("CARGO_BIN_EXE_evoiced"))
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
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
    let daemon = Daemon(
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
    drop(daemon);
    std::fs::remove_file(&path).ok();
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
