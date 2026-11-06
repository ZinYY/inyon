//! Port of the Bun hub's terminal folder (removed in 0.23): the phone's terminal on a pane.

pub mod mirror;
pub(crate) mod psmux;
#[allow(clippy::module_inception)]
pub mod terminal;
