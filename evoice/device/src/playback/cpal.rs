use std::fmt::{self, Debug};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};

use ::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ::cpal::{Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::error::DeviceError;
use crate::host::CpalHost;
use crate::playback::base::Playback;
use crate::playback::queue::PlaybackQueue;

/// An output device (named, or the system default) fed mono f32 at any rate through a
/// [`PlaybackQueue`]. The stream must stay alive on the thread that opened it.
pub struct CpalPlayback {
    _stream: Stream,
    queue: Arc<Mutex<PlaybackQueue>>,
    pub rate: u32,
}

impl Debug for CpalPlayback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpalPlayback")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl CpalPlayback {
    // ##### PRIVATE #####

    fn open_typed<T, E>(
        device: &Device,
        config: StreamConfig,
        queue: Arc<Mutex<PlaybackQueue>>,
        fail: E,
    ) -> Result<Stream, ::cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
        E: FnMut(::cpal::Error) + Send + 'static,
    {
        let channels = usize::from(config.channels).max(1);
        device.build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                for frame in data.chunks_mut(channels) {
                    let sample = T::from_sample(queue.pop().unwrap_or(0.0));
                    for slot in frame.iter_mut() {
                        *slot = sample;
                    }
                }
            },
            fail,
            None,
        )
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// The output whose name contains `name`, or the system default without one. `fail` receives the
    /// errors the stream reports after it started.
    ///
    /// # Errors
    /// No matching output device, an unsupported sample format, or the backend refusing the stream.
    pub fn open<E>(name: Option<&str>, fail: E) -> Result<Self, DeviceError>
    where
        E: FnMut(::cpal::Error) + Send + 'static,
    {
        let host = CpalHost::pick();
        let device = match name {
            None => host.default_output_device(),
            Some(wanted) => {
                let wanted = wanted.to_lowercase();
                host.output_devices()?.find(|device| {
                    device
                        .description()
                        .is_ok_and(|description| description.name().to_lowercase().contains(&wanted))
                })
            }
        }
        .ok_or(DeviceError::Output)?;
        let supported = device.default_output_config()?;
        let config = supported.config();
        let queue = Arc::new(Mutex::new(PlaybackQueue::default()));
        let stream = match supported.sample_format() {
            SampleFormat::I16 => Self::open_typed::<i16, _>(&device, config, Arc::clone(&queue), fail)?,
            SampleFormat::I32 => Self::open_typed::<i32, _>(&device, config, Arc::clone(&queue), fail)?,
            SampleFormat::U16 => Self::open_typed::<u16, _>(&device, config, Arc::clone(&queue), fail)?,
            SampleFormat::F32 => Self::open_typed::<f32, _>(&device, config, Arc::clone(&queue), fail)?,
            other => return Err(DeviceError::Format(other)),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            queue,
            rate: config.sample_rate,
        })
    }
}

impl Playback for CpalPlayback {
    fn push(&self, samples: &[f32], rate: NonZeroU32) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(samples, rate, self.rate);
    }

    fn clear(&self) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    #[allow(clippy::cast_precision_loss)]
    fn pending(&self) -> f64 {
        let queued = self.queue.lock().unwrap_or_else(PoisonError::into_inner).len();
        queued as f64 / f64::from(self.rate.max(1))
    }
}
