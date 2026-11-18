#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss
)]
#![allow(clippy::cast_possible_truncation, clippy::identity_op, clippy::cast_lossless)]

#[allow(dead_code)]
#[path = "../support/flac.rs"]
mod flac;
mod ingest;
mod silence;
