use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use e_voice_daemon::hearing::base::Recognized;
use e_voice_daemon::schema::event::{DaemonEvent, RecordState};
use e_voice_daemon::speech::base::VoiceClip;
use e_voice_daemon::workflow::record::{Learned, RecordError, Take};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::fake::{Device, FakeSpeech, Feed, Wired, heard, wired};

pub const RATE: usize = 16_000;

pub type Recording = JoinHandle<Result<Learned, RecordError>>;

/// A recorder over a fake microphone, a fake recognizer and a fake speech service, with the audio fed by hand.
pub struct Rig {
    pub w: Wired,
    pub events: broadcast::Receiver<DaemonEvent>,
    pub feeds: Vec<Feed>,
    fed: AtomicUsize,
}

impl Rig {
    pub fn new(sessions: usize) -> Self {
        Self::waiting(sessions, Duration::from_secs(5))
    }

    /// `listen` is how long a recording waits for someone to start speaking.
    pub fn waiting(sessions: usize, listen: Duration) -> Self {
        let w = wired(FakeSpeech::marked(), Device::new(0, 0), listen);
        let events = w.events.subscribe();
        let feeds = (0..sessions).map(|_| w.hearing.session()).collect();
        Self {
            w,
            events,
            feeds,
            fed: AtomicUsize::new(0),
        }
    }

    pub fn take(id: &str) -> Take {
        Take {
            id: id.to_owned(),
            text: None,
            silence: Duration::from_millis(2_500),
            max: Duration::from_secs(30),
            replace: false,
        }
    }

    pub fn start(&self, take: Take) -> Recording {
        let record = Arc::clone(&self.w.record);
        tokio::spawn(async move { record.record(take).await })
    }

    /// The microphone is open under the recording.
    pub async fn live(&self) {
        assert!(self.w.mic.live().await, "the microphone never opened");
    }

    /// `samples` of `value` from the microphone, `block` at a time, each waited for until the recognizer has
    /// them; a recording that ended stops taking them, which is no error here.
    pub async fn feed(&self, samples: usize, value: f32, block: usize) {
        let mut left = samples;
        while left > 0 {
            let size = left.min(block);
            self.w.mic.block(&vec![value; size]);
            left -= size;
            let target = self.fed.fetch_add(size, Ordering::SeqCst) + size;
            let deadline = Instant::now() + Duration::from_millis(500);
            while self.feeds[0].sent().len() < target && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }

    /// `seconds` of `value` in 0.1 s blocks.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub async fn audio(&self, seconds: f32, value: f32) {
        self.feed((seconds * RATE as f32) as usize, value, RATE / 10).await;
    }

    pub fn says(&self, what: Recognized) {
        self.feeds[0].say(what);
    }

    pub fn started(&self, seconds: f32) {
        self.says(Recognized::Started(Duration::from_secs_f32(seconds)));
    }

    pub fn stopped(&self, seconds: f32) {
        self.says(Recognized::Stopped(Duration::from_secs_f32(seconds)));
    }

    pub fn final_of(&self, text: &str) {
        self.says(Recognized::Final(heard(text), None));
    }

    pub fn clips(&self) -> Vec<VoiceClip> {
        self.w.learning.clips.lock().unwrap().clone()
    }

    /// Every recording state on the bus right now, in order.
    pub fn states(&mut self) -> Vec<RecordState> {
        std::iter::from_fn(|| self.events.try_recv().ok())
            .filter_map(|event| match event {
                DaemonEvent::Recording { state } => Some(state),
                _ => None,
            })
            .collect()
    }

    /// Waits until the bus has carried `state`, returning every recording state seen up to it.
    pub async fn until(&mut self, state: RecordState) -> Vec<RecordState> {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.events.recv()).await {
                Ok(Ok(DaemonEvent::Recording { state: got })) => {
                    seen.push(got);
                    if got == state {
                        return seen;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("the bus failed: {error:?}"),
                Err(elapsed) => panic!("never reached {state:?} ({elapsed}); saw {seen:?}"),
            }
        }
    }

    pub async fn done(&self, recording: Recording) -> Result<Learned, RecordError> {
        tokio::time::timeout(Duration::from_secs(10), recording)
            .await
            .expect("the recording never ended")
            .unwrap()
    }
}

/// The mean of the clip away from its edges, where the resampler rings.
pub fn core(samples: &[f32]) -> f32 {
    let inner = &samples[1_000..samples.len() - 1_000];
    inner.iter().sum::<f32>() / inner.len() as f32
}
