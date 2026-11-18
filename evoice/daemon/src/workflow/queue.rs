use std::fmt::{self, Debug};
use std::time::Duration;

use e_voice_device::error::DeviceError;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};

use crate::speech::base::{Frame, Speech, SpeechError, SpeechRequest, Utterance};
use crate::workflow::audio::AudioThread;

/// Requests that may wait behind the one being spoken; one more is refused, never queued.
pub const BACKLOG: usize = 16;

/// How often a finished utterance asks the device whether it has played everything. The queue's only
/// poll, confined to [`SpeechQueue::start_drain`]: an event from the sink can replace it there alone.
const DRAIN: Duration = Duration::from_millis(20);

/// Where the queue stands: speaking while the service sends audio, draining while the device plays the
/// rest of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechState {
    Idle,
    Speaking { id: u64 },
    Draining { id: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum SpeakError {
    #[error("text is empty")]
    Empty,
    #[error("speech queue is full: {} requests already waiting", BACKLOG)]
    Full,
    #[error("speech queue stopped")]
    Stopped,
    #[error(transparent)]
    Device(DeviceError),
    #[error(transparent)]
    Speech(SpeechError),
}

struct Job {
    request: SpeechRequest,
    done: oneshot::Sender<(u64, Result<(), SpeakError>)>,
}

/// Everything the daemon says goes through one queue, one text at a time: the next starts only once
/// the device has played the last sample of the one before, so two never overlap. The task behind it
/// ends when the last handle drops.
#[derive(Clone)]
pub struct SpeechQueue {
    jobs: mpsc::Sender<Job>,
    state: watch::Receiver<SpeechState>,
}

impl Debug for SpeechQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeechQueue")
            .field("state", &*self.state.borrow())
            .finish_non_exhaustive()
    }
}

impl SpeechQueue {
    // ##### PRIVATE #####

    /// Ids are given here, in the order texts are spoken: contiguous, and never spent on a refused one.
    async fn start_serve<S: Speech>(
        speech: S,
        audio: AudioThread,
        mut jobs: mpsc::Receiver<Job>,
        state: watch::Sender<SpeechState>,
    ) {
        let mut next = 0u64;
        while let Some(Job { request, done }) = jobs.recv().await {
            let id = next;
            next = next.saturating_add(1);
            state.send_replace(SpeechState::Speaking { id });
            let streamed = Self::start_stream(&speech, &audio, request).await;
            state.send_replace(SpeechState::Draining { id });
            Self::start_drain(&audio).await;
            state.send_replace(SpeechState::Idle);
            done.send((id, streamed)).ok();
        }
    }

    /// Everything the service sends goes to the device, even when it fails halfway: what was queued
    /// still plays, and drains, before the next text.
    async fn start_stream<S: Speech>(
        speech: &S,
        audio: &AudioThread,
        request: SpeechRequest,
    ) -> Result<(), SpeakError> {
        audio.open().await.map_err(SpeakError::Device)?;
        let mut utterance = speech.say(request).await.map_err(SpeakError::Speech)?;
        while let Some(frame) = utterance.frame().await {
            let Frame { rate, samples } = frame.map_err(SpeakError::Speech)?;
            audio.push(samples, rate);
        }
        Ok(())
    }

    /// Draining's single exit: returns once the device has nothing left to play.
    async fn start_drain(audio: &AudioThread) {
        while audio.pending().await > 0.0 {
            tokio::time::sleep(DRAIN).await;
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Starts the queue's task on the current runtime.
    #[must_use]
    pub fn start<S: Speech>(speech: S, audio: AudioThread) -> Self {
        let (jobs, inbox) = mpsc::channel(BACKLOG);
        let (state, observed) = watch::channel(SpeechState::Idle);
        tokio::spawn(Self::start_serve(speech, audio, inbox, state));
        Self { jobs, state: observed }
    }

    /// Speaks `request` after everything accepted before it and resolves with its id once the device has
    /// played it. The text is accepted at the first poll; dropping the future after that does not stop
    /// the speech.
    ///
    /// # Errors
    /// Empty text, a full queue, a stopped queue, a device that cannot open, or the speech service
    /// failing.
    pub async fn say(&self, request: SpeechRequest) -> Result<u64, SpeakError> {
        (!request.text.trim().is_empty())
            .then_some(())
            .ok_or(SpeakError::Empty)?;
        let (done, spoken) = oneshot::channel();
        self.jobs.try_send(Job { request, done }).map_err(|error| match error {
            TrySendError::Full(_) => SpeakError::Full,
            TrySendError::Closed(_) => SpeakError::Stopped,
        })?;
        let (id, streamed) = spoken.await.map_err(|_| SpeakError::Stopped)?;
        streamed.map(|()| id)
    }

    #[must_use]
    pub fn state(&self) -> SpeechState {
        *self.state.borrow()
    }
}
