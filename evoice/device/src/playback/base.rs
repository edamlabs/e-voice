use std::fmt::Debug;
use std::num::NonZeroU32;

/// An open output fed mono f32 at any rate.
pub trait Playback: Debug {
    /// Queues `samples` recorded at `rate`.
    fn push(&self, samples: &[f32], rate: NonZeroU32);
    /// Drops whatever has not been played yet (barge-in).
    fn clear(&self);
    /// Seconds still queued.
    fn pending(&self) -> f64;
}
