//! Port of the Bun hub's tmux folder (removed in 0.23): reading what tmux and the machine's probe print.

pub mod grid;
pub mod machine_kind;
#[allow(clippy::module_inception)]
pub mod tmux;
