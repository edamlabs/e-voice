use std::sync::Arc;
use std::time::{Duration, Instant};

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::Recognized;
use e_voice_daemon::schema::event::DaemonEvent;
use e_voice_daemon::talk::runner::Talk;
use tokio::sync::broadcast;

use crate::fake::{Device, FakeSpeech, Feed, TalkState, Wired, heard, talk, wired};

/// The bounds a conversation test runs with: short, because the microphone is a real thread.
pub const IDLE: Duration = Duration::from_millis(600);
pub const TURN: Duration = Duration::from_secs(3);
pub const ANSWER: Duration = Duration::from_millis(600);

pub struct Rig {
    requests: Arc<std::sync::Mutex<Vec<e_voice_daemon::speech::base::SpeechRequest>>>,
    pub w: Wired,
    pub talk: Arc<dyn Talk>,
    pub events: broadcast::Receiver<DaemonEvent>,
    pub feeds: Vec<Feed>,
    pub states: Vec<TalkState>,
}

impl Rig {
    pub fn new(speech: FakeSpeech, lang: Lang, sessions: usize) -> Self {
        Self::bounded(speech, lang, sessions, IDLE, TURN, ANSWER)
    }

    pub fn bounded(
        speech: FakeSpeech,
        lang: Lang,
        sessions: usize,
        idle: Duration,
        turn: Duration,
        answer: Duration,
    ) -> Self {
        let requests = Arc::clone(&speech.requests);
        let w = wired(speech, Device::new(0, 0), Duration::from_secs(5));
        let events = w.events.subscribe();
        let feeds = (0..sessions).map(|_| w.hearing.session()).collect();
        let talk = talk(&w, lang, idle, turn, answer);
        Self {
            requests,
            w,
            talk,
            events,
            feeds,
            states: Vec::new(),
        }
    }

    pub fn marked(sessions: usize) -> Self {
        Self::new(FakeSpeech::marked(), Lang::Es, sessions)
    }

    /// Reads every conversation state the bus carries until `state` shows up, or fails after `patience`.
    pub async fn until(&mut self, state: TalkState) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.events.recv()).await {
                Ok(Ok(DaemonEvent::Talk { state: seen })) => {
                    self.states.push(seen);
                    if seen == state {
                        return;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("the bus failed: {error:?}"),
                Err(elapsed) => panic!("never reached {state:?} ({elapsed}); saw {:?}", self.states),
            }
        }
    }

    /// Whatever states are on the bus right now.
    pub fn drain(&mut self) -> &[TalkState] {
        while let Ok(event) = self.events.try_recv() {
            if let DaemonEvent::Talk { state } = event {
                self.states.push(state);
            }
        }
        &self.states
    }

    /// The listener is open on the microphone for its `n`th listen.
    pub async fn hearing(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.w.mic.opened() >= n && self.w.mic.live().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the microphone was never opened {n} times: {}", self.w.mic.opened());
    }

    /// The user says `text` in the `n`th listen (counting from 1).
    pub async fn says(&self, n: usize, text: &str) {
        self.hearing(n).await;
        let feed = &self.feeds[n - 1];
        feed.say(Recognized::Started(std::time::Duration::ZERO));
        feed.say(Recognized::Final(heard(text), None));
    }

    /// Waits until the agent's questions have `count` answers in.
    pub async fn answered(&self, count: usize) -> Vec<crate::fake::Verdict> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.w.script.verdicts().len() < count && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.w.script.verdicts()
    }

    /// Every text the speech service was asked for, in order.
    pub fn spoken(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.text.clone())
            .collect()
    }

    /// Waits until `state` has been seen `times` times in all.
    pub async fn until_count(&mut self, state: TalkState, times: usize) {
        while self.drain().iter().filter(|seen| **seen == state).count() < times {
            let before = self.states.len();
            self.until_any().await;
            assert!(self.states.len() > before);
        }
    }

    async fn until_any(&mut self) {
        let deadline = Duration::from_secs(10);
        loop {
            match tokio::time::timeout(deadline, self.events.recv()).await {
                Ok(Ok(DaemonEvent::Talk { state })) => {
                    self.states.push(state);
                    return;
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("the bus failed: {error:?}"),
                Err(elapsed) => panic!("no more conversation events ({elapsed}); saw {:?}", self.states),
            }
        }
    }

    /// The microphone, the desk, the listener and the queue are free: a listen works again, the desk can be
    /// opened, nothing is playing, and the conversation's agent was closed `closed` times.
    pub async fn assert_free(&self, closed: usize) {
        assert!(self.w.mic.freed().await, "the microphone stayed open");
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.w.state.speech.state() != e_voice_daemon::schema::event::SpeechState::Idle
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            self.w.state.speech.state(),
            e_voice_daemon::schema::event::SpeechState::Idle,
            "the queue is still busy"
        );
        let desk = self
            .w
            .approvals
            .open("probe".to_owned(), Lang::Es, std::path::PathBuf::from("/work"));
        assert!(desk.is_some(), "the desk is still held");
        drop(desk);
        assert_eq!(
            self.w.script.closes(),
            closed,
            "the agent was not closed exactly {closed} times"
        );
        let feed = self.w.hearing.session();
        let listen = Arc::clone(&self.w.state.listen);
        let task = tokio::spawn(async move { listen.listen(Some(Duration::from_secs(5))).await });
        assert!(self.w.mic.live().await);
        feed.say(Recognized::Final(heard("de nuevo"), None));
        let heard = task
            .await
            .unwrap()
            .expect("a listen works again after the conversation");
        assert_eq!(heard.text, "de nuevo");
    }
}
