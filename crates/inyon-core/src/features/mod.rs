//! Port of the Bun hub's features folder (removed in 0.23): what the app asks a machine for beyond the chat. Each module is
//! the port of the TS file of the same name; everything runs as shell on the machine through
//! `Host::exec`, so it works the same for this machine, an SSH server, a joined machine or the
//! phone's own link.

pub mod agent_latest;
// the companion agent: a session you talk to that reads and steers the others
pub mod companion;
pub mod conversations;
// features/features.ts, named as the TS file is
#[allow(clippy::module_inception)]
pub mod features;
pub mod models;
// a page's research notes as pages to read
pub mod notes;
// how sessions are organised (pins, parking, snoozes, tags, notes, projects), kept for every device
pub mod organize;
// the rules for serving a page (pages-core.ts): no shell, no disk; the hub brings its own
pub mod pages_core;
// pages on other machines, through their own `inyon pages`: the hub's joined machines, the phone's computers
pub mod pages_remote;
pub mod paths;
pub mod providers;
pub mod setup;
pub mod signin;
// agent temp files, through the machine's own `inyon temp`
pub mod temp;
// uploads sent a chunk at a time, resumable (from 0.24)
pub mod uploads;
pub mod workspace;
