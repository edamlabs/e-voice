#![allow(dead_code, clippy::cast_precision_loss)]

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Debug;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use e_voice_core::schema::emotion::Emotion;
use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::{Heard, Hearing, HearingError, Recognized, Transcription};
use e_voice_daemon::schema::event::DaemonEvent;
use e_voice_daemon::speech::base::{Frame, Speech, SpeechError, SpeechRequest, Utterance};
use e_voice_daemon::workflow::audio::AudioThread;
use e_voice_daemon::workflow::capture::{CaptureThread, Deliver};
use e_voice_daemon::workflow::listen::{Listen, ListenError};
use e_voice_daemon::workflow::queue::SpeechQueue;
use e_voice_device::capture::base::Capture;
use e_voice_device::error::DeviceError;
use e_voice_device::playback::base::Playback;
use futures_util::future::BoxFuture;
use tokio::sync::{Semaphore, broadcast, mpsc};

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
    realtime: bool,
    ledger: Mutex<(f64, Instant)>,
    received: Arc<Mutex<Vec<f32>>>,
    max_pending: Arc<Mutex<f64>>,
}

impl FakePlayback {
    /// Seconds of audio a device playing in real time still holds.
    fn left(&self) -> f64 {
        let (value, at) = *self.ledger.lock().unwrap();
        (value - at.elapsed().as_secs_f64()).max(0.0)
    }

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
        self.received.lock().unwrap().extend_from_slice(samples);
        if self.realtime {
            let held = self.left() + samples.len() as f64 / f64::from(rate.get());
            *self.ledger.lock().unwrap() = (held, Instant::now());
            let mut max = self.max_pending.lock().unwrap();
            *max = max.max(held);
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
        *self.ledger.lock().unwrap() = (0.0, Instant::now());
        self.record(Ev::Clear);
    }

    fn pending(&self) -> f64 {
        if self.realtime {
            let left = self.left();
            self.record(if left > 0.0 { Ev::Playing } else { Ev::Silent });
            return left;
        }
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
    pub realtime: bool,
    pub received: Arc<Mutex<Vec<f32>>>,
    pub max_pending: Arc<Mutex<f64>>,
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
            realtime: false,
            received: Arc::default(),
            max_pending: Arc::default(),
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
        let (realtime, received, max_pending) =
            (self.realtime, Arc::clone(&self.received), Arc::clone(&self.max_pending));
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
                    realtime,
                    ledger: Mutex::new((0.0, Instant::now())),
                    received: Arc::clone(&received),
                    max_pending: Arc::clone(&max_pending),
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
    SpeechQueue::start(speech, device.thread(), bus())
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
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if events(&device.log).contains(&Ev::Clear) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    false
}

/// `frames` frames of `len` samples each, produced at once; sample k of the utterance holds the value k, so a sink
/// can check it got every one exactly once, in order.
pub fn counting(frames: usize, len: usize) -> FakeSpeech {
    FakeSpeech::new(move |_| {
        (0..frames)
            .map(|frame| {
                Ok(Frame {
                    rate: rate(),
                    samples: (0..len).map(|at| (frame * len + at) as f32).collect(),
                })
            })
            .collect()
    })
}

pub fn heard(text: &str) -> Heard {
    Heard {
        text: text.to_owned(),
        lang: Lang::Es,
        emotion: Emotion::default(),
    }
}

type Answer = Arc<dyn Fn(Option<Duration>) -> Result<Heard, ListenError> + Send + Sync>;

/// A `Listen` that answers by a rule and records the timeout each call carried.
#[derive(Clone)]
pub struct FakeListen {
    answer: Answer,
    pub calls: Arc<Mutex<Vec<Option<Duration>>>>,
}

impl Debug for FakeListen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeListen").finish_non_exhaustive()
    }
}

impl FakeListen {
    pub fn new(answer: impl Fn(Option<Duration>) -> Result<Heard, ListenError> + Send + Sync + 'static) -> Self {
        Self {
            answer: Arc::new(answer),
            calls: Arc::default(),
        }
    }
}

impl Listen for FakeListen {
    fn listen(&self, timeout: Option<Duration>) -> BoxFuture<'_, Result<Heard, ListenError>> {
        self.calls.lock().unwrap().push(timeout);
        let answered = (self.answer)(timeout);
        Box::pin(async move { answered })
    }
}

/// A listen nobody asks for: the state of a test that exercises other doors.
pub fn idle() -> Arc<dyn Listen> {
    Arc::new(FakeListen::new(|_| Err(ListenError::Busy)))
}

/// The test's end of one recognizer session: what the recognizer says goes in, what the listener sent comes out.
pub struct Feed {
    pub says: mpsc::UnboundedSender<Result<Recognized, HearingError>>,
    pub sent: Arc<Mutex<Vec<f32>>>,
    pub closed: Arc<AtomicBool>,
}

