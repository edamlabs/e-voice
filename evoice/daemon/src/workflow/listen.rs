use std::fmt::{self, Debug};
use std::time::Duration;

use e_voice_device::error::DeviceError;
use futures_util::future::BoxFuture;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::hearing::base::{Heard, Hearing, HearingError, Recognized, Transcription};
use crate::workflow::capture::{CaptureThread, Microphone};

/// The longest a listen may wait for someone to start speaking. A listen holds the only microphone and
/// refuses every other listener meanwhile, so no single request may keep it longer than five minutes.
pub const LONGEST: Duration = Duration::from_secs(300);

/// How long a listen waits for the final once someone started speaking: the recognizer's longest segment
/// (`[stt.pipeline.vad] max_speech`, 20 s), plus its ASR deadline counted from the end of speech
/// (`[stt.pipeline.asr] deadline`, 15 s), plus 5 s for the closing silence, the pad and the transport.
/// Derived from the recognizer's defaults: a recognizer configured for longer segments needs it raised.
const SPOKEN: Duration = Duration::from_secs(40);

#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    #[error("a listen timeout is above zero and at most {} s, not {} s", LONGEST.as_secs(), .0.as_secs_f64())]
    Invalid(Duration),
    #[error("someone is already listening")]
    Busy,
    #[error("nobody started speaking within {} s", .0.as_secs_f64())]
    Timeout(Duration),
    #[error(transparent)]
    Device(DeviceError),
    #[error(transparent)]
    Hearing(HearingError),
}

/// What the HTTP door needs: one listen, whatever recognizer runs it. The listener is generic over its
/// recognizer; this seam keeps that type out of the state every handler shares.
pub trait Listen: Debug + Send + Sync {
    /// Listens with `timeout` to start speaking, the listener's default when `None`.
    fn listen(&self, timeout: Option<Duration>) -> BoxFuture<'_, Result<Heard, ListenError>>;
}

/// Where a listen stands: waiting for someone to start speaking, until the listen's own deadline, or
/// hearing them, until the instant the recognizer owes its final.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Waiting,
    Speaking(Instant),
}

/// Opens the microphone and a recognizer session for one listen, and returns the first segment heard in
/// full: a final with text and no error. One listen at a time; the microphone and the session close
/// when it ends, and when its caller stops waiting.
pub struct Listener<H> {
    hearing: H,
    capture: CaptureThread,
    timeout: Duration,
    turn: Mutex<()>,
}

impl<H> Debug for Listener<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<H: Hearing> Listener<H> {
    // ##### PRIVATE #####

    async fn listen_turn(&self, timeout: Duration) -> Result<Heard, ListenError> {
        let mut microphone = self.capture.open().await.map_err(ListenError::Device)?;
        let rate = microphone.rate;
        let mut transcription = self.hearing.open(rate).await.map_err(ListenError::Hearing)?;
        let heard = Self::listen_until(&mut microphone, &mut transcription, timeout).await;
        let dropped = microphone.dropped();
        if dropped > 0 {
            tracing::warn!(dropped, "microphone.dropped");
        }
        heard
    }

    /// Feeds every block to the recognizer until it finishes a segment with text. The timeout runs from
    /// the moment the microphone and the session are both open, and bounds only the wait for speech: once
    /// someone starts, the final is owed within [`SPOKEN`] of the latest start, and missing it is the
    /// recognizer's failure, not a timeout. A final that failed or heard nothing is skipped, and the
    /// listen waits for speech again, against its own deadline.
    async fn listen_until(
        microphone: &mut Microphone,
        transcription: &mut H::Transcription,
        timeout: Duration,
    ) -> Result<Heard, ListenError> {
        let now = Instant::now();
        let waiting = now.checked_add(timeout).ok_or(ListenError::Invalid(timeout))?;
        let mut phase = Phase::Waiting;
        loop {
            let deadline = match phase {
                Phase::Waiting => waiting,
                Phase::Speaking(until) => until,
            };
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => return Err(Self::listen_expired(phase, timeout)),
                block = microphone.recv() => match block {
                    Some(samples) => transcription.send(&samples).await.map_err(ListenError::Hearing)?,
                    None => return Err(ListenError::Device(DeviceError::Input(None))),
                },
                recognized = transcription.next() => match recognized {
                    Some(Ok(Recognized::Started)) => {
                        let until = Instant::now().checked_add(SPOKEN).ok_or(ListenError::Invalid(SPOKEN))?;
                        phase = Phase::Speaking(until);
                    }
                    Some(Ok(Recognized::Final(heard, None))) if !heard.text.trim().is_empty() => return Ok(heard),
                    Some(Ok(Recognized::Final(_, error))) => {
                        if let Some(error) = error {
                            tracing::warn!(%error, "listen.final.failed");
                        }
                        phase = Phase::Waiting;
                    }
                    Some(Ok(Recognized::Partial(_))) => {}
                    Some(Err(error)) => return Err(ListenError::Hearing(error)),
                    None => {
                        let closed = HearingError::Server("closed the session before a final".to_owned());
                        return Err(ListenError::Hearing(closed));
                    }
                },
            }
        }
    }

    fn listen_expired(phase: Phase, timeout: Duration) -> ListenError {
        let late = || HearingError::Server(format!("no final within {} s of speech", SPOKEN.as_secs()));
        match phase {
            Phase::Waiting => ListenError::Timeout(timeout),
            Phase::Speaking(_) => ListenError::Hearing(late()),
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// `timeout` is the default wait for speech, for a listen that names none.
    #[must_use]
    pub fn new(hearing: H, capture: CaptureThread, timeout: Duration) -> Self {
        Self {
            hearing,
            capture,
            timeout,
            turn: Mutex::new(()),
        }
    }
}

impl<H: Hearing> Listen for Listener<H> {
    /// A timeout of zero or over [`LONGEST`] is refused before anything opens; a second listen while one
    /// runs is refused at once, never queued.
    fn listen(&self, timeout: Option<Duration>) -> BoxFuture<'_, Result<Heard, ListenError>> {
        Box::pin(async move {
            let timeout = timeout.unwrap_or(self.timeout);
            (!timeout.is_zero() && timeout <= LONGEST)
                .then_some(())
                .ok_or(ListenError::Invalid(timeout))?;
            let _turn = self.turn.try_lock().map_err(|_| ListenError::Busy)?;
            self.listen_turn(timeout).await
        })
    }
}
