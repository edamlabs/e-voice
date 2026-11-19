use std::fmt::Debug;
use std::num::NonZeroU32;

use crate::error::DeviceError;

/// An open output fed mono f32 at any rate.
pub trait Playback: Debug {
    /// Queues `samples` recorded at `rate`.
    ///
    /// # Errors
    /// The samples cannot be resampled to the device's rate.
    fn push(&self, samples: &[f32], rate: NonZeroU32) -> Result<(), DeviceError>;
    /// Ends a stream so its last samples play; the next push starts a new one.
    ///
    /// # Errors
    /// The resampler fails on its last chunk.
    fn finish(&self) -> Result<(), DeviceError>;
    /// Drops whatever has not been played yet (barge-in).
    fn clear(&self);
    /// Seconds still queued.
    fn pending(&self) -> f64;
}
