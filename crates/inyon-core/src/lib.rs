//! Inyon's hub core: the readers that turn what an agent shows and writes into Inyon's wire
//! types, and the hub's stateful core on top of them (its machines, the chat watchers, the actions
//! on a pane, the phone's terminal). A port of the Bun hub's source (removed in 0.23; v0.22.5
//! has it), file for file and with the same names, so each Rust module reads next to the
//! TypeScript it came from:
//!
//! | TypeScript | Rust |
//! |---|---|
//! | `claude/screen.ts` | [`claude::screen`] |
//! | `claude/transcript.ts`, `claude/transcript-tail.ts` | [`claude::transcript`], [`claude::transcript_tail`] |
//! | `claude/delivery.ts`, `claude/drafts.ts`, `claude/keep-default.ts` | [`claude::delivery`], [`claude::drafts`], [`claude::keep_default`] |
//! | `claude/watcher.ts`, `claude/chat-cache.ts`, `claude/last-reply.ts`, `claude/send-receipts.ts` | [`claude::watcher`], [`claude::chat_cache`], [`claude::last_reply`], [`claude::send_receipts`] |
//! | `claude/actions.ts`, `claude/integration.ts`, `claude/integration-sh.ts` | [`claude::actions`], [`claude::integration`], [`claude::integration_sh`] |
//! | `hosts/host.ts`, `hosts/registry.ts`, `hosts/names.ts`, `hosts/tidy.ts`, `hosts/turns.ts` | [`hosts::host`], [`hosts::registry`], [`hosts::names`], [`hosts::tidy`], [`hosts::turns`] |
//! | `hosts/node-backend.ts`, `hosts/node-link.ts`, `hosts/via-backend.ts`, `hosts/node-update.ts` | [`hosts::node_backend`], [`hosts::node_link`], [`hosts::via_backend`], [`hosts::node_update`] |
//! | `hosts/http-error.ts`, `hosts/coalesce.ts`, `hosts/removed-nodes.ts` | [`hosts::http_error`], [`hosts::coalesce`], [`hosts::removed_nodes`] |
//! | `terminal/terminal.ts`, `terminal/mirror.ts` | [`terminal::terminal`], [`terminal::mirror`] |
//! | `agents/status.ts` | [`agents::status`] |
//! | `agents/codex.ts`, `agents/codex-live.ts` | [`agents::codex`], [`agents::codex_live`] |
//! | `agents/antigravity.ts`, `agents/gemini.ts` | [`agents::antigravity`], [`agents::gemini`] |
//! | `agents/copilot.ts`, `agents/copilot-screen.ts` | [`agents::copilot`], [`agents::copilot_screen`] |
//! | `agents/opencode.ts`, `agents/opencode-screen.ts`, `agents/opencode-db.ts` | [`agents::opencode`], [`agents::opencode_screen`], [`agents::opencode_db`] |
//! | `agents/opencode-models.ts`, `agents/opencode-keys.ts`, `agents/opencode-questions.ts` | [`agents::opencode_models`], [`agents::opencode_keys`], [`agents::opencode_questions`] |
//! | `agents/opencode-controls.ts`, the parsing in `agents/opencode-api.ts` | [`agents::opencode_controls`], [`agents::opencode_api`] |
//! | `tmux/tmux.ts`, `tmux/machine-kind.ts` | [`tmux::tmux`], [`tmux::machine_kind`] |
//!
//! The core does no I/O of its own. The readers take the bytes a command printed or a file held
//! and return what they mean. The stateful part reaches the outside only through three traits in
//! [`platform`] and [`hosts::host`]: a [`platform::rt::Runtime`] for tasks and timers, a
//! [`hosts::host::HostBackend`] that runs commands on a machine, and a [`platform::store::Store`]
//! for the records under ~/.inyon. The hub plugs in `inyon-backends`; the tests plug in a paused
//! clock and a fake machine; the phone app, which builds this crate for wasm32, plugs in its own.
//!
//! The stateful part runs on one thread, as the TS runs on Bun's one event loop: the registry, the
//! watchers and the plugged-in platform are thread-local, and nothing holds a borrow of them across
//! an await, so a synchronous stretch of the core is never interleaved with another task. The
//! readers keep nothing between calls but the provider names OpenCode reported, which any thread
//! may share.
//!
//! The `testing` feature opens a few hooks for `inyon-backends`' tests (putting a machine in the
//! registry as it is, ending a transcript tail as a dropped connection would). Nothing else uses it.

pub mod js;

pub mod agents;
pub mod claude;
pub mod clock;
pub mod config;
pub mod features;
pub mod hosts;
pub mod ids;
pub mod platform;
pub mod sha;
pub mod terminal;
pub mod tmux;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod build {
    /// regex without its `perf` features runs every pattern on its slowest engine
    #[test]
    fn native_builds_get_the_fast_regex_engines() {
        let manifest = include_str!("../Cargo.toml");
        let native = manifest.split("[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]").nth(1).expect("a native-only dependency table");
        let regex = native.lines().find(|l| l.starts_with("regex ")).expect("regex in it");
        assert!(regex.contains("\"perf\""), "{regex}");
    }
}
