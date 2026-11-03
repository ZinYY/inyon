//! What the hub's core (`inyon-core`) needs from the machine it runs on, for the Rust hub: its event
//! loop, commands run directly or through the system `ssh`, terminals in a pseudo-terminal, and its
//! records as files under ~/.inyon.
//!
//! These live outside `inyon-core` on purpose. The core is shared with the phone app, which builds
//! it for wasm32 and has none of this: no processes, no ssh, no file system, and its own event
//! loop. The core only states what it needs as traits ([`inyon_core::platform::rt::Runtime`],
//! [`inyon_core::hosts::host::HostBackend`], [`inyon_core::platform::store::Store`]) and each
//! platform plugs in its own. Keeping tokio's process and file code here keeps it out of the wasm
//! build, and keeps the core's state machines testable with a fake clock and a fake machine.
//!
//! The threading model follows Bun's. The core runs on one thread ([`core_thread`]): a tokio
//! current-thread runtime with a `LocalSet`, so every task, timer and callback of the core runs
//! there and a synchronous stretch of the core is never interleaved with another task, which the TS
//! relies on. Other threads (an HTTP server, say) hand it work through a [`CoreHandle`], in order.
//! Blocking work (a pseudo-terminal's reads, waiting on its child) runs on threads of its own and
//! comes back to the core's thread through channels.
//!
//! | TypeScript | Rust |
//! |---|---|
//! | Bun's event loop | [`runtime`] |
//! | `hosts/process-backend.ts` | [`process`], [`pty`] |
//! | `platform/fs-store.ts` | [`fs_store`] |
//! | `env.ts`, `legacy.ts` | [`env`] |

pub mod env;
pub mod fs_store;
#[cfg(unix)]
pub mod limits;
pub mod process;
pub mod pty;
pub mod runtime;

pub use runtime::{core_here, core_thread, CoreHandle};
