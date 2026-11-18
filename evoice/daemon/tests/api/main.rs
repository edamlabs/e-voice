#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

mod connect;
mod events;
#[path = "../fake/mod.rs"]
mod fake;
mod health;
mod http;
mod listen;
#[allow(dead_code)]
#[path = "../../../core/tests/support/rawws.rs"]
mod rawws;
mod running;
mod server;
mod speak;
mod stop;
