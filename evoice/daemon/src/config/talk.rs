use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// `[daemon.talk]`: the voice conversation with an agent. It lives under `[daemon]` because the services
/// read the same file and refuse a table they do not know. `idle` is how long it waits for the user to speak
/// before it ends; `turn` how long one agent turn may run before it is cut; `answer` how long a permission
/// question waits for a yes or a no.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TalkConfig {
    #[serde(with = "humantime_serde")]
    pub idle: Duration,
    #[serde(with = "humantime_serde")]
    pub turn: Duration,
    #[serde(with = "humantime_serde")]
    pub answer: Duration,
    pub claude: ClaudeConfig,
}

impl Default for TalkConfig {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(60),
            turn: Duration::from_secs(600),
            answer: Duration::from_secs(15),
            claude: ClaudeConfig::default(),
        }
    }
}

/// `[daemon.talk.claude]`: Claude Code as the agent, run headless.
///
/// `bin` is the `claude` executable. `tools` are the only built-in tools that exist in the session:
/// reading ones run unasked, every other one is asked aloud, and a name the daemon does not know is asked
/// too. `model` picks one; Claude Code's default when absent.
///
/// `cwd` is the session's project, the daemon's own directory when absent. The voice session never loads
/// the user's settings, but it does load the project's `.claude/settings.json` at `cwd`: its deny rules
/// and hooks keep protecting, while its allow rules cannot skip a spoken yes, because the daemon asks for
/// every tool that changes something and an ask rule outranks an allow. Managed settings, an organisation's
/// policy, outrank everything the daemon passes: a tool they allow is not asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    pub bin: String,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub tools: Vec<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            bin: "claude".to_owned(),
            cwd: None,
            model: None,
            tools: ["Read", "Bash", "Edit", "Write"].map(str::to_owned).to_vec(),
        }
    }
}
