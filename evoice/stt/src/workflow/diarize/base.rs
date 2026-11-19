use std::fmt::Debug;

use e_voice_core::schema::error::NodeError;

use crate::schema::speaker::Turn;

/// Shared diarization backend over one whole 16 kHz mono file; blocking, call off the async threads.
pub trait Diarize: Send + Sync + Debug {
    /// Turns sorted by start; speaker ids are the backend's own, renumbered at the join.
    ///
    /// # Errors
    /// The runtime producing no result, or one that cannot be read back.
    fn diarize(&self, audio: &[f32]) -> Result<Vec<Turn>, NodeError>;
}
