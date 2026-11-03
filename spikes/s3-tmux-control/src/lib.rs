//! A tmux control-mode client (`tmux -C`), the piece Inyon's hub uses today through
//! `apps/hub/src/tmux/live.ts`, extended to run commands over the same connection.
//!
//! The protocol part ([`proto`], [`escape`]) is sans-io: it takes lines and gives events, so the
//! same code works over a local process, an ssh channel or a node-link stream. [`client`] adds a
//! thread-based transport over any reader/writer pair, and [`tmux`] the commands Inyon runs.

pub mod client;
pub mod escape;
pub mod proto;
pub mod tmux;

pub use client::{Control, Error};
pub use proto::{Event, Notification, Parser, Reply};
