use std::fmt::{self, Debug};
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::{Pin, pin};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use futures_util::future::BoxFuture;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::config::talk::TalkConfig;
use crate::schema::event::{DaemonEvent, TalkState};
use crate::speech::base::SpeechRequest;
use crate::talk::agent::base::{Agent, AgentError, AgentEvent, Turn};
use crate::talk::answer::Answer;
use crate::talk::approval::{ApprovalInbox, Approvals, DeskItem, Pending, Verdict};
use crate::talk::chunker::SentenceChunker;
use crate::talk::notice::TalkNotice;
use crate::workflow::listen::{Listen, ListenError, ListenLease};
use crate::workflow::queue::{Halted, Queued, SpeakError, SpeechQueue};
use crate::workflow::record::Record;

/// Requests refused without asking, in a row, before the conversation says so aloud: the first ones the
/// agent rephrases from the reason it got.
const REFUSALS: usize = 3;

/// The longest an ended conversation may take to say its last notice and close its agent before it is
/// aborted.
const ENDING: Duration = Duration::from_secs(10);

/// The longest a notice may take to be queued and played before it is cut and the conversation goes on: a
/// notice never parks it, whatever the speech service does.
const NOTICE: Duration = Duration::from_secs(5);

/// The longest a halt waits for the conversation to take it: a door that stops the speech again after it
/// never hangs on a conversation stuck elsewhere.
const HALTING: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TalkError {
    #[error("a voice conversation is already running")]
    Running,
    #[error("the microphone is held by a listen")]
    Microphone,
}

/// What the HTTP doors need: a conversation started, ended, or told to stop what it is doing. The runner
/// is generic over its agent; this seam keeps that type out of the state every handler shares.
pub trait Talk: Debug + Send + Sync {
    /// Starts a conversation, listening at once.
    fn start(&self) -> BoxFuture<'_, Result<TalkState, TalkError>>;

    /// Ends the conversation, if one runs; nothing to do otherwise.
    fn end(&self) -> BoxFuture<'_, ()>;

    /// «Enough»: the agent's turn is cancelled and the conversation listens again. While it listens,
    /// nothing happens. From the moment this returns, nothing of the halted turn can be queued any more;
    /// the speech itself is stopped by whoever calls this, before and once more after. The returned wait
    /// tells the conversation, and resolves once it holds the halt or after [`HALTING`]; it is sent at once
    /// whenever it can be, so dropping the wait loses the halt only when the runner was busy. `None` only
    /// with no conversation.
    fn halt(&self) -> Option<BoxFuture<'_, ()>>;

    /// The one stop every door runs: a voice recording is cancelled and learns nothing, the speech is cut
    /// at once, the conversation halted, and once it holds the halt the speech is stopped again, so nothing
    /// of the halted turn stays queued. With no conversation, a single stop. What the first stop cut is the
    /// answer.
    ///
    /// # Errors
    /// The speech queue is gone.
    fn stop(&self) -> BoxFuture<'_, Result<Halted, SpeakError>>;
}

/// What the doors tell a running conversation; a halt carries the door's wait for it to be taken.
#[derive(Debug)]
enum Control {
    Halt(oneshot::Sender<()>),
    End,
}

impl Control {
    /// Why the conversation stops what it does. A halt is acknowledged here: the conversation's one task
    /// holds it, so no queuing of the halted turn is being driven, and every one waiting is dropped next.
    fn stop(self) -> Stop {
        match self {
            Self::Halt(taken) => {
                taken.send(()).ok();
                Stop::Halt
            }
            Self::End => Stop::End,
        }
    }
}

/// Why a step of the conversation stopped before it was done: a door said «enough», a door ended the
/// conversation, or the agent's turn ran past its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Halt,
    End,
    Bound,
}

/// What a halt does to a wait: in a question or a notice it breaks it; elsewhere the speech it silenced ends
/// the wait by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnHalt {
    Wait,
    Break,
}

