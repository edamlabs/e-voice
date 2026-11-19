use std::fmt::{self, Debug};
use std::num::NonZeroU32;
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use e_voice_core::audio::{AudioEncoding, AudioError, AudioIngest};
use e_voice_core::schema::error::NodeError;
use e_voice_device::error::DeviceError;
use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::hearing::base::{Hearing, HearingError, Recognized, Transcription};
use crate::schema::event::{DaemonEvent, RecordState};
use crate::speech::base::{Learning, SpeechError, VoiceClip, Voices};
use crate::workflow::capture::{CaptureThread, Microphone};
use crate::workflow::listen::Listen;

/// The rate a clip is sent at: the speech service keeps a voice's prompt as 24 kHz mono.
pub const RATE: NonZeroU32 = NonZeroU32::MIN.saturating_add(23_999);

/// The longest clip: the speech service keeps at most 30 s of a voice's prompt and throws the rest away.
pub const LONGEST: Duration = Duration::from_secs(30);

/// The silence after speech that ends a recording, for a take that names none.
pub const SILENCE: Duration = Duration::from_millis(2_500);

/// The longest voice id, as the speech service counts it.
const ID: usize = 64;

/// Silence sent to the recognizer when a recording ends mid-speech, so its VAD closes the segment and
/// finishes it: past its default closing silence (`[stt.pipeline.vad] min_silence`, 0.5 s).
const HUSH: Duration = Duration::from_secs(1);

/// How long the recognizer may take, once a recording ends, to finish what it heard: its ASR deadline
/// counted from the end of speech (`[stt.pipeline.asr] deadline`, 15 s), plus 5 s for the transport.
/// Past it the transcript lacks the last segments; the clip is learned all the same.
const DRAIN: Duration = Duration::from_secs(20);

/// Controls one recording may receive before it reads them: a finish and a stop.
const CONTROLS: usize = 4;

const NANOS: u128 = 1_000_000_000;

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("{0}")]
    Invalid(String),
    #[error("someone is already listening")]
    Busy,
    #[error("the voice {0:?} exists; send \"replace\": true to replace it")]
    Exists(String),
    #[error("nobody started speaking within {} s", .0.as_secs_f64())]
    Timeout(Duration),
    #[error("the recording ended before anyone spoke")]
    Unheard,
    #[error("the recording was stopped")]
    Cancelled,
    #[error(transparent)]
    Device(DeviceError),
    #[error(transparent)]
    Hearing(HearingError),
    #[error(transparent)]
    Speech(SpeechError),
    #[error(transparent)]
    Audio(AudioError),
}

/// One voice to record: its id, the text read (the transcript when `None`), the silence after speech
/// that ends it, the longest clip, and whether a voice with the same id may be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    pub id: String,
    pub text: Option<String>,
    pub silence: Duration,
    pub max: Duration,
    pub replace: bool,
}

/// A voice learned: its id, the seconds of audio sent, the text sent with them, and what the recognizer
/// heard in them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Learned {
    pub id: String,
    pub seconds: f64,
    pub text: Option<String>,
    pub transcript: String,
}

/// What the HTTP door needs: one recording at a time, whatever recognizer runs it, and the two ways to
/// end it early. The recorder is generic over its recognizer; this seam keeps that type out of the state
/// every handler shares.
pub trait Record: Debug + Send + Sync {
    /// Records `take` from the microphone and has the speech service learn it.
    fn record(&self, take: Take) -> BoxFuture<'_, Result<Learned, RecordError>>;

    /// Ends the recording running, keeping what was recorded; `false` when none runs.
    fn finish(&self) -> bool;

    /// Stops the recording running, learning nothing; `false` when none runs.
    fn cancel(&self) -> bool;
}

/// How a recording is ended from outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Finish,
    Cancel,
}

/// Samples at `rate` in `at`.
fn span(rate: NonZeroU32, at: Duration) -> usize {
    let samples = at
        .as_nanos()
        .saturating_mul(u128::from(rate.get()))
        .checked_div(NANOS)
        .unwrap_or_default();
    usize::try_from(samples).unwrap_or(usize::MAX)
}

/// Everything recorded since the microphone opened, and where the recognizer heard speech in it.
/// `start` is the first speech's start, `end` the latest speech's end; `open` counts the segments heard
/// and not yet finished.
#[derive(Debug)]
struct Tape {
    rate: NonZeroU32,
    samples: Vec<f32>,
    start: Option<usize>,
    end: Option<usize>,
    speaking: bool,
    open: usize,
    transcript: Vec<String>,
}

