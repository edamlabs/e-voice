#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used,
    clippy::cast_precision_loss
)]

#[allow(dead_code)]
#[path = "../mcp/client.rs"]
mod client;
mod doors;
mod exits;
#[path = "../fake/mod.rs"]
mod fake;
mod harness;
#[allow(dead_code)]
#[path = "../api/http.rs"]
mod http;
mod loops;
mod permissions;
#[allow(dead_code)]
#[path = "../api/running.rs"]
mod running;
