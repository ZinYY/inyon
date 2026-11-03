//! The app's view model, shared by every screen of the Dioxus app: the conversation as rows that
//! change by splices (so a virtualized list updates in place), the overview of machines and panes,
//! messages still on their way, and the notices a hub message raises. A port of the pure parts of
//! the React app's `apps/web/src/lib`, module for module, with its tests:
//!
//! | TypeScript | Rust |
//! |---|---|
//! | `chat.ts` | [`chat`] |
//! | `tools.ts` | [`tools`] |
//! | `format.ts` | [`format`] |
//! | `pending.ts` | [`pending`] |
//! | `overview.ts` | [`overview`] |
//! | `groups.ts` | [`groups`] |
//! | `store.ts` (the chat reducer), `snapshot.ts` (what a kept page holds), ChatView's rows | [`chat_list`] |
//! | `store.ts` (overview, notices, toasts, activity, settings), `tellApart` in `hubs.ts` | [`store`] |
//! | (new) protocol v2 frames from `/ws2` into the same rows and overview | [`v2`] |
//! | `upload.ts` (chunked uploads: routes, the next chunk, what an answer means) | [`upload`] |
//! | (new) pinned, parked and snoozed sessions, projects and tags, as the hub keeps them | [`organize`] |
//!
//! Nothing here touches the network, storage, the clock or the screen. Time comes in as `now`
//! (unix ms), and what the TS did to the page comes back as [`store::Effect`]s. It builds for
//! wasm32 as well as natively, and depends on no UI framework.

pub mod chat;
pub mod chat_list;
pub mod format;
pub mod groups;
pub mod js;
pub mod organize;
pub mod overview;
pub mod pending;
pub mod store;
pub mod tools;
pub mod upload;
pub mod v2;

#[cfg(test)]
pub(crate) mod test_support;

pub use chat_list::{apply_splices, Chat, ChatData, ListRow, Splice};
pub use store::{Context, Effect, Store, Update};