impl Feed {
    pub fn say(&self, what: Recognized) {
        self.says.send(Ok(what)).unwrap();
    }

    pub fn fail(&self, error: HearingError) {
        self.says.send(Err(error)).unwrap();
    }

    pub fn sent(&self) -> Vec<f32> {
        self.sent.lock().unwrap().clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

type Inbox = mpsc::UnboundedReceiver<Result<Recognized, HearingError>>;

struct Session {
    inbox: Inbox,
    sent: Arc<Mutex<Vec<f32>>>,
    closed: Arc<AtomicBool>,
}

/// A recognizer whose sessions are scripted by the test, one `Feed` each, in the order they are opened.
#[derive(Clone, Default)]
pub struct FakeHearing {
    sessions: Arc<Mutex<VecDeque<Session>>>,
    pub opens: Arc<AtomicUsize>,
    pub rates: Arc<Mutex<Vec<u32>>>,
    pub fail_open: Arc<Mutex<Option<HearingError>>>,
    pub hold_sends: Arc<AtomicBool>,
}

impl FakeHearing {
    pub fn session(&self) -> Feed {
        let (says, inbox) = mpsc::unbounded_channel();
        let (sent, closed) = (Arc::<Mutex<Vec<f32>>>::default(), Arc::<AtomicBool>::default());
        self.sessions.lock().unwrap().push_back(Session {
            inbox,
            sent: Arc::clone(&sent),
            closed: Arc::clone(&closed),
        });
        Feed { says, sent, closed }
    }

    pub fn opened(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }
}

pub struct FakeTranscription {
    inbox: Inbox,
    sent: Arc<Mutex<Vec<f32>>>,
    closed: Arc<AtomicBool>,
    hold: Arc<AtomicBool>,
}

impl Drop for FakeTranscription {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl Hearing for FakeHearing {
    type Transcription = FakeTranscription;

    fn open(
        &self,
        rate: std::num::NonZeroU32,
    ) -> impl std::future::Future<Output = Result<FakeTranscription, HearingError>> + Send {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.rates.lock().unwrap().push(rate.get());
        let failure = self.fail_open.lock().unwrap().clone();
        let session = self.sessions.lock().unwrap().pop_front();
        std::future::ready(match (failure, session) {
            (Some(error), _) => Err(error),
            (None, None) => Err(HearingError::Connect("no session scripted".to_owned())),
            (None, Some(session)) => Ok(FakeTranscription {
                inbox: session.inbox,
                sent: session.sent,
                closed: session.closed,
                hold: Arc::clone(&self.hold_sends),
            }),
        })
    }
}

impl Transcription for FakeTranscription {
    async fn send(&mut self, samples: &[f32]) -> Result<(), HearingError> {
        while self.hold.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.sent.lock().unwrap().extend_from_slice(samples);
        Ok(())
    }

    async fn next(&mut self) -> Option<Result<Recognized, HearingError>> {
        self.inbox.recv().await
    }
}

#[derive(Debug)]
struct FakeCapture {
    released: Arc<AtomicUsize>,
    opened_on: std::thread::ThreadId,
    released_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
}

impl Capture for FakeCapture {
    fn rate(&self) -> u32 {
        16_000
    }

    fn name(&self) -> &'static str {
        "fake"
    }

    fn level(&self) -> f32 {
        0.0
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        *self.released_on.lock().unwrap() = Some(std::thread::current().id());
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

/// An input device the test feeds by hand; counts how often it was opened and released.
#[derive(Clone, Default)]
pub struct Mic {
    pub opens: Arc<AtomicUsize>,
    pub released: Arc<AtomicUsize>,
    pub fail: Arc<AtomicBool>,
    pub opened_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
    pub released_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
    deliver: Arc<Mutex<Option<Deliver>>>,
}

impl Mic {
    pub fn thread(&self) -> CaptureThread {
        let mic = self.clone();
        CaptureThread::new(move |deliver| {
            if mic.fail.load(Ordering::SeqCst) {
                return Err(e_voice_device::error::DeviceError::Input(None));
            }
            mic.opens.fetch_add(1, Ordering::SeqCst);
            let me = std::thread::current().id();
            *mic.opened_on.lock().unwrap() = Some(me);
            *mic.deliver.lock().unwrap() = Some(deliver);
            Ok(Box::new(FakeCapture {
                released: Arc::clone(&mic.released),
                opened_on: me,
                released_on: Arc::clone(&mic.released_on),
            }) as Box<dyn Capture>)
        })
    }

    /// One block from the device, on the caller's thread.
    pub fn block(&self, samples: &[f32]) {
        (self.deliver.lock().unwrap().as_mut().expect("the device is open"))(samples);
    }

    /// The device thread's end of the channel goes away, as if the stream died.
    pub fn die(&self) {
        self.deliver.lock().unwrap().take();
    }

    pub fn opened(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    pub fn is_open(&self) -> bool {
        self.opened() > self.released.load(Ordering::SeqCst)
    }

    /// Waits up to a second of real time for the device to be released.
    pub async fn freed(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.opened() > 0 && !self.is_open() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        false
    }

    /// Waits up to a second of real time for the device to be open.
    pub async fn live(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.is_open() && self.deliver.lock().unwrap().is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        false
    }
}

/// The daemon's event bus, as the composition root makes it.
pub fn bus() -> broadcast::Sender<DaemonEvent> {
    broadcast::channel(e_voice_daemon::api::events::EVENTS).0
}

/// The subscriber permits, as the composition root makes them.
pub fn pool() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(e_voice_daemon::api::events::SUBSCRIBERS))
}

/// Everything a daemon is made of, wired as the composition root wires it: one bus, one queue, one listener.
pub struct Wired {
    pub state: e_voice_daemon::api::state::AppState,
    pub events: broadcast::Sender<DaemonEvent>,
    pub device: Device,
    pub hearing: FakeHearing,
    pub mic: Mic,
    pub voices: FakeVoices,
    pub approvals: e_voice_daemon::talk::approval::Approvals,
}

pub fn wired(speech: FakeSpeech, device: Device, listen: Duration) -> Wired {
    wired_with(speech, device, listen, FakeVoices::default(), BTreeMap::new())
}

pub fn wired_with(
    speech: FakeSpeech,
    device: Device,
    listen: Duration,
    voices: FakeVoices,
    table: BTreeMap<String, String>,
) -> Wired {
    let events = bus();
    let (hearing, mic) = (FakeHearing::default(), Mic::default());
    let queue = SpeechQueue::start(speech, device.thread(), events.clone());
    let listener: Arc<dyn Listen> = Arc::new(e_voice_daemon::workflow::listen::Listener::new(
        hearing.clone(),
        mic.thread(),
        listen,
        events.clone(),
    ));
    let approvals = e_voice_daemon::talk::approval::Approvals::default();
    let state = state_with(
        queue,
        listener,
        events.clone(),
        voices.clone(),
        table,
        approvals.clone(),
    );
    Wired {
        state,
        events,
        device,
        hearing,
        mic,
        voices,
        approvals,
    }
}

/// A scripted voice list.
#[derive(Debug, Clone)]
pub struct FakeVoices {
    pub answer: Arc<Mutex<Result<Vec<String>, SpeechError>>>,
    pub calls: Arc<AtomicUsize>,
}

impl Default for FakeVoices {
    fn default() -> Self {
        Self {
            answer: Arc::new(Mutex::new(Ok(vec!["ef_dora".to_owned(), "em_alex".to_owned()]))),
            calls: Arc::default(),
        }
    }
}

impl e_voice_daemon::speech::base::Voices for FakeVoices {
    fn list(&self) -> BoxFuture<'_, Result<Vec<String>, SpeechError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self.answer.lock().unwrap().clone();
        Box::pin(async move { answer })
    }
}

pub fn state(
    speech: SpeechQueue,
    listen: Arc<dyn Listen>,
    events: broadcast::Sender<DaemonEvent>,
) -> e_voice_daemon::api::state::AppState {
    state_with(
        speech,
        listen,
        events,
        FakeVoices::default(),
        BTreeMap::new(),
        e_voice_daemon::talk::approval::Approvals::default(),
    )
}

pub fn state_with(
    speech: SpeechQueue,
    listen: Arc<dyn Listen>,
    events: broadcast::Sender<DaemonEvent>,
    voices: FakeVoices,
    table: BTreeMap<String, String>,
    approvals: e_voice_daemon::talk::approval::Approvals,
) -> e_voice_daemon::api::state::AppState {
    let table = e_voice_daemon::speech::table::VoiceTable::new(table);
    let mcp = e_voice_daemon::mcp::server::McpServer::new(
        speech.clone(),
        Arc::clone(&listen),
        Arc::new(voices),
        table.clone(),
        approvals,
    );
    e_voice_daemon::api::state::AppState {
        speech,
        listen,
        events,
        subscribers: pool(),
        mcp,
        table,
        lang: Lang::Es,
    }
}

/// Every event already on the receiver, in order.
pub fn drained(receiver: &mut broadcast::Receiver<DaemonEvent>) -> Vec<DaemonEvent> {
    std::iter::from_fn(|| receiver.try_recv().ok()).collect()
}