/// The halts the doors have told, counted. A conversation queues a reply only under the count's lock and
/// only while the count is the one the user's words were heard under: once a halt is counted, nothing of
/// the halted turn is queued, however late its wait for room ends.
#[derive(Debug, Default)]
struct Halts(std::sync::Mutex<u64>);

impl Halts {
    fn count(&self) -> u64 {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        let mut count = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *count = count.wrapping_add(1);
    }

    /// `queue` run under the lock, unless a halt was counted since `seen`.
    fn admit<T>(&self, seen: u64, queue: impl FnOnce() -> T) -> Result<T, Stop> {
        let count = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (*count == seen).then(queue).ok_or(Stop::Halt)
    }
}

/// What comes after a user's turn: listening again, or the end, with what to say last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Listen,
    End(Option<TalkNotice>),
}

/// What a permission question heard back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    Said(Option<Answer>),
    Silent,
}

/// What a conversation is set in: its language, the agent's project, its bounds.
#[derive(Debug, Clone)]
pub struct TalkSetup {
    pub lang: Lang,
    pub cwd: PathBuf,
    pub config: TalkConfig,
}

/// One running conversation: the microphone lease, the approval desk and the agent are its own until it
/// ends; the speech queue and the events are shared.
struct Conversation<A: Agent> {
    agent: A,
    listen: Arc<dyn Listen>,
    lease: ListenLease,
    speech: SpeechQueue,
    inbox: ApprovalInbox,
    controls: mpsc::Receiver<Control>,
    events: broadcast::Sender<DaemonEvent>,
    setup: TalkSetup,
    last: Option<Queued>,
    refused: usize,
    halts: Arc<Halts>,
    seen: u64,
}

impl<A: Agent> Conversation<A> {
    // ##### PRIVATE #####

    /// `work`, unless a door says otherwise first or `until` passes; the work stays pinned and can be raced
    /// again. Every wait of the conversation goes through here, so none of them can park it.
    async fn run_race<F: Future>(
        controls: &mut mpsc::Receiver<Control>,
        work: Pin<&mut F>,
        until: Option<Instant>,
    ) -> Result<F::Output, Stop> {
        let deadline = until.unwrap_or_else(Instant::now);
        tokio::select! {
            biased;
            control = controls.recv() => Err(control.map_or(Stop::End, Control::stop)),
            () = tokio::time::sleep_until(deadline), if until.is_some() => Err(Stop::Bound),
            done = work => Ok(done),
        }
    }

    fn run_show(&self, state: TalkState) {
        self.events.send(DaemonEvent::Talk { state }).ok();
    }

    /// Queues `text` to be said after everything before it, waiting for room in the queue — a wait that a
    /// door or `until` still breaks: a full queue behind a dead speech service never parks the
    /// conversation. Once room is held, a halt counted since the user spoke drops the text instead. A
    /// refusal by the queue is logged, never fatal.
    async fn run_say(&mut self, text: String, until: Option<Instant>) -> Result<(), Stop> {
        let request = SpeechRequest {
            text,
            voice: None,
            lang: Some(self.setup.lang),
        };
        let reserving = pin!(self.speech.reserve());
        let slot = Self::run_race(&mut self.controls, reserving, until).await?;
        let queued = self
            .halts
            .admit(self.seen, || slot.and_then(|slot| slot.queue(request)))?;
        match queued {
            Ok(queued) => self.last = Some(queued),
            Err(error) => tracing::warn!(%error, "talk.say.refused"),
        }
        Ok(())
    }

    /// Waits until everything the conversation queued has played, unless a door says otherwise first or
    /// `until` passes; a halt breaks the wait only when `on_halt` says so.
    async fn run_settle(&mut self, until: Option<Instant>, on_halt: OnHalt) -> Result<(), Stop> {
        let Some(last) = self.last.take() else {
            return Ok(());
        };
        let mut done = pin!(last.done());
        loop {
            match Self::run_race(&mut self.controls, done.as_mut(), until).await {
                Ok(_) => return Ok(()),
                Err(Stop::Halt) if on_halt == OnHalt::Wait => {}
                Err(stop) => return Err(stop),
            }
        }
    }

