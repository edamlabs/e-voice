use std::fmt::{self, Debug};
use std::time::Duration;

use e_voice_device::error::DeviceError;
use serde::Serialize;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};

use crate::speech::base::{Frame, Speech, SpeechError, SpeechRequest, Utterance};
use crate::workflow::audio::AudioThread;

/// Requests that may wait behind the one being spoken; one more is refused, never queued.
pub const BACKLOG: usize = 16;

/// Stop orders that may wait for the queue's task; a sender beyond them waits its turn.
const CONTROLS: usize = 4;

/// The most characters one text may carry: what a spoken turn can reasonably be (≈ 4.5 min of speech),
/// and the TTS service's own limit on what one request may ask, which the daemon must never exceed.
pub const TEXT: usize = 4_096;

/// The most audio the device may hold ahead of what it plays. The reader stops pulling from the speech
/// service past it, so memory is bounded by time, whatever the length of the text.
const AHEAD: Duration = Duration::from_secs(2);

/// How often a waiting reader asks the device how much it still holds. The queue's only poll, confined
/// to [`SpeechQueue::start_settle`]: an event from the sink can replace it there alone.
const DRAIN: Duration = Duration::from_millis(20);

/// Where the queue stands: speaking while the service sends audio, draining while the device plays the
/// rest of it, cancelling while a stop silences both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechState {
    Idle,
    Speaking { id: u64 },
    Draining { id: u64 },
    Cancelling { id: u64 },
}

/// How a text that was accepted ended: played to its last sample, or cut short by a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Done,
    Cancelled,
}

/// What a stop did: the text it cut short, if one was playing, and how many waiting ones it dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Halted {
    pub stopped: Option<u64>,
    pub dropped: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SpeakError {
    #[error("text is empty")]
    Empty,
    #[error("text is too long: {0} characters, at most {limit}", limit = TEXT)]
    TooLong(usize),
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
    done: oneshot::Sender<(u64, Result<Outcome, SpeakError>)>,
}

enum Control {
    Stop(oneshot::Sender<Halted>),
}

/// How the service's part of a text ended: every frame read (or a failure), or a stop, with the
/// utterance when it was already open.
enum Streamed<U> {
    Done(Result<(), SpeakError>),
    Stopped(oneshot::Sender<Halted>, Option<U>),
}

/// Everything the daemon says goes through one queue, one text at a time: the next starts only once
/// the device has played the last sample of the one before, so two never overlap. The task behind it
/// ends when the last handle drops.
#[derive(Clone)]
pub struct SpeechQueue {
    jobs: mpsc::Sender<Job>,
    controls: mpsc::Sender<Control>,
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
    /// A stop while idle has nothing to silence and answers at once.
    async fn start_serve<S: Speech>(
        speech: S,
        audio: AudioThread,
        mut jobs: mpsc::Receiver<Job>,
        mut controls: mpsc::Receiver<Control>,
        state: watch::Sender<SpeechState>,
    ) {
        let mut next = 0u64;
        loop {
            tokio::select! {
                biased;
                Some(Control::Stop(reply)) = controls.recv() => {
                    reply.send(Halted { stopped: None, dropped: 0 }).ok();
                }
                job = jobs.recv() => match job {
                    Some(job) => {
                        next = Self::start_job(&speech, &audio, job, next, &mut jobs, &mut controls, &state).await;
                    }
                    None => return,
                },
            }
        }
    }

    /// One text from the service to the device; returns the next free id.
    async fn start_job<S: Speech>(
        speech: &S,
        audio: &AudioThread,
        job: Job,
        id: u64,
        jobs: &mut mpsc::Receiver<Job>,
        controls: &mut mpsc::Receiver<Control>,
        state: &watch::Sender<SpeechState>,
    ) -> u64 {
        let Job { request, done } = job;
        state.send_replace(SpeechState::Speaking { id });
        let (result, stop) = match Self::start_stream(speech, audio, request, controls).await {
            Streamed::Done(result) => {
                audio.finish();
                state.send_replace(SpeechState::Draining { id });
                let stop = Self::start_settle(audio, controls, Duration::ZERO).await;
                let outcome = stop.as_ref().map_or(Outcome::Done, |_| Outcome::Cancelled);
                (result.map(|()| outcome), stop.map(|reply| (reply, None)))
            }
            Streamed::Stopped(reply, utterance) => (Ok(Outcome::Cancelled), Some((reply, utterance))),
        };
        done.send((id, result)).ok();
        let next = id.saturating_add(1);
        match stop {
            Some((reply, utterance)) => Self::start_cancel(id, utterance, audio, jobs, next, reply, state).await,
            None => {
                state.send_replace(SpeechState::Idle);
                next
            }
        }
    }

