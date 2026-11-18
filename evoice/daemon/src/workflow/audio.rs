use std::num::NonZeroU32;
use std::sync::mpsc;
use std::thread;

use e_voice_device::error::DeviceError;
use e_voice_device::playback::base::Playback;
use tokio::sync::oneshot;

/// What the audio thread is asked to do.
enum Order {
    Open(oneshot::Sender<Result<(), DeviceError>>),
    Push(Vec<f32>, NonZeroU32),
    Pending(oneshot::Sender<f64>),
}

/// The thread that owns the output device: a cpal stream must stay on the thread that opened it, so
/// every call crosses a channel. Nothing opens at spawn, so the daemon starts without a sound card; the
/// device opens on the first [`AudioThread::open`], and a failure goes back to that caller and is tried
/// again on the next call, never kept. The thread ends when the handle drops.
#[derive(Debug)]
pub struct AudioThread {
    orders: mpsc::Sender<Order>,
}

impl AudioThread {
    // ##### PRIVATE #####

    /// Carries the device, open or not, from one order to the next until the handle drops.
    fn spawn_serve<P: Playback>(inbox: &mpsc::Receiver<Order>, mut open: impl FnMut() -> Result<P, DeviceError>) {
        inbox
            .iter()
            .fold(None, |device, order| Self::spawn_order(device, order, &mut open));
    }

    fn spawn_order<P: Playback>(
        device: Option<P>,
        order: Order,
        open: &mut impl FnMut() -> Result<P, DeviceError>,
    ) -> Option<P> {
        match order {
            Order::Open(reply) => {
                let (device, opened) = match device {
                    Some(ready) => (Some(ready), Ok(())),
                    None => open().map_or_else(|error| (None, Err(error)), |ready| (Some(ready), Ok(()))),
                };
                reply.send(opened).ok();
                device
            }
            Order::Push(samples, rate) => {
                if let Some(ready) = &device {
                    ready.push(&samples, rate);
                }
                device
            }
            Order::Pending(reply) => {
                reply.send(device.as_ref().map_or(0.0, Playback::pending)).ok();
                device
            }
        }
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Starts the thread; `open` runs on it.
    #[must_use]
    pub fn spawn<P, F>(open: F) -> Self
    where
        P: Playback + 'static,
        F: FnMut() -> Result<P, DeviceError> + Send + 'static,
    {
        let (orders, inbox) = mpsc::channel();
        thread::spawn(move || Self::spawn_serve(&inbox, open));
        Self { orders }
    }

    /// Opens the device unless it already is.
    ///
    /// # Errors
    /// The device cannot be opened; a thread that is gone reads as no output device.
    pub async fn open(&self) -> Result<(), DeviceError> {
        let (reply, opened) = oneshot::channel();
        self.orders.send(Order::Open(reply)).map_err(|_| DeviceError::Output)?;
        opened.await.unwrap_or(Err(DeviceError::Output))
    }

    /// Queues `samples` recorded at `rate`; dropped while the device is not open.
    pub fn push(&self, samples: Vec<f32>, rate: NonZeroU32) {
        self.orders.send(Order::Push(samples, rate)).ok();
    }

    /// Seconds the device still has to play; zero when it is not open.
    pub async fn pending(&self) -> f64 {
        let (reply, pending) = oneshot::channel();
        self.orders.send(Order::Pending(reply)).ok();
        pending.await.unwrap_or(0.0)
    }
}
