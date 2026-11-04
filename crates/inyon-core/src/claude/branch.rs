//! Claude rewinds keep abandoned turns in the same JSONL. Only the current parent
//! chain is a conversation; reading every physical line would resurrect old turns.
use std::collections::HashSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::hosts::host::shq;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Branch {
    #[serde(default)]
    known: HashSet<String>,
    #[serde(default)]
    discarded: HashSet<String>,
    leaf: Option<String>,
    #[serde(default)]
    size: u64,
}
impl Branch {
    pub fn has_rewind(&self) -> bool { !self.discarded.is_empty() }
    pub fn includes(&self, entry: &Value) -> bool {
        entry.get("uuid").and_then(Value::as_str).is_none_or(|id| !self.discarded.contains(id))
    }
    /// A newly appended human turn points to an earlier ancestor after a rewind.
    /// Ask for a fresh chain before showing the new branch or saving it to cache.
    pub fn observe(&mut self, entry: &Value, offset: u64) -> bool {
        if offset < self.size || entry.get("isSidechain").and_then(Value::as_bool) == Some(true) { return false; }
        let Some(id) = entry.get("uuid").and_then(Value::as_str) else { return false };
        if entry.get("parentUuid").is_none() { return false; }
        let parent = entry.get("parentUuid").and_then(Value::as_str);
        let human = entry.get("type").and_then(Value::as_str) == Some("user")
            && entry.get("isMeta").and_then(Value::as_bool) != Some(true)
            && entry.pointer("/message/content").is_some_and(|v| v.is_string() || v.as_array().is_some_and(|a| a.iter().all(|b| b.get("type").and_then(Value::as_str) != Some("tool_result"))));
        if human && self.leaf.is_some() && parent != self.leaf.as_deref()
            && parent.is_none_or(|p| self.known.contains(p)) { return true; }
        self.known.insert(id.to_owned());
        self.leaf = Some(id.to_owned());
        false
    }
}

#[derive(Serialize, Deserialize)]
pub struct Selection {
    path: String,
    process: String,
    before: String,
    since: u64,
    at: u64,
}
fn selection_key(key: &str) -> String { format!("claude-rewinds/{}", crate::platform::store::key_part(key)) }
fn same_process(saved: &str, current: Option<&str>) -> bool {
    let Some(current) = current else { return false };
    if saved == current { return true; }
    let (Some((pid, start)), Some((other_pid, other_start))) = (saved.split_once(':'), current.split_once(':')) else { return false };
    // ps reports elapsed whole seconds. Subtracting that from the probe's clock can
    // move a living process's inferred start time by a second between polls.
    pid == other_pid && start.parse::<u64>().ok().zip(other_start.parse::<u64>().ok()).is_some_and(|(a, b)| a.abs_diff(b) <= 5000)
}
pub fn save_selection(key: &str, path: String, process: String, before: String, since: u64) {
    let _ = crate::platform::store::kv::set(&selection_key(key), &Selection { path, process, before, since, at: crate::clock::now_ms() });
}
pub fn selection(key: &str, path: &str, process: Option<&str>, session_start: f64) -> Option<Selection> {
    let saved: Selection = crate::platform::store::kv::get(&selection_key(key))?;
    if saved.path != path || !same_process(&saved.process, process) || session_start * 1000.0 > saved.at as f64 {
        crate::platform::store::kv::del(&selection_key(key));
        None
    } else { Some(saved) }
}
pub fn command(path: &str, selected: Option<&Selection>) -> String {
    let extra = selected.map(|s| format!(" {} {}", shq(&s.before), s.since)).unwrap_or_default();
    format!("for p in python3 python; do if \"$p\" -c 'import sys; sys.exit(sys.version_info[0] != 3)' </dev/null >/dev/null 2>&1; then exec \"$p\" -c {} {}{extra}; fi; done", shq(include_str!("branch.py")), shq(path))
}

/// The confirmation screen names the exact prompt that will return as a draft.
/// Ambiguous repeated prompts wait for the authoritative on-disk parent chain.
pub fn confirmation(lines: &[String], items: &[inyon_protocol::ChatItem]) -> Option<(String, String)> {
    let clean: Vec<String> = lines.iter().map(|l| super::screen::unstyle(l).text).collect();
    let from = clean.iter().rposition(|l| l.contains("Confirm you want to restore to the point before"))?;
    if !clean[from..].iter().any(|l| l.contains("Restore conversation")) { return None; }
    let target = clean[from + 1..].iter().take_while(|l| !l.contains("This message and everything after"))
        .filter_map(|l| l.trim().strip_prefix('│').map(str::trim)).filter(|l| !l.starts_with('(')).collect::<Vec<_>>().join(" ");
    if target.is_empty() { return None; }
    let matches: Vec<_> = items.iter().filter_map(|it| match it {
        inyon_protocol::ChatItem::User { uuid, text, .. } if crate::js::text::squash(text) == crate::js::text::squash(&target) => Some((uuid.clone(), text.clone())),
        _ => None,
    }).collect();
    (matches.len() == 1).then(|| matches[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn process_clock_rounding_does_not_undo_a_rewind_on_reconnect() {
        assert!(same_process("42:10000", Some("42:11000")));
        assert!(same_process("42:10000", Some("42:9000")));
        assert!(!same_process("42:10000", Some("43:10000")));
        assert!(!same_process("42:10000", Some("42:20000")));
        assert!(!same_process("42:10000", None));
    }
    #[test]
    fn a_rewind_is_detected_without_confusing_regular_turns_or_tool_results() {
        let mut b: Branch = serde_json::from_value(json!({"known":["a","b"],"discarded":["old"],"leaf":"b","size":100})).unwrap();
        assert!(!b.includes(&json!({"uuid":"old"})));
        assert!(b.includes(&json!({"type":"custom-title"})));
        let user = |id: &str, parent: &str| json!({"type":"user","uuid":id,"parentUuid":parent,"message":{"content":"A question"}});
        assert!(!b.observe(&user("c", "b"), 100));
        assert!(b.observe(&user("d", "a"), 200));
        assert!(!b.observe(&user("d", "a"), 50));
        assert!(!b.observe(&json!({"type":"user","uuid":"tool","parentUuid":"a","message":{"content":[{"type":"tool_result"}]}}), 200));
    }

    #[test]
    fn only_an_unambiguous_conversation_restore_selects_a_prompt() {
        let item = |id: &str| inyon_protocol::ChatItem::User {
            seq: 0, uuid: id.into(), ts: String::new(), text: "Explain underfitting with coffee.".into(), images: 0, queued: None, shell: None,
        };
        let lines = ["Confirm you want to restore to the point before this message:", "│ Explain underfitting with", "│ coffee.", "│ (2 minutes ago)", "This message and everything after will be removed.", "❯ Restore conversation"].map(str::to_owned);
        assert_eq!(confirmation(&lines, &[item("a")]).map(|v| v.0), Some("a".into()));
        assert!(confirmation(&lines, &[item("a"), item("b")]).is_none());
        assert!(confirmation(&lines[..5], &[item("a")]).is_none());
    }
}