    /// Everything the service sends goes to the device, even when it fails halfway: what was queued
    /// still plays, and drains, before the next text. A stop ends it at any await: opening the device,
    /// connecting to the service (the half-open connection is dropped), or reading.
    async fn start_stream<S: Speech>(
        speech: &S,
        audio: &AudioThread,
        request: SpeechRequest,
        controls: &mut mpsc::Receiver<Control>,
    ) -> Streamed<S::Utterance> {
        let opened = tokio::select! {
            biased;
            Some(Control::Stop(reply)) = controls.recv() => return Streamed::Stopped(reply, None),
            opened = audio.open() => opened,
        };
        if let Err(error) = opened {
            return Streamed::Done(Err(SpeakError::Device(error)));
        }
        let said = tokio::select! {
            biased;
            Some(Control::Stop(reply)) = controls.recv() => return Streamed::Stopped(reply, None),
            said = speech.say(request) => said,
        };
        let mut utterance = match said {
            Ok(utterance) => utterance,
            Err(error) => return Streamed::Done(Err(SpeakError::Speech(error))),
        };
        let reply = loop {
            if let Some(reply) = Self::start_settle(audio, controls, AHEAD).await {
                break reply;
            }
            tokio::select! {
                biased;
                Some(Control::Stop(reply)) = controls.recv() => break reply,
                frame = utterance.frame() => match frame {
                    Some(Ok(Frame { rate, samples })) => audio.push(samples, rate),
                    Some(Err(error)) => return Streamed::Done(Err(SpeakError::Speech(error))),
                    None => return Streamed::Done(Ok(())),
                },
            }
        };
        Streamed::Stopped(reply, Some(utterance))
    }

    /// Waits until the device holds at most `below` of audio, or returns the stop that came first.
    /// Backpressure (`below` = [`AHEAD`]) and Draining's single exit (`below` = zero) share it.
    async fn start_settle(
        audio: &AudioThread,
        controls: &mut mpsc::Receiver<Control>,
        below: Duration,
    ) -> Option<oneshot::Sender<Halted>> {
        while audio.pending().await > below.as_secs_f64() {
            tokio::select! {
                biased;
                Some(Control::Stop(reply)) = controls.recv() => return Some(reply),
                () = tokio::time::sleep(DRAIN) => {}
            }
        }
        None
    }

    /// Silences the text `id`: the service is told to stop, the device drops what it holds, and every
    /// waiting text is answered cancelled, each with the next id. Returns the next free id.
    async fn start_cancel<U: Utterance>(
        id: u64,
        utterance: Option<U>,
        audio: &AudioThread,
        jobs: &mut mpsc::Receiver<Job>,
        next: u64,
        reply: oneshot::Sender<Halted>,
        state: &watch::Sender<SpeechState>,
    ) -> u64 {
        state.send_replace(SpeechState::Cancelling { id });
        if let Some(mut utterance) = utterance
            && let Err(error) = utterance.cancel().await
        {
            tracing::warn!(%error, id, "speech.cancel.failed");
        }
        audio.clear();
        let (mut next, mut dropped) = (next, 0usize);
        while let Ok(Job { done, .. }) = jobs.try_recv() {
            done.send((next, Ok(Outcome::Cancelled))).ok();
            next = next.saturating_add(1);
            dropped = dropped.saturating_add(1);
        }
        state.send_replace(SpeechState::Idle);
        reply
            .send(Halted {
                stopped: Some(id),
                dropped,
            })
            .ok();
        next
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Starts the queue's task on the current runtime.
    #[must_use]
    pub fn start<S: Speech>(speech: S, audio: AudioThread) -> Self {
        let (jobs, inbox) = mpsc::channel(BACKLOG);
        let (controls, orders) = mpsc::channel(CONTROLS);
        let (state, observed) = watch::channel(SpeechState::Idle);
        tokio::spawn(Self::start_serve(speech, audio, inbox, orders, state));
        Self {
            jobs,
            controls,
            state: observed,
        }
    }

    /// Speaks `request` after everything accepted before it and resolves with its id and how it ended:
    /// played to the end, or cut short by a stop. The text is accepted at the first poll; dropping the
    /// future after that does not stop the speech.
    ///
    /// # Errors
    /// Empty text, a text over [`TEXT`] characters, a full queue, a stopped queue, a device that cannot
    /// open, or the speech service failing.
    pub async fn say(&self, request: SpeechRequest) -> Result<(u64, Outcome), SpeakError> {
        (!request.text.trim().is_empty())
            .then_some(())
            .ok_or(SpeakError::Empty)?;
        let length = request.text.chars().count();
        (length <= TEXT).then_some(()).ok_or(SpeakError::TooLong(length))?;
        let (done, spoken) = oneshot::channel();
        self.jobs.try_send(Job { request, done }).map_err(|error| match error {
            TrySendError::Full(_) => SpeakError::Full,
            TrySendError::Closed(_) => SpeakError::Stopped,
        })?;
        let (id, ended) = spoken.await.map_err(|_| SpeakError::Stopped)?;
        ended.map(|outcome| (id, outcome))
    }

    /// Silences what is playing and drops what waits; nothing to do while idle.
    ///
    /// # Errors
    /// The queue's task is gone.
    pub async fn stop(&self) -> Result<Halted, SpeakError> {
        let (reply, halted) = oneshot::channel();
        self.controls
            .send(Control::Stop(reply))
            .await
            .map_err(|_| SpeakError::Stopped)?;
        halted.await.map_err(|_| SpeakError::Stopped)
    }

    #[must_use]
    pub fn state(&self) -> SpeechState {
        *self.state.borrow()
    }
}
