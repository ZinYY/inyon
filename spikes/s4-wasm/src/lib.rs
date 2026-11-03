//! Spike S4: the phone's link (packages/link) in Rust, built for wasm32-unknown-unknown and run
//! in a browser against the TypeScript machine side.

pub mod identity;
pub mod link;
#[cfg(target_arch = "wasm32")]
pub mod web;
