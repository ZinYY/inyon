//! `test/fixtures.ts`: small builders for transcript items, machines and panes. Each test only
//! spells out what it checks.

use std::sync::atomic::{AtomicI64, Ordering};

use inyon_protocol::{ChatInfo, HostState, PaneInfo};
use serde_json::{json, Value};

/// 2026-10-11T12:00:00Z, the fixtures' clock
pub const T0: f64 = 1_791_720_000_000.0;

/// `new Date(ms).toISOString()`
pub fn iso(ms: f64) -> String {
    let t = crate::js::local_time(ms, 0);
    let secs = (ms / 1000.0).floor() as i64 % 60;
    let millis = ms.rem_euclid(1000.0) as i64;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{millis:03}Z", t.year, t.month, t.day, t.hour, t.minute, secs)
}

pub mod item {
    use super::*;
    use inyon_protocol::ChatItem;

    static SEQ: AtomicI64 = AtomicI64::new(0);

    fn base(kind: &str, ts: Option<&str>) -> serde_json::Map<String, Value> {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let ts = ts.map(str::to_string).unwrap_or_else(|| iso(T0 + seq as f64 * 1000.0));
        let Value::Object(m) = json!({ "kind": kind, "seq": seq, "uuid": format!("u{seq}"), "ts": ts }) else { unreachable!() };
        m
    }

    fn make(kind: &str, ts: Option<&str>, more: Value) -> ChatItem {
        let mut m = base(kind, ts);
        if let Value::Object(more) = more {
            m.extend(more);
        }
        serde_json::from_value(Value::Object(m)).expect("a valid chat item")
    }

    pub fn user(text: &str) -> ChatItem {
        make("user", None, json!({ "text": text, "images": 0 }))
    }
    pub fn user_at(text: &str, ts: &str) -> ChatItem {
        make("user", Some(ts), json!({ "text": text, "images": 0 }))
    }
    pub fn shell_at(text: &str, ts: &str) -> ChatItem {
        make("user", Some(ts), json!({ "text": text, "images": 0, "shell": true }))
    }
    pub fn user_queued(text: &str) -> ChatItem {
        make("user", None, json!({ "text": text, "images": 0, "queued": true }))
    }
    pub fn text(text: &str) -> ChatItem {
        make("assistant", None, json!({ "msgId": "m", "block": { "type": "text", "text": text } }))
    }
    /// assistant text written at `ts`
    pub fn text_at(text: &str, ts: &str) -> ChatItem {
        make("assistant", Some(ts), json!({ "msgId": "m", "block": { "type": "text", "text": text } }))
    }
    pub fn thinking(text: &str) -> ChatItem {
        make("assistant", None, json!({ "msgId": "m", "block": { "type": "thinking", "text": text } }))
    }
    pub fn tool(id: &str) -> ChatItem {
        tool_with(id, "Bash", json!({ "command": "ls" }))
    }
    pub fn tool_named(id: &str, name: &str) -> ChatItem {
        tool_with(id, name, json!({ "command": "ls" }))
    }
    pub fn tool_with(id: &str, name: &str, input: Value) -> ChatItem {
        make("assistant", None, json!({ "msgId": "m", "block": { "type": "tool_use", "id": id, "name": name, "input": input } }))
    }
    pub fn result(tool_use_id: &str, text: &str) -> ChatItem {
        make("tool_result", None, json!({ "toolUseId": tool_use_id, "isError": false, "text": text }))
    }
    pub fn end(duration_ms: f64) -> ChatItem {
        make("turn_end", None, json!({ "durationMs": duration_ms }))
    }
    pub fn command(name: &str, args: &str, output: Option<&str>) -> ChatItem {
        make("command", None, json!({ "name": name, "args": args, "output": output }))
    }
    pub fn command_at(name: &str, args: &str, output: Option<&str>, ts: &str) -> ChatItem {
        make("command", Some(ts), json!({ "name": name, "args": args, "output": output }))
    }
    pub fn notice(subtype: &str) -> ChatItem {
        notice_text(subtype, subtype)
    }
    pub fn notice_text(subtype: &str, text: &str) -> ChatItem {
        make("notice", None, json!({ "subtype": subtype, "text": text, "level": "info" }))
    }
    pub fn goal(condition: &str, met: bool, reason: Option<&str>, cleared: Option<bool>) -> ChatItem {
        make("goal", None, json!({ "condition": condition, "met": met, "reason": reason, "cleared": cleared }))
    }

    /// The same item with another seq, uuid or message id (the hub's record of a live message).
    pub fn with(i: &ChatItem, more: Value) -> ChatItem {
        let Value::Object(mut m) = serde_json::to_value(i).unwrap() else { unreachable!() };
        if let Value::Object(more) = more {
            m.extend(more);
        }
        serde_json::from_value(Value::Object(m)).expect("a valid chat item")
    }
}

/// A plain terminal pane; `more` overrides any field.
pub fn pane(id: &str, more: Value) -> PaneInfo {
    let mut v = json!({ "id": id, "index": 0, "active": true, "command": "zsh", "cwd": "/home/me/code", "title": "", "width": 80, "height": 24, "dead": false });
    merge(&mut v, more);
    serde_json::from_value(v).expect("a valid pane")
}

pub fn agent_pane(id: &str, kind: &str, more: Value) -> PaneInfo {
    let mut v = json!({ "command": kind, "agent": { "kind": kind, "pid": 1, "state": "idle" } });
    merge(&mut v, more);
    pane(id, v)
}

/// A machine with one tmux session holding the given panes, one window each.
pub fn host(id: &str, panes: Vec<PaneInfo>, more: Value) -> HostState {
    let windows: Vec<Value> =
        panes.into_iter().enumerate().map(|(i, p)| json!({ "id": format!("@{i}"), "index": i, "name": "", "active": i == 0, "activity": 0, "panes": [p] })).collect();
    let mut v = json!({
        "config": { "id": id, "name": id, "kind": "ssh", "target": format!("me@{id}") },
        "status": "online",
        "integration": "installed",
        "tmux": "ok",
        "agents": ["claude"],
        "updatedAt": 0,
        "sessions": [{ "id": "$1", "name": "main", "attached": 0, "windows": windows }],
    });
    merge(&mut v, more);
    serde_json::from_value(v).expect("a valid host")
}

pub fn chat_info(more: Value) -> ChatInfo {
    let mut v = json!({ "hostId": "box", "paneId": "%1", "running": true, "queue": [], "tasks": [], "subagents": [], "artifacts": [], "stats": { "turns": 0, "compactions": 0 } });
    merge(&mut v, more);
    serde_json::from_value(v).expect("valid chat info")
}

/// `{ ...a, ...b }`, one level deep
fn merge(a: &mut Value, b: Value) {
    if let (Value::Object(a), Value::Object(b)) = (a, b) {
        a.extend(b);
    }
}
