use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use e_voice_daemon::config::talk::ClaudeConfig;
use e_voice_daemon::talk::agent::base::{AgentError, AgentEvent, Turn};
use e_voice_daemon::talk::agent::claude::{ClaudeAgent, ClaudeTurn};

pub const INIT_OK: &str = include_str!("fixtures/init_ok.jsonl");
pub const INIT_AUTO: &str = include_str!("fixtures/init_auto.jsonl");
pub const INIT_NONE: &str = include_str!("fixtures/init_none.jsonl");
pub const INIT_FAILED: &str = include_str!("fixtures/init_failed.jsonl");
pub const INIT_TWO: &str = include_str!("fixtures/init_two.jsonl");
pub const INIT_WRONG: &str = include_str!("fixtures/init_wrong_name.jsonl");
pub const TEXT_BEFORE_INIT: &str = include_str!("fixtures/text_before_init.jsonl");
pub const STATUS: &str = include_str!("fixtures/status.jsonl");
pub const DELTAS: &str = include_str!("fixtures/turn_deltas.jsonl");
pub const SUCCESS: &str = include_str!("fixtures/result_success.jsonl");
pub const ABORTED: &str = include_str!("fixtures/result_aborted.jsonl");
pub const ERROR_DURING: &str = include_str!("fixtures/result_error_during.jsonl");
pub const MAX_TURNS: &str = include_str!("fixtures/result_max_turns.jsonl");
pub const INTERRUPT_REPLY: &str = include_str!("fixtures/interrupt_reply.jsonl");

/// The conversation secret the fake sessions are started with.
pub const TOKEN: &str = "tok-test-0123456789";

pub const PLACEHOLDER: &str = "00000000-0000-4000-8000-000000000000";

/// The fake `claude`: records its argv, environment and every stdin line in the directory it runs in, and plays
/// the files it finds there. `on_start[.N]` at startup; `on_user[.N[.K]]` for the K-th user line of the N-th spawn;
/// `on_interrupt[.N]` for an interrupt. A played file's lines are written as they are, except the directives
/// `~SLEEP s`, `~PART text` (no newline), `~CAT file`, `~ERR text` (a line on stderr), `~CATERR file`, `~CLOSEIN` (close stdin once the file is played, then stay alive) and `~EXIT`. `grandchild` makes it start a sleeping
/// process of its own; `trap_term` makes it ignore SIGTERM.
const SCRIPT: &str = r#"#!/bin/sh
D=$(pwd)
n=$(cat "$D/spawns" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$D/spawns"
printf '%s\n' "$@" > "$D/argv.$n"
env > "$D/env.$n"
[ -f "$D/trap_term" ] && trap '' TERM
if [ -f "$D/grandchild" ]; then
  sleep 300 > /dev/null 2>&1 < /dev/null &
  echo $! > "$D/grandchild.$n"
fi
play() {
  [ -f "$1" ] || return 1
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      '~SLEEP '*) sleep "${line#'~SLEEP '}" ;;
      '~PART '*) printf '%s' "${line#'~PART '}" ;;
      '~CAT '*) cat "$D/${line#'~CAT '}" ;;
      '~ERR '*) printf '%s\n' "${line#'~ERR '}" >&2 ;;
      '~CATERR '*) cat "$D/${line#'~CATERR '}" >&2 ;;
      '~CLOSEIN') CLOSE=1 ;;
      '~EXIT') exit 0 ;;
      *) printf '%s\n' "$line" ;;
    esac
  done < "$1"
  return 0
}
play "$D/on_start.$n" || play "$D/on_start"
u=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$D/stdin.$n"
  case "$line" in
    *'"subtype":"interrupt"'*) play "$D/on_interrupt.$n" || play "$D/on_interrupt" ;;
    *)
      u=$((u + 1))
      play "$D/on_user.$n.$u" || play "$D/on_user.$n" || play "$D/on_user"
      if [ -n "$CLOSE" ]; then
        exec 0<&-
        sleep 30
        exit 0
      fi
      ;;
  esac
done
"#;

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// The fake, written once per test process before any test spawns it: no other thread can be forking while the
/// file is open for writing, so the executable is never busy.
fn script() -> &'static Path {
    ROOT.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("evoice-agent-fake");
        fs::create_dir_all(&root).unwrap();
        let pending = root.join(format!("claude.{}", std::process::id()));
        fs::write(&pending, SCRIPT).unwrap();
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&pending, root.join("claude")).unwrap();
        root
    })
}

pub struct Rig {
    pub dir: PathBuf,
    pub agent: ClaudeAgent,
}

impl Rig {
    /// A fake in its own directory with `files` in it, and an agent that runs it there.
    pub fn new(files: &[(&str, String)]) -> Self {
        Self::with(files, |_| {})
    }

    pub fn with(files: &[(&str, String)], tweak: impl FnOnce(&mut ClaudeConfig)) -> Self {
        let bin = script().join("claude");
        let dir = script().join(format!(
            "run-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }
        let mut config = ClaudeConfig {
            bin: bin.to_string_lossy().into_owned(),
            cwd: Some(dir.clone()),
            model: Some("sonnet".to_owned()),
            ..ClaudeConfig::default()
        };
        tweak(&mut config);
        let agent = ClaudeAgent::new(
            config,
            "http://127.0.0.1:1/mcp".to_owned(),
            "PROMPT".to_owned(),
            TOKEN.to_owned(),
        );
        Self { dir, agent }
    }

    #[allow(clippy::unused_self)]
    pub fn bin(&self) -> String {
        script().join("claude").to_string_lossy().into_owned()
    }

    pub fn read(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }

    pub fn spawns(&self) -> usize {
        self.read("spawns").trim().parse().unwrap_or(0)
    }

    pub fn argv(&self, spawn: usize) -> Vec<String> {
        self.read(&format!("argv.{spawn}")).lines().map(str::to_owned).collect()
    }

    pub fn stdin(&self, spawn: usize) -> Vec<String> {
        self.read(&format!("stdin.{spawn}"))
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn pid(&self, file: &str) -> Option<u32> {
        self.read(file).trim().parse().ok()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).ok();
    }
}

/// Fixture lines joined, each with its newline.
pub fn lines(parts: &[&str]) -> String {
    parts.concat()
}

/// Whether the process is alive: it has a `/proc` entry that is not a zombie.
pub fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .is_some_and(|zombie| !zombie)
}

/// Waits up to `patience` for the process to be gone.
pub async fn gone(pid: u32, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !alive(pid)
}

/// Every event of a turn up to its end, and the error that ended it, if one did.
pub async fn drain(turn: &mut ClaudeTurn) -> (Vec<AgentEvent>, Option<AgentError>) {
    let mut events = Vec::new();
    while let Some(next) = turn.next().await {
        match next {
            Ok(event) => events.push(event),
            Err(error) => return (events, Some(error)),
        }
    }
    (events, None)
}

pub fn deltas(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Delta(text) => Some(text.clone()),
            AgentEvent::Done(_) => None,
        })
        .collect()
}
