use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use e_voice_core::audio::{AudioEncoding, AudioError, AudioIngest};
use e_voice_core::schema::lang::Lang;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::api::state::AppState;
use crate::schema::audio::{Audio, RATE};
use crate::schema::event::Event;
use crate::schema::transcript::Transcript;
use crate::workflow::runner::{RunnerError, RunnerIntake};

pub const NORMAL: u16 = 1000;
pub const UNSUPPORTED: u16 = 1003;
/// RFC 6455 «Message Too Big».
pub const TOO_BIG: u16 = 1009;
pub const INTERNAL: u16 = 1011;
/// How long a socket closed with 1009 stays open before it is dropped. The refused frame's payload may
/// still be arriving; dropping at once makes the kernel reset the connection, and the reset can reach
/// the client before the close does. Nothing is read meanwhile.
pub const LINGER: Duration = Duration::from_secs(1);

/// The largest message or frame an audio socket takes: 2 MiB, over a second of the highest rate
/// accepted (`MAX_RATE`) in f32. Realtime clients send tens of milliseconds at a time.
pub const AUDIO_MESSAGE: usize = 2_097_152;

/// What a client frame means, in protocol-neutral terms.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Audio(Vec<u8>),
    Configure {
        lang: Option<Lang>,
        rate: Option<u32>,
        encoding: Option<AudioEncoding>,
    },
    Reply(Message),
    End,
    Close,
}

/// How a live run ended, for the protocol's goodbye and the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveOutcome {
    pub code: u16,
    pub seconds: f64,
    pub peak_db: f32,
    pub ended: bool,
}

/// One realtime wire protocol over the shared pipeline: native events, OpenAI Realtime or Deepgram.
pub trait LiveProtocol: Send {
    /// Frames sent right after the upgrade.
    fn greet(&mut self) -> Vec<Message>;
    fn parse(&mut self, message: Message) -> Vec<Inbound>;
    fn render(&mut self, event: &Event) -> Vec<Message>;
    /// Frames sent before the close frame.
    fn farewell(&mut self, outcome: &LiveOutcome) -> Vec<Message>;
}

/// Whether the socket keeps being read after one inbound step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Read,
    Stop,
    Refuse,
}

struct LiveRun {
    ingest: AudioIngest,
    audio: Option<mpsc::Sender<Audio>>,
    task: JoinHandle<Result<(), RunnerError>>,
}

/// Socket ⇄ runner bridge shared by every live protocol. The run opens lazily on the first audio,
/// so a configuring first message can still set language, rate and encoding.
#[derive(Debug)]
pub struct LiveStream {
    lang: Lang,
    rate: u32,
    encoding: AudioEncoding,
    cap: usize,
    received: usize,
    peak: f32,
    ended: bool,
}

impl LiveStream {
    // ##### PRIVATE #####

