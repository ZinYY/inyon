//! Port of claude/chat-cache.ts.
//!
//! The latest part of each conversation, kept on the hub's disk (in the phone app, its storage).
//! Opening the chat again shows it at once and reads on from where it stopped, so a long session
//! isn't read from the start after hours away or after the app was closed. A session on a machine
//! that dropped off (NetBird or Tailscale down, laptop asleep) can still be read meanwhile.

use inyon_protocol::{AgentKind, ChatInfo, ChatItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock::now_ms;
use crate::platform::console;
use crate::platform::store::{key_part, kv};

pub const KEEP: usize = 300;
/// Bumped when reading a transcript changes how the summary is built (tasks, queue, subagents), so
/// a chat saved by an older reader is read again from the start instead of carrying its mistakes on.
pub const READER: u32 = 3;

fn store_key(key: &str) -> String {
    format!("chats/{}", key_part(key))
}

/// Where reading the transcript stopped, with the summary built up to there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatAt {
    pub offset: u64,
    pub seq: i64,
    pub codex: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    /// what `save_meta` made of the summary
    pub meta: Value,
    /// the file offset of the line the first saved message came from (0: nothing before it)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedChat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// the agent process the conversation belongs to (see proc_of in watcher.rs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proc: Option<String>,
    pub info: ChatInfo,
    pub items: Vec<ChatItem>,
    pub saved_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<ChatAt>,
}

pub fn load_chat(key: &str) -> Option<CachedChat> {
    kv::get(&store_key(key))
}

/// Saves the chat, its last [`KEEP`] items, stamped now.
pub fn save_chat(key: &str, mut chat: CachedChat) {
    if chat.items.len() > KEEP {
        chat.items.drain(..chat.items.len() - KEEP);
    }
    chat.saved_at = now_ms();
    if let Err(e) = kv::set(&store_key(key), &chat) {
        console::error(&format!("chat cache: {e}"));
    }
}

/// Drops cached conversations of a removed machine, and any older than 30 days. A record is read
/// only for its `savedAt`, so one this build can't fully read is kept until it's old.
pub fn prune_chats(host_id: Option<&str>) {
    let old = now_ms() as f64 - 30.0 * 86_400_000.0;
    let mine = host_id.map(|h| format!("chats/{}_", key_part(h)));
    for k in kv::keys("chats/") {
        let saved = kv::get_value(&k).and_then(|v| v.get("savedAt").and_then(Value::as_f64)).unwrap_or(0.0);
        if mine.as_deref().is_some_and(|m| k.starts_with(m)) || saved < old {
            kv::del(&k);
        }
    }
}
