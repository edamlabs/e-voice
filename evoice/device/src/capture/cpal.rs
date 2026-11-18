use std::fmt::{self, Debug};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ::cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::capture::base::Capture;
use crate::error::DeviceError;
use crate::host::{CpalHost, MONITOR};

/// An input device (named, or the system default) captured as mono f32 at its own rate; every block
/// goes to the `deliver` callback it was opened with, on the audio thread.
/// `level` holds the peak of the latest callback as `f32` bits, for a live meter.
/// The stream must stay alive on the thread that opened it.
pub struct CpalCapture {
    _stream: Stream,
    pub rate: u32,
    pub name: String,
    pub level: Arc<AtomicU32>,
}

impl Debug for CpalCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpalCapture")
            .field("name", &self.name)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl CpalCapture {
    // ##### PRIVATE #####

    #[allow(clippy::cast_precision_loss)]
    fn open_typed<T, F, E>(
        device: &Device,
        config: StreamConfig,
        mut deliver: F,
        fail: E,
        level: Arc<AtomicU32>,
    ) -> Result<Stream, ::cpal::Error>
    where
        T: SizedSample,
        f32: FromSample<T>,
        F: FnMut(&[f32]) + Send + 'static,
        E: FnMut(::cpal::Error) + Send + 'static,
    {
        let channels = usize::from(config.channels).max(1);
        let scale = 1.0 / channels as f32;
        device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| frame.iter().map(|sample| f32::from_sample(*sample)).sum::<f32>() * scale)
                    .collect();
                let peak = mono.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                level.store(peak.to_bits(), Ordering::Relaxed);
                deliver(&mono);
            },
            fail,
            None,
        )
    }

    // ##########################################################

    // ##### PUBLIC #####

    /// Without a name, the system default input — unless it is a playback monitor (a Bluetooth headset
    /// in its music profile has no microphone, so PulseAudio falls back to its monitor), then the first
    /// real input. `fail` receives the errors the stream reports after it started.
    ///
    /// # Errors
    /// No matching device, an unsupported sample format, or the audio backend refusing the stream.
    pub fn open<F, E>(name: Option<&str>, deliver: F, fail: E) -> Result<Self, DeviceError>
    where
        F: FnMut(&[f32]) + Send + 'static,
        E: FnMut(::cpal::Error) + Send + 'static,
    {
        let host = CpalHost::pick();
        let monitor = |device: &Device| {
            device
                .description()
                .is_ok_and(|description| description.name().starts_with(MONITOR))
        };
        let device = match name {
            None => host
                .default_input_device()
                .filter(|device| !monitor(device))
                .or_else(|| host.input_devices().ok()?.find(|device| !monitor(device)))
                .or_else(|| host.default_input_device()),
            Some(wanted) => {
                let wanted = wanted.to_lowercase();
                host.input_devices()?.find(|device| {
                    device
                        .description()
                        .is_ok_and(|description| description.name().to_lowercase().contains(&wanted))
                })
            }
        }
        .ok_or_else(|| DeviceError::Input(name.map(str::to_owned)))?;
        let supported = device.default_input_config()?;
        let config = supported.config();
        let level = Arc::new(AtomicU32::new(0));
        let tap = Arc::clone(&level);
        let stream = match supported.sample_format() {
            SampleFormat::I16 => Self::open_typed::<i16, _, _>(&device, config, deliver, fail, tap)?,
            SampleFormat::I32 => Self::open_typed::<i32, _, _>(&device, config, deliver, fail, tap)?,
            SampleFormat::U16 => Self::open_typed::<u16, _, _>(&device, config, deliver, fail, tap)?,
            SampleFormat::F32 => Self::open_typed::<f32, _, _>(&device, config, deliver, fail, tap)?,
            other => return Err(DeviceError::Format(other)),
        };
        stream.play()?;
        let name = device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_default();
        Ok(Self {
            _stream: stream,
            rate: config.sample_rate,
            name,
            level,
        })
    }
}

impl Capture for CpalCapture {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}
