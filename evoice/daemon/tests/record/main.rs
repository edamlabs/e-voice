#![allow(
    clippy::arithmetic_side_effects,
    clippy::assert_is_empty,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used,
    clippy::cast_precision_loss
)]

mod busy;
mod cancel;
#[allow(dead_code)]
#[path = "../mcp/client.rs"]
mod client;
mod doors;
mod events;
#[path = "../fake/mod.rs"]
mod fake;
#[allow(dead_code)]
#[path = "../api/http.rs"]
mod http;
mod rig;
#[allow(dead_code)]
#[path = "../api/running.rs"]
mod running;
mod stops;
mod take;