    /// Whether a read failed because the client's message or frame passed the socket's cap.
    fn serve_oversized(error: &axum::Error) -> bool {
        std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<tungstenite::Error>())
            .is_some_and(|error| matches!(error, tungstenite::Error::Capacity(_)))
    }

    fn serve_open(
        &self,
        state: &AppState,
        cancel: &CancellationToken,
    ) -> Result<(LiveRun, mpsc::Receiver<Event>), AudioError> {
        let ingest = AudioIngest::new(self.rate, RATE, self.encoding)?;
        let (audio_tx, audio_rx) = mpsc::channel(64);
        let (events_tx, events_rx) = mpsc::channel(256);
        let (runner, lang, token) = (state.runner.clone(), self.lang, cancel.clone());
        let task = tokio::spawn(async move { runner.run(lang, RunnerIntake::Live, audio_rx, events_tx, token).await });
        tracing::info!(lang = ?self.lang, rate = self.rate, "stream.open");
        Ok((
            LiveRun {
                ingest,
                audio: Some(audio_tx),
                task,
            },
            events_rx,
        ))
    }

    async fn serve_audio(&mut self, run: &mut LiveRun, samples: Result<Vec<f32>, AudioError>) -> bool {
        match samples {
            Ok(samples) if samples.is_empty() => true,
            Ok(samples) => {
                self.received = self.received.saturating_add(samples.len());
                self.peak = samples.iter().fold(self.peak, |peak, sample| peak.max(sample.abs()));
                match run.audio.as_ref() {
                    Some(audio) => audio.send(Audio::from(samples)).await.is_ok(),
                    None => false,
                }
            }
            Err(error) => {
                tracing::warn!(%error, "stream.audio");
                false
            }
        }
    }

    async fn serve_step(
        &mut self,
        step: Inbound,
        state: &AppState,
        cancel: &CancellationToken,
        run: &mut Option<LiveRun>,
        events: &mut Option<mpsc::Receiver<Event>>,
        sink: &mut SplitSink<WebSocket, Message>,
    ) -> Flow {
        if matches!(step, Inbound::Audio(_) | Inbound::End) && run.is_none() {
            match self.serve_open(state, cancel) {
                Ok((started, stream)) => {
                    *run = Some(started);
                    *events = Some(stream);
                }
                Err(error) => {
                    tracing::warn!(%error, "stream.refused");
                    return Flow::Refuse;
                }
            }
        }
        match (step, run.as_mut()) {
            (Inbound::Audio(bytes), Some(active)) => {
                let samples = active.ingest.push(&bytes);
                if self.serve_audio(active, samples).await {
                    Flow::Read
                } else {
                    cancel.cancel();
                    Flow::Stop
                }
            }
            (Inbound::End, Some(active)) => {
                let samples = active.ingest.flush();
                self.serve_audio(active, samples).await;
                active.audio = None;
                self.ended = true;
                Flow::Stop
            }
            (Inbound::Configure { lang, rate, encoding }, None) => {
                self.lang = lang.unwrap_or(self.lang);
                self.rate = rate.unwrap_or(self.rate);
                self.encoding = encoding.unwrap_or(self.encoding);
                Flow::Read
            }
            (Inbound::Reply(frame), _) => {
                sink.send(frame).await.ok();
                Flow::Read
            }
            (Inbound::Close, _) => {
                cancel.cancel();
                Flow::Stop
            }
            (Inbound::Configure { .. } | Inbound::Audio(_) | Inbound::End, _) => Flow::Read,
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    #[must_use]
    /// `cap` is the socket's message limit, named in the close reason when a client passes it.
    pub const fn new(lang: Lang, rate: u32, encoding: AudioEncoding, cap: usize) -> Self {
        Self {
            lang,
            rate,
            encoding,
            cap,
            received: 0,
            peak: 0.0,
            ended: false,
        }
    }

    /// Runs the protocol until the pipeline closes: `End` drains pending segments, a client close or
    /// server shutdown cancels them; either way every segment still gets its final.
    pub async fn serve(mut self, state: AppState, socket: WebSocket, mut protocol: impl LiveProtocol) {
        let (mut sink, mut source) = socket.split();
        for frame in protocol.greet() {
            sink.send(frame).await.ok();
        }
        let cancel = state.shutdown.child_token();
        let (mut run, mut events): (Option<LiveRun>, Option<mpsc::Receiver<Event>>) = (None, None);
        let (mut reading, mut code) = (true, NORMAL);
        loop {
            let pending = events.is_some();
            tokio::select! {
                message = source.next(), if reading => {
                    let inbound = match message {
                        Some(Ok(message)) => protocol.parse(message),
                        Some(Err(error)) => {
                            code = if Self::serve_oversized(&error) { TOO_BIG } else { code };
                            vec![Inbound::Close]
                        }
                        None => vec![Inbound::Close],
                    };
                    for step in inbound {
                        match self.serve_step(step, &state, &cancel, &mut run, &mut events, &mut sink).await {
                            Flow::Read => {}
                            Flow::Stop => reading = false,
                            Flow::Refuse => {
                                code = UNSUPPORTED;
                                reading = false;
                                break;
                            }
                        }
                    }
                }
                event = async { events.as_mut()?.recv().await }, if pending => match event {
                    Some(event) => {
                        for frame in protocol.render(&event) {
                            if sink.send(frame).await.is_err() {
                                cancel.cancel();
                                reading = false;
                            }
                        }
                    }
                    None => events = None,
                },
            }
            let idle = events.is_none() && (run.is_some() || !reading);
            if idle {
                break;
            }
        }
        if let Some(active) = run {
            drop(active.audio);
            code = match active.task.await {
                Ok(Ok(())) => code,
                Ok(Err(error)) => {
                    tracing::error!(%error, "stream.failed");
                    INTERNAL
                }
                Err(error) => {
                    tracing::error!(%error, "stream.panicked");
                    INTERNAL
                }
            };
        }
        let outcome = LiveOutcome {
            code,
            seconds: Transcript::seconds(self.received as u64),
            peak_db: 20.0 * self.peak.max(f32::MIN_POSITIVE).log10(),
            ended: self.ended,
        };
        for frame in protocol.farewell(&outcome) {
            if let Err(error) = sink.send(frame).await {
                tracing::warn!(%error, code = outcome.code, "stream.farewell.failed");
            }
        }
        let reason = match outcome.code {
            TOO_BIG => format!("message over {} bytes", self.cap),
            _ => String::new(),
        };
        if let Err(error) = sink
            .send(Message::Close(Some(CloseFrame {
                code: outcome.code,
                reason: reason.into(),
            })))
            .await
        {
            tracing::warn!(%error, code = outcome.code, "stream.close.failed");
        }
        if outcome.code == TOO_BIG {
            tokio::time::sleep(LINGER).await;
        }
        tracing::info!(lang = ?self.lang, ended = outcome.ended, code = outcome.code, seconds = outcome.seconds, peak_db = outcome.peak_db, "stream.closed");
    }
}
