//! Claude rewinds keep abandoned turns in the same JSONL. Only the current parent
//! chain is a conversation; reading every physical line would resurrect old turns.
use std::collections::{HashMap, HashSet};
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
    #[serde(default)]
    bridged: bool,
    #[serde(default)]
    prefixes: HashMap<String, Value>,
    #[serde(default)]
    repair_pending: bool,
    #[serde(default)]
    repair_enabled: bool,
}
impl Branch {
    pub fn needs_replay(&self) -> bool { !self.discarded.is_empty() || self.bridged || !self.prefixes.is_empty() }
    /// A native last-prompt leaf can recover a missing Windows user row. Place
    /// it immediately before its response, regardless of physical file order.
    pub fn prefix(&self, entry: &Value) -> Option<&Value> {
        self.prefixes.get(entry.get("uuid")?.as_str()?)
    }
    pub fn leaf(&self) -> Option<&str> { self.leaf.as_deref() }
    pub fn includes(&self, entry: &Value) -> bool {
        entry.get("uuid").and_then(Value::as_str).is_none_or(|id| !self.discarded.contains(id))
    }
    /// A newly appended human turn points to an earlier ancestor after a rewind.
    /// Ask for a fresh chain before showing the new branch or saving it to cache.
    pub fn observe(&mut self, entry: &Value, offset: u64) -> bool {
        if offset < self.size || entry.get("isSidechain").and_then(Value::as_bool) == Some(true) { return false; }
        if self.repair_pending && entry.get("type").and_then(Value::as_str) == Some("last-prompt") { return true; }
        let Some(id) = entry.get("uuid").and_then(Value::as_str) else { return false };
        if entry.get("parentUuid").is_none() { return false; }
        let parent = entry.get("parentUuid").and_then(Value::as_str);
        if self.repair_enabled && entry.get("type").and_then(Value::as_str) == Some("assistant")
            && parent.is_some_and(|p| !self.known.contains(p)) && !self.prefixes.contains_key(id) {
            self.repair_pending = true;
        }
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
    pub path: String,
    process: String,
    pub before: String,
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
pub fn command(path: &str, selected: Option<&Selection>, windows_psmux: bool) -> String {
    let mut extra = selected.map(|s| format!(" {} {}", shq(&s.before), s.since)).unwrap_or_default();
    if windows_psmux { extra.push_str(" --windows-psmux"); }
    format!("for p in python3 python; do if \"$p\" -c 'import sys; sys.exit(sys.version_info[0] != 3)' </dev/null >/dev/null 2>&1; then exec \"$p\" -c {} {}{extra}; fi; done", shq(include_str!("branch.py")), shq(path))
}

/// The confirmation screen names the exact prompt that will return as a draft.
/// Ambiguous repeated prompts wait for the authoritative on-disk parent chain.
pub fn confirmation(lines: &[String], items: &[inyon_protocol::ChatItem]) -> Option<(String, String)> {
    let clean: Vec<String> = lines.iter().map(|l| super::screen::unstyle(l).text).collect();
    let from = clean.iter().rposition(|l| l.contains("Confirm you want to restore"))?;
    // Claude versions differ: some say "restore to", others "restore the
    // conversation to". A narrow terminal can wrap this heading as well.
    let heading = clean[from..].iter().take_while(|l| !l.trim().starts_with('│')).map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    let heading = crate::js::text::squash(&heading);
    if !["Confirm you want to restore to the point before", "Confirm you want to restore the conversation to the point before"]
        .iter().any(|prefix| heading.starts_with(&crate::js::text::squash(prefix))) { return None; }
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
    fn missing_windows_prompt_reloads_when_its_native_leaf_receipt_arrives() {
        let mut branch: Branch = serde_json::from_value(json!({"known":["old"], "leaf":"old", "size":100, "repair_enabled":true})).unwrap();
        let response = json!({"type":"assistant", "uuid":"response", "parentUuid":"missing-input"});
        assert!(!branch.observe(&response, 100));
        assert!(!branch.observe(&json!({"type":"last-prompt"}), 90));
        assert!(branch.observe(&json!({"type":"last-prompt"}), 200));
        let repaired: Branch = serde_json::from_value(json!({"known":["response"], "leaf":"response", "size":300,
            "prefixes":{"response":{"type":"user","uuid":"missing-input","message":{"content":"Question"}}}})).unwrap();
        assert!(repaired.needs_replay());
        assert_eq!(repaired.prefix(&response).unwrap()["uuid"], "missing-input");
        assert!(repaired.prefix(&json!({"uuid":"unrelated"})).is_none());
        let mut other = Branch::default();
        assert!(!other.observe(&response, 100));
        assert!(!other.observe(&json!({"type":"last-prompt"}), 200));
    }
    #[test]
    fn repaired_windows_branches_replay_old_caches_without_invalidating_other_hosts() {
        let normal: Branch = serde_json::from_value(json!({"known":["a"],"leaf":"a","size":10})).unwrap();
        assert!(!normal.needs_replay());
        let repaired: Branch = serde_json::from_value(json!({"known":["a"],"leaf":"a","size":10,"bridged":true})).unwrap();
        assert!(repaired.needs_replay());
    }
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

    #[test]
    fn windows_claude_conversation_restore_heading_can_wrap() {
        let item = inyon_protocol::ChatItem::User {
            seq: 0, uuid: "last-question".into(), ts: String::new(), text: "那过拟合和欠拟合怎么区分？用两句话。".into(), images: 0, queued: None, shell: None,
        };
        for heading in [
            "Confirm you want to restore the conversation to the point before you sent this message:",
            "Confirm you want to restore the conversation to the\npoint before you sent this message:",
        ] {
            let lines = format!("Rewind\n{heading}\n\n│ 那过拟合和欠拟合怎么区分？用两句话。\n│ (12h ago)\n\nThis message and everything after it will be removed.\nThe code will be unchanged.\n❯ 1. Restore conversation\n2. Summarize from here");
            assert_eq!(confirmation(&lines.lines().map(str::to_owned).collect::<Vec<_>>(), std::slice::from_ref(&item)).map(|v| v.0), Some("last-question".into()));
        }
    }
}