    /// One listen under the conversation's lease; a halt breaks it only when `on_halt` says so.
    async fn run_hear(&mut self, timeout: Duration, on_halt: OnHalt) -> Result<Result<String, ListenError>, Stop> {
        let listening = self.listen.listen_leased(&self.lease, timeout);
        let mut listening = pin!(listening);
        loop {
            match Self::run_race(&mut self.controls, listening.as_mut(), None).await {
                Ok(heard) => return Ok(heard.map(|heard| heard.text)),
                Err(Stop::Halt) if on_halt == OnHalt::Wait => {}
                Err(stop) => return Err(stop),
            }
        }
    }

    /// The listening step: everything queued is played first (half-duplex), then the user's turn is heard.
    async fn run_listen(&mut self) -> Next {
        if self.run_settle(None, OnHalt::Wait).await.is_err() {
            return Next::End(None);
        }
        self.run_show(TalkState::Listening);
        let Ok(heard) = self.run_hear(self.setup.config.idle, OnHalt::Wait).await else {
            return Next::End(None);
        };
        self.seen = self.halts.count();
        match heard {
            Ok(text) if TalkNotice::bye(&text, self.setup.lang) => Next::End(Some(TalkNotice::Goodbye)),
            Ok(text) => self.run_turn(text).await,
            Err(ListenError::Timeout(_)) => Next::End(Some(TalkNotice::Goodbye)),
            Err(error) => {
                tracing::warn!(%error, "talk.listen.failed");
                Next::End(Some(TalkNotice::Failed))
            }
        }
    }

    /// One agent turn, from the user's text to its last sentence queued, with its questions asked on the
    /// way. A halt or the turn's bound cancels it; an end cancels it and ends the conversation.
    async fn run_turn(&mut self, text: String) -> Next {
        self.refused = 0;
        let mut turn = match self.agent.turn(text).await {
            Ok(turn) => turn,
            Err(error) => return self.run_failure(error).await,
        };
        self.run_show(TalkState::Thinking);
        let mut chunker = SentenceChunker::default();
        let mut speaking = false;
        let until = Instant::now().checked_add(self.setup.config.turn);
        let deadline = until.unwrap_or_else(Instant::now);
        loop {
            tokio::select! {
                biased;
                control = self.controls.recv() => {
                    let stop = control.map_or(Stop::End, Control::stop);
                    turn.cancel().await.ok();
                    return self.run_stopped(stop).await;
                }
                item = self.inbox.recv() => match item {
                    Some(DeskItem::Question(pending)) => {
                        if let Err(stop) = self.run_ask(pending).await {
                            turn.cancel().await.ok();
                            return if stop == Stop::Halt { Next::Listen } else { Next::End(None) };
                        }
                        self.run_show(TalkState::Thinking);
                        speaking = false;
                    }
                    Some(DeskItem::Refused(reason)) => {
                        if let Err(stop) = self.run_refused(&reason, until).await {
                            turn.cancel().await.ok();
                            return self.run_stopped(stop).await;
                        }
                    }
                    None => {}
                },
                event = turn.next() => match event {
                    Some(Ok(AgentEvent::Delta(text))) => {
                        let pieces = chunker.push(&text);
                        if let Err(stop) = self.run_pieces(pieces, until, &mut speaking).await {
                            turn.cancel().await.ok();
                            return self.run_stopped(stop).await;
                        }
                    }
                    Some(Ok(AgentEvent::Done(_))) | None => {
                        let pieces = chunker.finish();
                        return match self.run_pieces(pieces, until, &mut speaking).await {
                            Ok(()) => Next::Listen,
                            Err(stop) => self.run_stopped(stop).await,
                        };
                    }
                    Some(Err(error)) => return self.run_failure(error).await,
                },
                () = tokio::time::sleep_until(deadline), if until.is_some() => {
                    turn.cancel().await.ok();
                    return self.run_stopped(Stop::Bound).await;
                }
            }
        }
    }

