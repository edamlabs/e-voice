#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

mod audio;
#[path = "../fake/mod.rs"]
mod fake;
mod queue;
