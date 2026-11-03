//! Port of the Bun hub's platform folder (removed in 0.23): what the stateful core needs from the platform it runs on. The
//! core does no I/O of its own; the hub (crates/inyon-backends) and the phone app plug these in.

pub mod console;
pub mod fetch;
pub mod rt;
pub mod store;
