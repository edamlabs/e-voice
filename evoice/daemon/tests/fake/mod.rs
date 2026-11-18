#![allow(dead_code)]

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    Clear,
    Finish,
}

pub type Log = Arc<Mutex<Vec<Ev>>>;

/// The same events with the instant each happened.
pub type Stamps = Arc<Mutex<Vec<(Instant, Ev)>>>;

pub fn events(log: &Log) -> Vec<Ev> {
    log.lock().unwrap().clone()
}

/// A device that, after each push, reports audio left for `drain_polls` calls of `pending`, then none.
#[derive(Debug)]
pub struct FakePlayback {
    log: Log,
    stamps: Stamps,
    polls: Mutex<u32>,
    drain_polls: u32,
    tail_polls: u32,
    fail_pushes: bool,
    dropped: Arc<AtomicBool>,
}

impl FakePlayback {
    fn record(&self, event: Ev) {
        self.log.lock().unwrap().push(event.clone());
        self.stamps.lock().unwrap().push((Instant::now(), event));
    }
}

impl Playback for FakePlayback {
    fn push(&self, samples: &[f32], rate: NonZeroU32) -> Result<(), DeviceError> {
        self.record(Ev::Push {
            rate: rate.get(),
            len: samples.len(),
            first: samples.first().copied().unwrap_or_default(),
        });
        if self.fail_pushes {
            return Err(DeviceError::Output);
        }
        *self.polls.lock().unwrap() = self.drain_polls;
        Ok(())
    }

    fn finish(&self) -> Result<(), DeviceError> {
        self.record(Ev::Finish);
        *self.polls.lock().unwrap() += self.tail_polls;
        Ok(())
    }

    fn clear(&self) {
        *self.polls.lock().unwrap() = 0;
        self.record(Ev::Clear);
    }

    fn pending(&self) -> f64 {
        let mut polls = self.polls.lock().unwrap();
        if *polls > 0 {
            *polls -= 1;
            self.record(Ev::Playing);
            0.01
        } else {
            self.record(Ev::Silent);
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
    pub stamps: Stamps,
    pub opens: Arc<AtomicUsize>,
    pub dropped: Arc<AtomicBool>,
    pub hold_open: Arc<AtomicBool>,
    drain_polls: u32,
    fail_first: usize,
    pub tail_polls: u32,
    pub fail_pushes: bool,
}

impl Device {
    pub fn new(drain_polls: u32, fail_first: usize) -> Self {
        Self {
            log: Log::default(),
            stamps: Stamps::default(),
            opens: Arc::default(),
            dropped: Arc::default(),
            hold_open: Arc::default(),
            drain_polls,
            fail_first,
            tail_polls: 0,
            fail_pushes: false,
        }
    }

    pub fn opener(&self) -> impl FnMut() -> Result<FakePlayback, DeviceError> + Send + 'static {
        let hold = Arc::clone(&self.hold_open);
        let (log, stamps, opens, dropped) = (
            Arc::clone(&self.log),
            Arc::clone(&self.stamps),
            Arc::clone(&self.opens),
            Arc::clone(&self.dropped),
        );
        let (drain_polls, fail_first, tail_polls, fail_pushes) =
            (self.drain_polls, self.fail_first, self.tail_polls, self.fail_pushes);
        move || {
            let attempt = opens.fetch_add(1, Ordering::SeqCst);
            while hold.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            (attempt >= fail_first)
                .then(|| FakePlayback {
                    log: Arc::clone(&log),
                    stamps: Arc::clone(&stamps),
                    polls: Mutex::new(0),
                    drain_polls,
                    tail_polls,
                    fail_pushes,
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
    pub period: Option<Duration>,
    pub cancels: Arc<AtomicUsize>,
    pub cancel_error: Option<SpeechError>,
    pub cancel_delay: Duration,
}

impl FakeSpeech {
    pub fn new(script: impl Fn(&SpeechRequest) -> Vec<Result<Frame, SpeechError>> + Send + Sync + 'static) -> Self {
        Self {
            script: Arc::new(script),
            requests: Arc::default(),
            gate: None,
            fail_say: None,
            panic_say: false,
            period: None,
            cancels: Arc::default(),
            cancel_error: None,
            cancel_delay: Duration::ZERO,
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
    period: Option<Duration>,
    cancels: Arc<AtomicUsize>,
    cancel_error: Option<SpeechError>,
    cancel_delay: Duration,
}

impl Utterance for FakeUtterance {
    async fn frame(&mut self) -> Option<Result<Frame, SpeechError>> {
        if let Some(period) = self.period {
            tokio::time::sleep(period).await;
        }
        self.frames.pop_front()
    }

    async fn cancel(&mut self) -> Result<(), SpeechError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.cancel_delay).await;
        self.cancel_error.clone().map_or(Ok(()), Err)
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
            period: self.period,
            cancels: Arc::clone(&self.cancels),
            cancel_error: self.cancel_error.clone(),
            cancel_delay: self.cancel_delay,
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

/// Frames that keep coming: `count` frames of `marker`, spaced by the fake's period.
pub fn long(count: usize, marker: f32) -> FakeSpeech {
    let mut speech = FakeSpeech::new(move |_| (0..count).map(|_| Ok(frame(marker, 4))).collect());
    speech.period = Some(Duration::from_millis(80));
    speech
}

/// The audio thread records a clear after `stop` has answered; waits for it, up to a second.
pub async fn cleared(device: &Device) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if events(&device.log).contains(&Ev::Clear) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    false
}
