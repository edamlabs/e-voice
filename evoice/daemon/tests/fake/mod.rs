#![allow(dead_code)]

use std::collections::VecDeque;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use e_voice_daemon::speech::base::{Frame, Speech, SpeechError, SpeechRequest, Utterance};
use e_voice_daemon::workflow::audio::AudioThread;
use e_voice_daemon::workflow::queue::SpeechQueue;
use e_voice_device::error::DeviceError;
use e_voice_device::playback::base::Playback;
use tokio::sync::Semaphore;

/// What the fake device saw, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    Push { rate: u32, len: usize, first: f32 },
    Playing,
    Silent,
}

pub type Log = Arc<Mutex<Vec<Ev>>>;

pub fn events(log: &Log) -> Vec<Ev> {
    log.lock().unwrap().clone()
}

/// A device that, after each push, reports audio left for `drain_polls` calls of `pending`, then none.
#[derive(Debug)]
pub struct FakePlayback {
    log: Log,
    polls: Mutex<u32>,
    drain_polls: u32,
    dropped: Arc<AtomicBool>,
}

impl Playback for FakePlayback {
    fn push(&self, samples: &[f32], rate: NonZeroU32) {
        self.log.lock().unwrap().push(Ev::Push {
            rate: rate.get(),
            len: samples.len(),
            first: samples.first().copied().unwrap_or_default(),
        });
        *self.polls.lock().unwrap() = self.drain_polls;
    }

    fn clear(&self) {}

    fn pending(&self) -> f64 {
        let mut polls = self.polls.lock().unwrap();
        if *polls > 0 {
            *polls -= 1;
            self.log.lock().unwrap().push(Ev::Playing);
            0.01
        } else {
            self.log.lock().unwrap().push(Ev::Silent);
            0.0
        }
    }
}

impl Drop for FakePlayback {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

/// The opener an `AudioThread` is given, with the handles a test watches.
pub struct Device {
    pub log: Log,
    pub opens: Arc<AtomicUsize>,
    pub dropped: Arc<AtomicBool>,
    drain_polls: u32,
    fail_first: usize,
}

impl Device {
    pub fn new(drain_polls: u32, fail_first: usize) -> Self {
        Self {
            log: Log::default(),
            opens: Arc::default(),
            dropped: Arc::default(),
            drain_polls,
            fail_first,
        }
    }

    pub fn opener(&self) -> impl FnMut() -> Result<FakePlayback, DeviceError> + Send + 'static {
        let (log, opens, dropped) = (
            Arc::clone(&self.log),
            Arc::clone(&self.opens),
            Arc::clone(&self.dropped),
        );
        let (drain_polls, fail_first) = (self.drain_polls, self.fail_first);
        move || {
            let attempt = opens.fetch_add(1, Ordering::SeqCst);
            (attempt >= fail_first)
                .then(|| FakePlayback {
                    log: Arc::clone(&log),
                    polls: Mutex::new(0),
                    drain_polls,
                    dropped: Arc::clone(&dropped),
                })
                .ok_or(DeviceError::Output)
        }
    }

    pub fn thread(&self) -> AudioThread {
        AudioThread::spawn(self.opener())
    }

    pub fn opened(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }
}

pub fn rate() -> NonZeroU32 {
    NonZeroU32::new(24_000).unwrap()
}

pub fn frame(marker: f32, len: usize) -> Frame {
    Frame {
        rate: rate(),
        samples: vec![marker; len],
    }
}

type Script = Arc<dyn Fn(&SpeechRequest) -> Vec<Result<Frame, SpeechError>> + Send + Sync>;

/// A speech service with a script per request; `gate` holds every `say` until a permit is given.
#[derive(Clone)]
pub struct FakeSpeech {
    script: Script,
    pub requests: Arc<Mutex<Vec<SpeechRequest>>>,
    pub gate: Option<Arc<Semaphore>>,
    pub fail_say: Option<SpeechError>,
    pub panic_say: bool,
}

impl FakeSpeech {
    pub fn new(script: impl Fn(&SpeechRequest) -> Vec<Result<Frame, SpeechError>> + Send + Sync + 'static) -> Self {
        Self {
            script: Arc::new(script),
            requests: Arc::default(),
            gate: None,
            fail_say: None,
            panic_say: false,
        }
    }

    /// Three frames whose samples carry the number the text spells.
    pub fn marked() -> Self {
        Self::new(|request| {
            let marker: f32 = request.text.parse().unwrap_or(-1.0);
            (0..3).map(|_| Ok(frame(marker, 4))).collect()
        })
    }

    pub fn gated(mut self) -> (Self, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        self.gate = Some(Arc::clone(&gate));
        (self, gate)
    }

    pub fn texts(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.text.clone())
            .collect()
    }
}

pub struct FakeUtterance {
    frames: VecDeque<Result<Frame, SpeechError>>,
}

impl Utterance for FakeUtterance {
    fn frame(&mut self) -> impl Future<Output = Option<Result<Frame, SpeechError>>> + Send {
        std::future::ready(self.frames.pop_front())
    }
}

impl Speech for FakeSpeech {
    type Utterance = FakeUtterance;

    async fn say(&self, request: SpeechRequest) -> Result<FakeUtterance, SpeechError> {
        assert!(!self.panic_say, "the fake speech service panics");
        self.requests.lock().unwrap().push(request.clone());
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        if let Some(error) = &self.fail_say {
            return Err(error.clone());
        }
        Ok(FakeUtterance {
            frames: (self.script)(&request).into(),
        })
    }
}

pub fn queue(speech: FakeSpeech, device: &Device) -> SpeechQueue {
    SpeechQueue::start(speech, device.thread())
}

pub fn say(text: &str) -> SpeechRequest {
    SpeechRequest {
        text: text.to_owned(),
        voice: None,
        lang: None,
    }
}

pub fn same(left: f32, right: f32) -> bool {
    (left - right).abs() < 1e-6
}
