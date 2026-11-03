//! The hub in Rust: the Bun hub's HTTP server (index.ts and its routes) on inyon-core, the same
//! records, routes, answers, cookies and checks, so the web app, the phone app and machines
//! connect to it as they did to the Bun hub. `inyon` runs it, by default from 0.21 and only it
//! from 0.23, when the Bun hub was removed.
//!
//! Everything the hub keeps lives on one thread with one current-thread tokio runtime, the
//! listener included; see `server` for why.

// the API itself, shared with the phone app
pub use inyon_api::{clients, core_routes, http, multipart};

pub mod auth;
pub mod boards;
pub mod cli;
pub mod companions;
pub mod finding;
pub mod hostnames;
pub mod hub_update;
pub mod idle;
pub mod index;
pub mod limits;
pub mod machines;
pub mod node_dist;
#[cfg(test)]
mod organize_tests;
pub mod pages;
pub mod pages_stream;
// both live in the core, which the phone app runs too
pub use inyon_core::features::{notes, pages_remote};
pub mod password;
pub mod phones;
pub mod push;
pub mod server;
pub mod start;
pub mod static_files;
#[cfg(test)]
pub mod test_util;
pub mod tls;
pub mod ws;

/// `inyon-engine hub …`: the arguments after `hub`; the process's exit code.
pub fn run(args: &[String]) -> i32 {
    cli::run(args)
}
