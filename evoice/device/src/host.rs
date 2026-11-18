use cpal::Host;
use cpal::traits::{DeviceTrait, HostTrait};

use crate::error::DeviceError;

/// PulseAudio names the loopback of every output "Monitor of …": what the speakers play, not a microphone.
pub const MONITOR: &str = "Monitor of";

/// The audio host every device of this crate opens on.
#[derive(Debug)]
pub struct CpalHost;

impl CpalHost {
    /// PulseAudio when present (served by PipeWire on modern Linux) names real sources; otherwise the
    /// platform default (ALSA, CoreAudio, WASAPI).
    #[must_use]
    pub fn pick() -> Host {
        cpal::available_hosts()
            .into_iter()
            .find(|id| id.name() == "PulseAudio")
            .and_then(|id| cpal::host_from_id(id).ok())
            .unwrap_or_else(cpal::default_host)
    }

    /// `(name, is_default)` for every input device of [`CpalHost::pick`].
    ///
    /// # Errors
    /// The audio backend cannot enumerate devices.
    pub fn inputs() -> Result<Vec<(String, bool)>, DeviceError> {
        let host = Self::pick();
        let default = host
            .default_input_device()
            .and_then(|device| device.description().ok())
            .map(|d| d.name().to_owned());
        Ok(host
            .input_devices()?
            .filter_map(|device| {
                device
                    .description()
                    .ok()
                    .map(|description| description.name().to_owned())
            })
            .map(|name| {
                let chosen = default.as_deref() == Some(name.as_str());
                (name, chosen)
            })
            .collect())
    }
}
