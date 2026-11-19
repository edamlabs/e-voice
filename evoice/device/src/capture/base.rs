use std::fmt::Debug;

/// An open input: mono f32 blocks reach the callback it was opened with; dropping it stops capture.
pub trait Capture: Debug {
    /// Rate of the delivered samples.
    fn rate(&self) -> u32;
    /// The device's name.
    fn name(&self) -> &str;
    /// Peak of the latest block.
    fn level(&self) -> f32;
}