    /// What follows a turn a door or its bound stopped: listening again, with the speech cut and a word
    /// said when the bound did it; the end, when a door ended the conversation.
    async fn run_stopped(&mut self, stop: Stop) -> Next {
        match stop {
            Stop::Halt => Next::Listen,
            Stop::End => Next::End(None),
            Stop::Bound => {
                self.speech.stop().await.ok();
                self.last = None;
                self.run_onward(TalkNotice::TooLong).await
            }
        }
    }

    /// The turn's pieces queued in order; the conversation shows it speaks once the first is queued.
    async fn run_pieces(
        &mut self,
        pieces: Vec<String>,
        until: Option<Instant>,
        speaking: &mut bool,
    ) -> Result<(), Stop> {
        for piece in pieces {
            self.run_say(piece, until).await?;
            if !*speaking {
                *speaking = true;
                self.run_show(TalkState::Speaking);
            }
        }
        Ok(())
    }

    /// `notice` said after everything queued before it, the whole of it played within [`NOTICE`]: past it,
    /// or on a halt, the speech is cut and the conversation goes on. Only an end stops it.
    async fn run_notice(&mut self, notice: TalkNotice) -> Result<(), Stop> {
        let until = Instant::now().checked_add(NOTICE);
        let text = notice.text(self.setup.lang).to_owned();
        let said = async {
            self.run_settle(until, OnHalt::Break).await?;
            self.run_say(text, until).await?;
            self.run_settle(until, OnHalt::Break).await
        }
        .await;
        match said {
            Ok(()) => Ok(()),
            Err(Stop::End) => Err(Stop::End),
            Err(Stop::Halt | Stop::Bound) => {
                self.last = None;
                self.speech.stop().await.ok();
                Ok(())
            }
        }
    }

    /// `notice` said, then listening again; the end, when a door ends the conversation meanwhile.
    async fn run_onward(&mut self, notice: TalkNotice) -> Next {
        self.run_notice(notice).await.map_or(Next::End(None), |()| Next::Listen)
    }

    /// A request refused without asking: counted, and said aloud on the [`REFUSALS`]th in a row.
    async fn run_refused(&mut self, reason: &str, until: Option<Instant>) -> Result<(), Stop> {
        tracing::info!(%reason, "talk.refused.unasked");
        self.refused = self.refused.saturating_add(1);
        if self.refused < REFUSALS {
            return Ok(());
        }
        self.refused = 0;
        self.run_say(TalkNotice::Unreadable.text(self.setup.lang).to_owned(), until)
            .await
    }

    /// A permission question: said after everything queued, then heard; an unclear answer is asked once
    /// more. Whatever stops it first, the question is answered: a halt or an end is a refusal.
    async fn run_ask(&mut self, pending: Pending) -> Result<(), Stop> {
        self.run_show(TalkState::Asking);
        self.refused = 0;
        let (verdict, outcome) = match self.run_question(&pending.question).await {
            Ok(verdict) => (verdict, Ok(())),
            Err(Stop::Halt | Stop::Bound) => (Verdict::Deny("stopped".to_owned()), Err(Stop::Halt)),
            Err(Stop::End) => (Verdict::Deny("the voice conversation ended".to_owned()), Err(Stop::End)),
        };
        pending.answer(verdict);
        outcome
    }

