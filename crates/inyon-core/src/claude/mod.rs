//! Port of the Bun hub's claude folder (removed in 0.23): the screen reader, transcripts, drafts and delivery, the
//! integration scripts, and the actions and watchers that drive an agent.

pub mod actions;
pub mod agent_hooks;
pub mod chat_cache;
pub mod chosen;
pub mod delivery;
pub mod drafts;
pub mod driving;
pub mod held;
pub mod integration;
pub mod integration_sh;
pub mod keep_default;
pub mod last_reply;
pub mod screen;
pub mod screen_reply;
pub mod send_check;
pub mod send_receipts;
pub mod transcript;
pub mod transcript_tail;
pub mod watcher;
