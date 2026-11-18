#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

mod app;
mod deepgram;
mod diarize;
mod docs;
mod duration;
mod elevenlabs;
#[allow(dead_code)]
#[path = "../workflow/fake.rs"]
mod fake;
#[allow(dead_code)]
#[path = "../../../core/tests/support/flac.rs"]
mod flac;
mod health;
mod lifespan;
mod rate;
mod realtime;
mod render;
mod stream;
mod transcriptions;