impl Tape {
    fn new(rate: NonZeroU32) -> Self {
        Self {
            rate,
            samples: Vec::new(),
            start: None,
            end: None,
            speaking: false,
            open: 0,
            transcript: Vec::new(),
        }
    }

    /// Whether this is the first speech of the recording.
    fn started(&mut self, at: Duration) -> bool {
        let first = self.start.is_none();
        let at = span(self.rate, at).min(self.samples.len());
        self.start.get_or_insert(at);
        self.speaking = true;
        self.open = self.open.saturating_add(1);
        first
    }

    fn stopped(&mut self, at: Duration) {
        self.end = Some(span(self.rate, at).min(self.samples.len()));
        self.speaking = false;
    }

    /// A final with text joins the transcript; a failed one only closes its segment.
    fn finished(&mut self, text: &str, error: Option<NodeError>) {
        self.open = self.open.saturating_sub(1);
        match error {
            Some(error) => tracing::warn!(%error, "record.final.failed"),
            None => self
                .transcript
                .extend(Some(text.trim().to_owned()).filter(|text| !text.is_empty())),
        }
    }

    /// The clip reaches `max` from the first speech, or `silence` has passed since the latest speech
    /// ended, both counted in recorded audio.
    fn ended(&self, take: &Take) -> bool {
        let len = self.samples.len();
        let long = self
            .start
            .is_some_and(|start| len.saturating_sub(start) >= span(self.rate, take.max));
        let quiet = !self.speaking
            && self
                .end
                .is_some_and(|end| len.saturating_sub(end) >= span(self.rate, take.silence));
        long || quiet
    }

    /// From the first speech's start to the latest speech's end, or to the last sample while someone is
    /// still speaking; at most `max`. `None` while nobody spoke.
    fn bounds(&self, max: Duration) -> Option<Range<usize>> {
        let len = self.samples.len();
        let start = self.start?;
        let stop = self.end.filter(|_| !self.speaking).unwrap_or(len);
        Some(start..stop.min(len).min(start.saturating_add(span(self.rate, max))))
    }
}

/// One recording's announcements on the daemon's events: `started` when it is made, `failed` when it
/// drops unless it was learned, so a recording its caller stopped waiting for still ends.
struct RecordTurn<'a> {
    events: &'a broadcast::Sender<DaemonEvent>,
    learned: bool,
}

impl<'a> RecordTurn<'a> {
    fn new(events: &'a broadcast::Sender<DaemonEvent>) -> Self {
        let turn = Self { events, learned: false };
        turn.state(RecordState::Started);
        turn
    }

    fn state(&self, state: RecordState) {
        self.events.send(DaemonEvent::Recording { state }).ok();
    }

    fn learned(mut self) {
        self.learned = true;
        self.state(RecordState::Learned);
    }
}

impl Drop for RecordTurn<'_> {
    fn drop(&mut self) {
        if !self.learned {
            self.state(RecordState::Failed);
        }
    }
}

/// The way in for [`Record::finish`] and [`Record::cancel`] while a recording runs; cleared when it
/// ends, however it ends.
struct RecordControl<'a> {
    slot: &'a Mutex<Option<mpsc::Sender<Control>>>,
}

