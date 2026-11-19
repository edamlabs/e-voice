#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

mod answer;
mod approval;
mod chunker;
#[path = "../fake/mod.rs"]
mod fake;
mod prompt;
mod property;
mod question;
