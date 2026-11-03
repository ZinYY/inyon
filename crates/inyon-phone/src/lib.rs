//! Inyon's core in the phone app and in the browser copy, as wasm: inyon-api's routes and the
//! app's sockets on the browser's event loop, with the app's storage and `fetch`. The web app
//! loads it as @inyon/phone (see apps/web/vite.config.ts); it replaced the TypeScript core, which
//! was removed in 0.23, with the same `startPhone`.

mod computers;
pub mod keys;
pub mod link;
#[cfg(feature = "native")]
pub mod native;
pub mod pages;
pub mod phone;
pub mod push;
mod servers;
pub mod ssh;
mod tables;
#[cfg(test)]
mod testing;
pub mod web;