impl Drop for RecordControl<'_> {
    fn drop(&mut self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// A recording under way: the recognizer's session, the controls from outside, and the tape.
struct Session<T> {
    transcription: T,
    controls: mpsc::Receiver<Control>,
    tape: Tape,
}

impl<T: Transcription> Session<T> {
    // ##### PRIVATE #####

    fn until_recognized(
        &mut self,
        recognized: Option<Result<Recognized, HearingError>>,
        turn: &RecordTurn<'_>,
    ) -> Result<(), RecordError> {
        match recognized {
            Some(Ok(Recognized::Started(at))) => {
                if self.tape.started(at) {
                    turn.state(RecordState::Heard);
                }
            }
            Some(Ok(Recognized::Stopped(at))) => self.tape.stopped(at),
            Some(Ok(Recognized::Final(heard, error))) => self.tape.finished(&heard.text, error),
            Some(Ok(Recognized::Partial(_))) => {}
            Some(Err(error)) => return Err(RecordError::Hearing(error)),
            None => {
                let closed = HearingError::Server("closed the session before the recording ended".to_owned());
                return Err(RecordError::Hearing(closed));
            }
        }
        Ok(())
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Feeds every block to the recognizer and to the tape until the tape has ended or a finish comes.
    /// The timeout bounds only the wait for speech; once someone spoke, the recording ends in audio.
    async fn until(
        &mut self,
        microphone: &mut Microphone,
        take: &Take,
        timeout: Duration,
        turn: &RecordTurn<'_>,
    ) -> Result<(), RecordError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(RecordError::Timeout(timeout))?;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(deadline), if self.tape.start.is_none() => {
                    return Err(RecordError::Timeout(timeout));
                }
                control = self.controls.recv() => match control {
                    Some(Control::Finish) => return Ok(()),
                    Some(Control::Cancel) | None => return Err(RecordError::Cancelled),
                },
                block = microphone.recv() => match block {
                    Some(samples) => {
                        self.transcription.send(&samples).await.map_err(RecordError::Hearing)?;
                        self.tape.samples.extend_from_slice(&samples);
                        if self.tape.ended(take) {
                            return Ok(());
                        }
                    }
                    None => return Err(RecordError::Device(DeviceError::Input(None))),
                },
                recognized = self.transcription.next() => self.until_recognized(recognized, turn)?,
            }
        }
    }

    /// Waits, at most [`DRAIN`], for the finals of every segment heard, after [`HUSH`] closes one cut
    /// mid-speech. A recognizer that fails now only shortens the transcript; a stop still cancels.
    async fn drain(&mut self) -> Result<(), RecordError> {
        if self.tape.speaking {
            let hush = vec![0.0; span(self.tape.rate, HUSH)];
            self.transcription.send(&hush).await.map_err(RecordError::Hearing)?;
        }
        let until = Instant::now().checked_add(DRAIN).ok_or(RecordError::Unheard)?;
        while self.tape.open > 0 {
            tokio::select! {
                () = tokio::time::sleep_until(until) => {
                    tracing::warn!(open = self.tape.open, "record.drain.expired");
                    return Ok(());
                }
                control = self.controls.recv() => match control {
                    Some(Control::Finish) => {}
                    Some(Control::Cancel) | None => return Err(RecordError::Cancelled),
                },
                recognized = self.transcription.next() => match recognized {
                    Some(Ok(Recognized::Final(heard, error))) => self.tape.finished(&heard.text, error),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        tracing::warn!(%error, "record.drain.failed");
                        return Ok(());
                    }
                    None => return Ok(()),
                },
            }
        }
        Ok(())
    }
}

/// Records a voice from the microphone, under the same lease every listen and the voice conversation
/// take, while the recognizer says where speech starts and stops; then trims the clip to that speech,
/// resamples it to [`RATE`] and has the speech service learn it. One recording at a time.
pub struct Recorder<H> {
    hearing: H,
    capture: CaptureThread,
    listen: Arc<dyn Listen>,
    voices: Arc<dyn Voices>,
    learning: Arc<dyn Learning>,
    timeout: Duration,
    events: broadcast::Sender<DaemonEvent>,
    control: Mutex<Option<mpsc::Sender<Control>>>,
}

