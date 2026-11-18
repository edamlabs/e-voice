#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

mod connect;
#[path = "../fake/mod.rs"]
mod fake;
mod health;
mod http;
mod listen;
mod running;
mod server;
mod speak;
mod stop;
