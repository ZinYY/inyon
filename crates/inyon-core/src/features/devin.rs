//! Read saved Devin sessions independently of panes; hand off writes to a named tmux CLI.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Value};

use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host, StreamHandle};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;
use crate::platform::rt::{sleep, spawn};

thread_local! {
    static READERS: RefCell<HashMap<(String, String), Rc<RefCell<Reader>>>> = RefCell::new(HashMap::new());
}

#[derive(Default)]
struct Reader {
    version: u64,
    session: Value,
    state: Value,
    order: Vec<String>,
    messages: HashMap<String, (u64, Value)>,
    stream: Option<StreamHandle>,
    touched: u64,
    retry_at: u64,
    error: Option<String>,
}

impl Reader {
    fn apply(&mut self, value: Value) {
        self.version += 1;
        if let Some(error) = value["error"].as_str() {
            self.error = Some(error.into());
            return;
        }
        self.error = None;
        if let Some(session) = value.get("session") {
            self.session = session.clone();
            self.order = value["order"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
            let keep: std::collections::HashSet<&str> = self.order.iter().map(String::as_str).collect();
            self.messages.retain(|key, _| keep.contains(key.as_str()));
            for message in value["messages"].as_array().into_iter().flatten() {
                if let Some(id) = message["id"].as_str() {
                    self.messages.insert(id.into(), (self.version, message.clone()));
                }
            }
        }
        if let Some(state) = value.get("state") {
            self.state = state.clone();
        }
    }

    fn answer(&self, since: u64) -> Value {
        let reset = since == 0 || since > self.version;
        json!({
            "version": self.version, "reset": reset, "loading": self.session.is_null(),
            "session": self.session, "state": self.state, "error": self.error,
            "order": self.order,
            "messages": self.order.iter().filter_map(|id| self.messages.get(id)).filter(|(v, _)| reset || *v > since).map(|(_, m)| m).collect::<Vec<_>>()
        })
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) && id.as_bytes()[0].is_ascii_alphanumeric()
}

fn machine(id: &str) -> HubResult<Host> {
    let rt = registry().get(id).ok_or_else(|| http(404, "Unknown machine"))?;
    let rt = rt.borrow();
    if rt.state.status != inyon_protocol::HostStatus::Online {
        return Err(http(503, "Machine is not connected"));
    }
    Ok(rt.host.clone())
}

fn command(action: &str, sid: Option<&str>) -> String {
    format!("python3 -u -c {} {} {}", shq(include_str!("devin_store.py")), shq(action), sid.map(shq).unwrap_or_default())
}

pub async fn list(id: &str) -> HubResult<Value> {
    let r = machine(id)?.exec(&command("list", None), ExecOptions::timeout(15_000)).await?;
    let v: Value = serde_json::from_str(r.stdout.trim()).map_err(|_| http(502, "Could not read Devin sessions. Python 3 is required on the machine."))?;
    if let Some(error) = v["error"].as_str() { return Err(http(409, error)); }
    Ok(v)
}

pub async fn read(id: &str, sid: &str, since: u64) -> HubResult<Value> {
    if !valid_id(sid) { return Err(http(400, "Invalid Devin session ID")); }
    let host = machine(id)?;
    let key = (id.to_owned(), sid.to_owned());
    let reader = READERS.with(|all| -> HubResult<_> {
        let mut all = all.borrow_mut();
        if !all.contains_key(&key) && all.len() >= 8 {
            return Err(http(429, "Too many open Devin conversations. Close an unused tab and try again shortly."));
        }
        Ok(all.entry(key.clone()).or_insert_with(|| Rc::new(RefCell::new(Reader { version: now_ms(), ..Reader::default() }))).clone())
    })?;
    reader.borrow_mut().touched = now_ms();
    let start = { let r = reader.borrow(); r.stream.is_none() && now_ms() >= r.retry_at };
    if start {
        let weak = Rc::downgrade(&reader);
        let stream = host.stream(&command("watch", Some(sid)), move |line| {
            if let (Some(reader), Ok(value)) = (weak.upgrade(), serde_json::from_str::<Value>(&line)) {
                reader.borrow_mut().apply(value);
            }
        }, false)?;
        reader.borrow_mut().stream = Some(stream.clone());
        let weak = Rc::downgrade(&reader);
        let ended = stream.clone();
        spawn(async move {
            let _ = ended.exited.clone().await;
            if let Some(reader) = weak.upgrade() {
                let mut r = reader.borrow_mut();
                if r.stream.as_ref().is_some_and(|s| s.same(&ended)) {
                    r.stream = None;
                    r.retry_at = now_ms() + 2_000;
                    r.error.get_or_insert_with(|| "Devin connection closed. Reconnecting…".into());
                }
            }
        });
        let weak = Rc::downgrade(&reader);
        spawn(async move {
            loop {
                sleep(10_000).await;
                let Some(reader) = weak.upgrade() else { break };
                let inactive = now_ms().saturating_sub(reader.borrow().touched) > 45_000;
                if inactive {
                    stream.kill();
                    READERS.with(|all| { all.borrow_mut().remove(&key); });
                    break;
                }
                if !reader.borrow().stream.as_ref().is_some_and(|s| s.same(&stream)) { break; }
            }
        });
    }
    // The first request waits briefly for history; subsequent polls only read this cache.
    for _ in 0..40 {
        if !reader.borrow().session.is_null() || reader.borrow().error.is_some() { break; }
        sleep(100).await;
    }
    let result = reader.borrow().answer(since);
    Ok(result)
}

pub async fn send(id: &str, sid: &str, body: &Value) -> HubResult<Value> {
    if !valid_id(sid) { return Err(http(400, "Invalid Devin session ID")); }
    let text = body["text"].as_str().filter(|s| !s.trim().is_empty() && s.len() <= 512 * 1024).ok_or_else(|| http(400, "Send a message up to 512 KB"))?;
    let request = body["requestId"].as_str().filter(|s| valid_id(s)).ok_or_else(|| http(400, "A request ID is required"))?;
    let data = json!({ "id": sid, "text": text, "requestId": request });
    let r = machine(id)?.exec(&command("send", None), ExecOptions::input(data.to_string()).with_timeout(65_000)).await?;
    let v: Value = serde_json::from_str(r.stdout.trim()).map_err(|_| http(502, "Devin delivery was not confirmed. Inspect the tmux terminal before trying again."))?;
    if let Some(error) = v["error"].as_str() { return Err(http(409, error)); }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_history_retains_unchanged_messages_and_removes_old_branches() {
        let mut r = Reader::default();
        r.apply(json!({"session":{"id":"example"},"order":["u","a"],"messages":[{"id":"u","text":"hello"},{"id":"a","text":"one"}]}));
        let first = r.version;
        r.apply(json!({"session":{"id":"example"},"order":["u","b"],"messages":[{"id":"b","text":"two"}]}));
        let delta = r.answer(first);
        assert_eq!(delta["messages"], json!([{"id":"b","text":"two"}]));
        assert_eq!(delta["order"], json!(["u","b"]));
        assert_eq!(r.answer(0)["messages"].as_array().unwrap().len(), 2);
        assert!(!r.messages.contains_key("a"));
    }

    #[test]
    fn session_identifiers_cannot_inject_commands_or_paths() {
        for id in ["", "../a", "-r", "a;exit", "a\nb", "a b"] { assert!(!valid_id(id)); }
        assert!(valid_id("example-session_2"));
    }
}
