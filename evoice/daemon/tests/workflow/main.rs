#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used,
    clippy::cast_precision_loss
)]

mod audio;
mod backpressure;
mod capture;
#[path = "../fake/mod.rs"]
mod fake;
mod listen;
mod queue;
mod stop;
