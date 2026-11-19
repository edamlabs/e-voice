#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used,
    clippy::cast_precision_loss
)]

mod approve;
mod client;
mod debug;
#[path = "../fake/mod.rs"]
mod fake;
#[path = "../api/http.rs"]
mod http;
mod protocol;
#[allow(dead_code)]
#[path = "../api/running.rs"]
mod running;
mod same;
mod tools;
mod voice;
