use std::io::Write;
use std::process::{Child, Command, Stdio};

use crate::fake::{Ending, Fake};

struct Run {
    child: Child,
}

impl Run {
    fn start(fake: &Fake, args: &[&str]) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_ecli"))
            .args(["voice", "add"])
            .args(args)
            .args(["--url", &fake.url()])
            .env_remove("ECLI_DAEMON_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self { child }
    }

    fn enter(&mut self) {
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn interrupt(&self) {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn end(self) -> (Option<i32>, String, String) {
        let output = self.child.wait_with_output().unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

fn paths(fake: &Fake) -> Vec<String> {
    fake.requests()
        .iter()
        .map(|seen| format!("{} {}", seen.method, seen.path))
        .collect()
}

#[test]
fn test_one_enter_starts_the_next_finishes_then_it_prints_what_was_learned_and_says_the_sample() {
    let fake = Fake::start(Ending::Learned);
    let mut run = Run::start(&fake, &["ana"]);
    fake.until("/voices/script");
    run.enter();
    fake.until("/voices");
    run.enter();
    let (code, stdout, stderr) = run.end();
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        paths(&fake),
        [
            "GET /voices/script",
            "POST /voices",
            "POST /voices/finish",
            "POST /speak"
        ]
    );
    let requests = fake.requests();
    assert_eq!(
        requests[1].json(),
        serde_json::json!({"id": "ana", "text": "Lee esto en español.", "silence": null, "max": null, "replace": false})
    );
    assert_eq!(
        requests[3].json(),
        serde_json::json!({"text": "Hola, voz nueva.", "voice": "ana", "lang": "es"})
    );
    assert_eq!(
        stdout,
        "Lee esto en español.\n\nlearned ana (2.5 s)\ntext: lo leído\ntranscript: lo oído\n"
    );
    assert!(stderr.contains("Enter para empezar"), "{stderr}");
}

#[test]
fn test_the_options_reach_the_daemon_verbatim_and_the_language_chooses_the_script_and_the_sample() {
    let fake = Fake::start(Ending::Learned);
    let mut run = Run::start(
        &fake,
        &["ana", "--lang", "en", "--silence", "1s", "--max", "20s", "--replace"],
    );
    fake.until("/voices/script");
    run.enter();
    fake.until("/voices");
    run.enter();
    let (code, stdout, stderr) = run.end();
    assert_eq!(code, Some(0), "{stderr}");
    let requests = fake.requests();
    assert_eq!(requests[0].path, "/voices/script?lang=en");
    assert_eq!(
        requests[1].json(),
        serde_json::json!({"id": "ana", "text": "Read this in English.", "silence": "1s", "max": "20s", "replace": true})
    );
    assert_eq!(
        requests[3].json(),
        serde_json::json!({"text": "Hello, new voice.", "voice": "ana", "lang": "en"})
    );
    assert!(stdout.starts_with("Read this in English.\n"), "{stdout}");
    assert!(stderr.contains("Enter to start reading"), "{stderr}");
}

#[test]
fn test_a_given_text_is_shown_and_sent_instead_of_the_script() {
    let fake = Fake::start(Ending::Learned);
    let mut run = Run::start(&fake, &["ana", "--text", "mi propio texto"]);
    fake.until("/voices/script");
    run.enter();
    fake.until("/voices");
    run.enter();
    let (code, stdout, _) = run.end();
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with("mi propio texto\n"), "{stdout}");
    assert_eq!(fake.requests()[1].json()["text"], "mi propio texto");
}

#[test]
fn test_a_refusal_prints_the_daemons_error_exits_1_and_says_no_sample() {
    let fake = Fake::start(Ending::Exists);
    let mut run = Run::start(&fake, &["ana"]);
    fake.until("/voices/script");
    run.enter();
    fake.until("/voices");
    run.enter();
    let (code, stdout, stderr) = run.end();
    assert_eq!(code, Some(1));
    let last = stderr.lines().last().unwrap();
    assert_eq!(last, r#"ecli: daemon answered 409: the voice "ana" exists"#);
    assert!(!stdout.contains("learned"), "{stdout}");
    assert!(!paths(&fake).contains(&"POST /speak".to_owned()));
}

#[test]
fn test_ctrl_c_while_recording_asks_the_daemon_to_stop_and_says_no_sample() {
    let fake = Fake::start(Ending::Stopped);
    let mut run = Run::start(&fake, &["ana"]);
    fake.until("/voices/script");
    run.enter();
    fake.until("/voices");
    std::thread::sleep(std::time::Duration::from_millis(300));
    run.interrupt();
    fake.until("/stop");
    let (code, _, stderr) = run.end();
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(
        stderr.lines().last().unwrap(),
        "ecli: daemon answered 409: the recording was stopped"
    );
    let paths = paths(&fake);
    assert!(paths.contains(&"POST /stop".to_owned()), "{paths:?}");
    assert!(!paths.contains(&"POST /speak".to_owned()), "{paths:?}");
    assert!(!paths.contains(&"POST /voices/finish".to_owned()), "{paths:?}");
}

#[test]
fn test_a_daemon_that_is_not_there_is_a_cannot_reach_error() {
    let port = {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let output = Command::new(env!("CARGO_BIN_EXE_ecli"))
        .args(["voice", "add", "ana", "--url", &format!("http://127.0.0.1:{port}")])
        .env_remove("ECLI_DAEMON_URL")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("ecli: ") && stderr.contains(&port.to_string()),
        "{stderr}"
    );
}
