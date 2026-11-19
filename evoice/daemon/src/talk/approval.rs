use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::talk::question::{Asked, ToolQuestion};

/// The header that carries a conversation's secret on its agent's MCP connection.
pub const APPROVER: &str = "x-evoice-approver";

/// Why a request that waited behind a refused one is refused too, unasked.
pub const QUEUED: &str = "refused without asking: an earlier request was refused while this one waited";

/// The longest an approval waits for the conversation's answer. Claude Code never stops waiting on its
/// side, so this bound is the only one; past it the request is denied. It exceeds what the conversation
/// takes to ask, listen and ask once more.
const APPROVE: Duration = Duration::from_secs(60);

/// The answer to a permission request, in the words Claude Code reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
}

impl Verdict {
    /// The one text block Claude Code's prompt tool must answer with: `{"behavior": "allow"}` or
    /// `{"behavior": "deny", "message": …}`.
    #[must_use]
    pub fn behavior(&self) -> String {
        let answer = match self {
            Self::Allow => json!({"behavior": "allow"}),
            Self::Deny(message) => json!({"behavior": "deny", "message": message}),
        };
        answer.to_string()
    }
}

/// A question waiting for the conversation: what to say, and where the answer goes. Dropped unanswered, it
/// is a refusal.
#[derive(Debug)]
pub struct Pending {
    pub question: String,
    reply: oneshot::Sender<Verdict>,
}

impl Pending {
    pub fn answer(self, verdict: Verdict) {
        self.reply.send(verdict).ok();
    }
}

/// The conversation that holds the desk: the secret its agent's MCP connection carries, its language and
/// project, and where its questions go.
#[derive(Debug)]
struct Desk {
    token: String,
    lang: Lang,
    cwd: PathBuf,
    questions: mpsc::Sender<Pending>,
}

/// Where permission requests meet the voice conversation. One conversation at a time holds the desk; a
/// request from anyone but that conversation's own agent — known by the secret it was started with — is
/// refused, and so is any request while no conversation is held. Questions are asked one at a time; a
/// request that was waiting behind one the listener refused is refused too, unasked.
#[derive(Debug, Clone, Default)]
pub struct Approvals {
    desk: Arc<Mutex<Option<Desk>>>,
    turn: Arc<tokio::sync::Mutex<()>>,
    refusals: Arc<AtomicU64>,
}

/// The conversation's side of the desk: the questions to ask, one at a time. Dropping it gives the desk
/// back, and every request after that is refused.
#[derive(Debug)]
pub struct ApprovalInbox {
    questions: mpsc::Receiver<Pending>,
    desk: Arc<Mutex<Option<Desk>>>,
}

impl ApprovalInbox {
    /// The next question; `None` once nobody can ask any more.
    pub async fn recv(&mut self) -> Option<Pending> {
        self.questions.recv().await
    }
}

impl Drop for ApprovalInbox {
    fn drop(&mut self) {
        self.desk.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

impl Approvals {
    // ##### PRIVATE #####

    /// Whether two secrets are equal, comparing every byte whatever the first difference.
    fn ask_same(one: &str, other: &str) -> bool {
        let differ = one
            .bytes()
            .zip(other.bytes())
            .fold(0u8, |differ, (a, b)| differ | (a ^ b));
        one.len() == other.len() && differ == 0
    }

    /// Where a request from the holder of `token` goes, with the language and project it is asked in.
    fn ask_desk(&self, token: Option<&str>) -> Result<(mpsc::Sender<Pending>, Lang, PathBuf), Verdict> {
        let desk = self.desk.lock().unwrap_or_else(PoisonError::into_inner);
        let held = desk
            .as_ref()
            .ok_or_else(|| Verdict::Deny("no voice conversation is active".to_owned()))?;
        let own = token.is_some_and(|token| Self::ask_same(token, &held.token));
        own.then(|| (held.questions.clone(), held.lang, held.cwd.clone()))
            .ok_or_else(|| Verdict::Deny("not the voice conversation's own agent".to_owned()))
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Takes the desk for a conversation whose agent carries `token`, asking in `lang` about the project at
    /// `cwd`. `None` while another conversation holds it.
    #[must_use]
    pub fn open(&self, token: String, lang: Lang, cwd: PathBuf) -> Option<ApprovalInbox> {
        let mut desk = self.desk.lock().unwrap_or_else(PoisonError::into_inner);
        if desk.is_some() {
            return None;
        }
        let (questions, inbox) = mpsc::channel(1);
        *desk = Some(Desk {
            token,
            lang,
            cwd,
            questions,
        });
        Some(ApprovalInbox {
            questions: inbox,
            desk: Arc::clone(&self.desk),
        })
    }

    /// The answer to the agent's request to use `tool` with `input`, from the holder of `token`: asked
    /// aloud by the conversation and answered within [`APPROVE`], or refused.
    pub async fn ask(&self, token: Option<&str>, tool: &str, input: &Value) -> Verdict {
        let arrived = self.refusals.load(Ordering::Acquire);
        let (questions, lang, cwd) = match self.ask_desk(token) {
            Ok(desk) => desk,
            Err(refused) => return refused,
        };
        let question = match ToolQuestion::of(tool, input, lang, &cwd).await {
            Asked::Ask(question) => question,
            Asked::Refuse(reason) => return Verdict::Deny(reason),
        };
        let _turn = self.turn.lock().await;
        if self.refusals.load(Ordering::Acquire) != arrived {
            return Verdict::Deny(QUEUED.to_owned());
        }
        let (reply, answer) = oneshot::channel();
        let gone = || Verdict::Deny("the voice conversation ended".to_owned());
        if questions.send(Pending { question, reply }).await.is_err() {
            return gone();
        }
        let verdict = match tokio::time::timeout(APPROVE, answer).await {
            Ok(Ok(verdict)) => verdict,
            Ok(Err(_)) => gone(),
            Err(_) => Verdict::Deny("no answer in time".to_owned()),
        };
        if matches!(verdict, Verdict::Deny(_)) {
            self.refusals.fetch_add(1, Ordering::AcqRel);
        }
        verdict
    }
}
