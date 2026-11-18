//! The emotion vocabulary lives in core, so a client of this service (the daemon) reads it typed without
//! depending on this crate; these paths stay as they were.
pub use e_voice_core::schema::emotion::{Emotion, EmotionLabel};