impl<H> Debug for Recorder<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recorder")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl<H: Hearing> Recorder<H> {
    // ##### PRIVATE #####

    /// The lease first, so a held microphone is refused before anything is asked of the speech service.
    async fn record_take(&self, take: Take) -> Result<Learned, RecordError> {
        Self::record_valid(&take)?;
        let _lease = self.listen.lease().ok_or(RecordError::Busy)?;
        let (sender, controls) = mpsc::channel(CONTROLS);
        *self.control.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        let _control = RecordControl { slot: &self.control };
        self.record_absent(&take).await?;
        let mut microphone = self.capture.open().await.map_err(RecordError::Device)?;
        let rate = microphone.rate;
        let transcription = self.hearing.open(rate).await.map_err(RecordError::Hearing)?;
        let turn = RecordTurn::new(&self.events);
        let mut session = Session {
            transcription,
            controls,
            tape: Tape::new(rate),
        };
        session.until(&mut microphone, &take, self.timeout, &turn).await?;
        turn.state(RecordState::Finished);
        drop(microphone);
        let bounds = session.tape.bounds(take.max);
        session.drain().await?;
        let clip = bounds
            .and_then(|bounds| session.tape.samples.get(bounds))
            .filter(|clip| !clip.is_empty())
            .ok_or(RecordError::Unheard)?
            .to_vec();
        let samples = Self::record_resample(rate, clip).await?;
        let transcript = session.tape.transcript.join(" ");
        let text = take
            .text
            .or_else(|| Some(transcript.clone()).filter(|transcript| !transcript.is_empty()));
        let seconds = Self::record_seconds(samples.len());
        let clip = VoiceClip {
            id: take.id.clone(),
            rate: RATE,
            samples,
            text: text.clone(),
        };
        self.learning.learn(clip).await.map_err(RecordError::Speech)?;
        turn.learned();
        tracing::info!(voice = %take.id, seconds, "record.learned");
        Ok(Learned {
            id: take.id,
            seconds,
            text,
            transcript,
        })
    }

    /// The speech service's own id rule, checked before anyone reads 30 s for nothing.
    fn record_valid(take: &Take) -> Result<(), RecordError> {
        let id = (1..=ID).contains(&take.id.len())
            && take
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_');
        let bounded = |duration: Duration| !duration.is_zero() && duration <= LONGEST;
        [
            (id, "a voice id is 1-64 chars of a-z, 0-9, '-' or '_'"),
            (bounded(take.silence), "a silence is above zero and at most 30 s"),
            (bounded(take.max), "a max is above zero and at most 30 s"),
        ]
        .into_iter()
        .find(|(ok, _)| !ok)
        .map_or(Ok(()), |(_, reason)| Err(RecordError::Invalid(reason.to_owned())))
    }

    /// A voice with the id is refused unless the take replaces it.
    async fn record_absent(&self, take: &Take) -> Result<(), RecordError> {
        let voices = if take.replace {
            Vec::new()
        } else {
            self.voices.list().await.map_err(RecordError::Speech)?
        };
        (!voices.contains(&take.id))
            .then_some(())
            .ok_or_else(|| RecordError::Exists(take.id.clone()))
    }

    /// The clip at [`RATE`], cut to its own length (the resampler pads its last chunk) and to
    /// [`LONGEST`]; off the event loop, being CPU work.
    async fn record_resample(rate: NonZeroU32, clip: Vec<f32>) -> Result<Vec<f32>, RecordError> {
        let resample = move || {
            let seconds = Duration::from_nanos(
                u64::try_from(clip.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(1_000_000_000)
                    .checked_div(u64::from(rate.get()))
                    .unwrap_or_default(),
            );
            let length = span(RATE, seconds.min(LONGEST));
            let mut ingest = AudioIngest::new(rate.get(), RATE.get(), AudioEncoding::F32le)?;
            let mut samples = ingest.feed(&clip)?;
            samples.extend(ingest.flush()?);
            samples.truncate(length);
            Ok(samples)
        };
        tokio::task::spawn_blocking(resample)
            .await
            .map_err(|error| RecordError::Audio(AudioError::Resample(error.to_string())))?
            .map_err(RecordError::Audio)
    }

    fn record_seconds(samples: usize) -> f64 {
        f64::from(u32::try_from(samples).unwrap_or(u32::MAX)) / f64::from(RATE.get())
    }

    /// Hands `control` to the recording running, if one does.
    fn common_send(&self, control: Control) -> bool {
        self.control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|sender| sender.try_send(control).is_ok())
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// `timeout` is how long a recording waits for someone to start speaking; every recording that opens
    /// the microphone is announced on `events`.
    #[must_use]
    pub fn new(
        hearing: H,
        capture: CaptureThread,
        listen: Arc<dyn Listen>,
        voices: Arc<dyn Voices>,
        learning: Arc<dyn Learning>,
        timeout: Duration,
        events: broadcast::Sender<DaemonEvent>,
    ) -> Self {
        Self {
            hearing,
            capture,
            listen,
            voices,
            learning,
            timeout,
            events,
            control: Mutex::new(None),
        }
    }
}

impl<H: Hearing> Record for Recorder<H> {
    fn record(&self, take: Take) -> BoxFuture<'_, Result<Learned, RecordError>> {
        Box::pin(self.record_take(take))
    }

    fn finish(&self) -> bool {
        self.common_send(Control::Finish)
    }

    fn cancel(&self) -> bool {
        self.common_send(Control::Cancel)
    }
}
