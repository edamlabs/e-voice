use std::fmt::{self, Debug};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use e_voice_device::capture::base::Capture;
use e_voice_device::error::DeviceError;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

/// Blocks a microphone may hold ahead of the listener: past them a block is dropped and counted. A cpal
/// block is ~10 ms, so the listener may fall ~0.6 s behind before audio is lost.
const CAPTURE: usize = 64;

/// What an input device is handed to deliver its blocks with, on its own audio thread.
pub type Deliver = Box<dyn FnMut(&[f32]) + Send>;

type Opener = dyn Fn(Deliver) -> Result<Box<dyn Capture>, DeviceError> + Send + Sync;

/// Opens the input device for one listen at a time on a thread of its own: a cpal stream must stay on
/// the thread that opened it, and the microphone is held only while someone listens.
#[derive(Clone)]
pub struct CaptureThread {
    open: Arc<Opener>,
}

impl Debug for CaptureThread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaptureThread").finish_non_exhaustive()
    }
}

impl CaptureThread {
    #[must_use]
    pub fn new<F>(open: F) -> Self
    where
        F: Fn(Deliver) -> Result<Box<dyn Capture>, DeviceError> + Send + Sync + 'static,
    {
        Self { open: Arc::new(open) }
    }

    /// Opens the device; it stays open until the returned microphone drops. The thread owns the stream
    /// and waits on a channel whose only sender is the microphone, so dropping it closes the device.
    ///
    /// # Errors
    /// The device cannot be opened, or reports no sample rate; a thread that is gone reads as no input.
    pub async fn open(&self) -> Result<Microphone, DeviceError> {
        let (sender, blocks) = mpsc::channel(CAPTURE);
        let dropped = Arc::new(AtomicUsize::new(0));
        let tally = Arc::clone(&dropped);
        let deliver: Deliver = Box::new(move |samples: &[f32]| {
            if let Err(TrySendError::Full(_)) = sender.try_send(samples.to_vec()) {
                tally.fetch_add(1, Ordering::Relaxed);
            }
        });
        let (reply, opened) = oneshot::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let open = Arc::clone(&self.open);
        thread::spawn(move || match open(deliver) {
            Ok(capture) => {
                let rate = NonZeroU32::new(capture.rate()).ok_or(DeviceError::Input(None));
                reply.send(rate).ok();
                held.recv().ok();
            }
            Err(error) => {
                reply.send(Err(error)).ok();
            }
        });
        let rate = opened.await.unwrap_or(Err(DeviceError::Input(None)))?;
        Ok(Microphone {
            rate,
            blocks,
            dropped,
            _release: release,
        })
    }
}

/// An open input: mono blocks at `rate`, and how many the listener was too slow to take.
#[derive(Debug)]
pub struct Microphone {
    pub rate: NonZeroU32,
    blocks: mpsc::Receiver<Vec<f32>>,
    dropped: Arc<AtomicUsize>,
    _release: std::sync::mpsc::Sender<()>,
}

impl Microphone {
    /// The next block; `None` once the device thread is gone.
    pub async fn recv(&mut self) -> Option<Vec<f32>> {
        self.blocks.recv().await
    }

    /// Blocks dropped so far because the listener fell [`CAPTURE`] blocks behind.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}