    async fn run_question(&mut self, question: &str) -> Result<Verdict, Stop> {
        let first = self.run_reply(question.to_owned()).await?;
        let again = format!("{} {question}", TalkNotice::Again.text(self.setup.lang));
        let reply = match first {
            Reply::Said(None) => self.run_reply(again).await?,
            known => known,
        };
        Ok(match reply {
            Reply::Said(Some(Answer::Yes)) => Verdict::Allow,
            Reply::Said(Some(Answer::No)) => Verdict::Deny("the user said no".to_owned()),
            Reply::Said(None) => Verdict::Deny("no clear answer".to_owned()),
            Reply::Silent => Verdict::Deny("no answer heard".to_owned()),
        })
    }

    /// `text` said and played, then the answer heard within `[daemon.talk] answer`; a halt breaks either.
    async fn run_reply(&mut self, text: String) -> Result<Reply, Stop> {
        self.run_say(text, None).await?;
        self.run_settle(None, OnHalt::Break).await?;
        let heard = self.run_hear(self.setup.config.answer, OnHalt::Break).await?;
        Ok(match heard {
            Ok(text) => Reply::Said(Answer::of(&text, self.setup.lang)),
            Err(_) => Reply::Silent,
        })
    }

    /// An agent failure: a lost process is taken up again by the next turn; a refused or unstartable
    /// agent ends the conversation, its notice said last; a failed turn is said and the conversation goes on.
    async fn run_failure(&mut self, error: AgentError) -> Next {
        tracing::warn!(%error, "talk.agent.failed");
        match error {
            AgentError::Gone => self.run_onward(TalkNotice::Lost).await,
            AgentError::Refused(_) => Next::End(Some(TalkNotice::Refused)),
            AgentError::Start(_) | AgentError::Protocol(_) => Next::End(Some(TalkNotice::Failed)),
            AgentError::Agent(_) => self.run_onward(TalkNotice::Failed).await,
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// The conversation from its first listen to its end; whatever ends it, the last notice is said, the
    /// agent closed, and the lease, the desk and the doors given back before it shows idle: idle promises
    /// that a new conversation starts.
    async fn run(mut self) {
        let notice = loop {
            match self.run_listen().await {
                Next::Listen => {}
                Next::End(notice) => break notice,
            }
        };
        if let Some(notice) = notice {
            self.run_notice(notice).await.ok();
        }
        self.agent.close().await;
        let Self {
            lease,
            inbox,
            controls,
            events,
            ..
        } = self;
        drop((lease, inbox, controls));
        events.send(DaemonEvent::Talk { state: TalkState::Idle }).ok();
    }
}

/// A conversation that runs, and how to reach it: it is over once its doors are closed, which it does before
/// it shows idle. Dropping it aborts the conversation.
struct Running {
    task: JoinHandle<()>,
    controls: mpsc::Sender<Control>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The voice conversation: one at a time, each with a fresh agent from `make` (handed the conversation's
/// secret), the microphone held for its whole length, and the approval desk its own. `record` is the voice
/// recorder, held only so the one stop cancels it too.
pub struct TalkRunner<A, F> {
    make: F,
    listen: Arc<dyn Listen>,
    speech: SpeechQueue,
    record: Arc<dyn Record>,
    approvals: Approvals,
    events: broadcast::Sender<DaemonEvent>,
    setup: TalkSetup,
    running: Mutex<Option<Running>>,
    halts: Arc<Halts>,
    agent: PhantomData<fn() -> A>,
}

impl<A, F> Debug for TalkRunner<A, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TalkRunner")
            .field("setup", &self.setup)
            .finish_non_exhaustive()
    }
}

impl<A, F> TalkRunner<A, F>
where
    A: Agent,
    F: Fn(String) -> A + Send + Sync + 'static,
{
    // ##### PRIVATE #####

    /// `control` sent to the running conversation, waiting for the lock when `controls` was not taken and
    /// for room in the channel; `None` once there is no conversation.
    async fn halt_send(&self, controls: Option<mpsc::Sender<Control>>, control: Control) -> Option<()> {
        let controls = match controls {
            Some(controls) => controls,
            None => self.running.lock().await.as_ref()?.controls.clone(),
        };
        controls.send(control).await.ok()
    }

    // ##########################################################

    // ##### PUBLIC #####

    #[must_use]
    pub fn new(
        make: F,
        listen: Arc<dyn Listen>,
        speech: SpeechQueue,
        record: Arc<dyn Record>,
        approvals: Approvals,
        events: broadcast::Sender<DaemonEvent>,
        setup: TalkSetup,
    ) -> Self {
        Self {
            make,
            listen,
            speech,
            record,
            approvals,
            events,
            setup,
            running: Mutex::new(None),
            halts: Arc::default(),
            agent: PhantomData,
        }
    }
}

impl<A, F> Talk for TalkRunner<A, F>
where
    A: Agent,
    F: Fn(String) -> A + Send + Sync + 'static,
{
    /// Refused while a conversation runs, or while a listen holds the microphone.
    fn start(&self) -> BoxFuture<'_, Result<TalkState, TalkError>> {
        Box::pin(async move {
            let mut running = self.running.lock().await;
            if running.as_ref().is_some_and(|running| !running.controls.is_closed()) {
                return Err(TalkError::Running);
            }
            let lease = self.listen.lease().ok_or(TalkError::Microphone)?;
            let token = uuid::Uuid::new_v4().simple().to_string();
            let (lang, cwd) = (self.setup.lang, self.setup.cwd.clone());
            let desk = self.approvals.open(token.clone(), lang, cwd);
            let inbox = desk.ok_or(TalkError::Running)?;
            let (controls, orders) = mpsc::channel(4);
            let conversation = Conversation {
                agent: (self.make)(token),
                listen: Arc::clone(&self.listen),
                lease,
                speech: self.speech.clone(),
                inbox,
                controls: orders,
                events: self.events.clone(),
                setup: self.setup.clone(),
                last: None,
                refused: 0,
                halts: Arc::clone(&self.halts),
                seen: self.halts.count(),
            };
            *running = Some(Running {
                task: tokio::spawn(conversation.run()),
                controls,
            });
            Ok(TalkState::Listening)
        })
    }

    /// Tells the conversation to end and waits for it, up to [`ENDING`]; past that it is aborted, and waited
    /// for again. Once this returns, the conversation queues nothing more.
    fn end(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let running = self.running.lock().await.take();
            if let Some(mut running) = running {
                running.controls.send(Control::End).await.ok();
                if tokio::time::timeout(ENDING, &mut running.task).await.is_err() {
                    running.task.abort();
                    (&mut running.task).await.ok();
                }
            }
        })
    }

    /// Counted first, so the fence stands before anything else; then sent at once when the slot and the
    /// channel are free, else by waiting for them inside the bound. Busy is never taken for absent.
    fn halt(&self) -> Option<BoxFuture<'_, ()>> {
        self.halts.bump();
        let (taken, held) = oneshot::channel();
        let pending = match self.running.try_lock() {
            Ok(running) => {
                let controls = running.as_ref()?.controls.clone();
                match controls.try_send(Control::Halt(taken)) {
                    Ok(()) => None,
                    Err(TrySendError::Full(control)) => Some((Some(controls), control)),
                    Err(TrySendError::Closed(_)) => return None,
                }
            }
            Err(_) => Some((None, Control::Halt(taken))),
        };
        Some(Box::pin(async move {
            let told = async {
                if let Some((controls, control)) = pending {
                    self.halt_send(controls, control).await?;
                }
                held.await.ok()
            };
            if tokio::time::timeout(HALTING, told).await.is_err() {
                tracing::warn!("talk.halt.unacked");
            }
        }))
    }

    fn stop(&self) -> BoxFuture<'_, Result<Halted, SpeakError>> {
        Box::pin(async move {
            self.record.cancel();
            let stopped = self.speech.stop().await;
            if let Some(halted) = self.halt() {
                halted.await;
                self.speech.stop().await.ok();
            }
            stopped
        })
    }
}
