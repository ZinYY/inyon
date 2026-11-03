//! Port of agents/gemini.ts: Gemini CLI's session file into chat items.
//!
//! Gemini CLI records each session as JSONL under `~/.gemini/tmp/<project>/chats/session-*.jsonl`:
//! a metadata line, message records, and `$patch` / `$set` / `$rewindTo` records that edit
//! earlier messages while a reply streams. (Format: packages/core/src/services/chatRecording*.)

use std::collections::HashMap;

use inyon_protocol::{ChatItem, NoticeLevel, Num};
use serde_json::{Map, Value};

use super::status::empty_status;
use crate::claude::transcript::{numbered, stdout_meta, string_of, text_block, thinking_block, tool_block, Base, ToolCall, TranscriptMeta};
use crate::clock::now_ms;
use crate::hosts::host::{shq, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{self, obj, s, value_string, Js};
use crate::js::{fmt_number, to_number, JsStr};
use crate::re;
use crate::sha::sha256_hex;

/// A Gemini process as the session picker sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeminiProc {
    pub cwd: Option<String>,
    /// unix seconds
    pub started_at: f64,
}

/// The command that lists the session files written since the process started, each with its
/// mtime and first line (`now_s`: the time now, in seconds).
pub fn guess_gemini_session_command(home: &str, proc: &GeminiProc, now_s: f64) -> String {
    let age_min = (((now_s - proc.started_at) / 60.0).ceil() + 2.0).max(5.0);
    format!(
        r#"find {} -path '*/chats/session-*.json*' -mmin -{} 2>/dev/null | while read -r f; do printf '%s\t%s\t%s\n' "$(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f")" "$f" "$(head -c 3000 "$f" | head -n 1)"; done"#,
        shq(&format!("{home}/.gemini/tmp")),
        fmt_number(age_min)
    )
}

/// The session the process has open, from what that command printed.
pub fn guess_gemini_session(stdout: &str, proc: &GeminiProc) -> Option<String> {
    let cwd = proc.cwd.as_deref().filter(|c| !c.is_empty());
    let hash = cwd.map(sha256_hex).unwrap_or_default();
    struct Row {
        mtime: f64,
        path: String,
        hash: Option<String>,
        dirs: Vec<Value>,
    }
    let mut rows: Vec<Row> = stdout
        .split('\n')
        .filter_map(|l| re!(r"^(\d+)\t([^\t]+)\t(.*)$").captures(l))
        .filter_map(|m| {
            let meta = json::parse(&m[3]).unwrap_or(Value::Null);
            let meta = Some(&meta);
            if meta.at("kind").str() == Some("subagent") {
                return None;
            }
            Some(Row {
                mtime: to_number(&m[1]),
                path: m[2].to_owned(),
                hash: meta.at("projectHash").str().map(str::to_owned),
                dirs: meta.at("directories").arr().cloned().unwrap_or_default(),
            })
        })
        .collect();
    rows.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
    rows.into_iter()
        .find(|x| {
            x.mtime >= proc.started_at - 2.0
                && cwd.is_none_or(|cwd| {
                    x.hash.as_deref() == Some(hash.as_str())
                        || x.dirs.iter().any(|d| d.as_str() == Some(cwd))
                        || x.path.contains(&format!("/{}/", cwd.rsplit('/').next().unwrap_or("")))
                })
        })
        .map(|x| x.path)
}

fn text_of(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().map(|x| text_of(Some(x))).collect::<Vec<_>>().concat(),
        Some(Value::Object(o)) => o.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
        _ => String::new(),
    }
}

/// `arr.join(sep)` for a JSON array
fn join(a: Option<&Value>, sep: &str) -> String {
    a.arr().map(|a| a.iter().map(|x| if x.is_null() { String::new() } else { value_string(x) }).collect::<Vec<_>>().join(sep)).unwrap_or_default()
}

/// Gemini tool names → the names Inyon's tool rows know.
pub fn map_tool(name: &str, args: Option<&Value>) -> ToolCall {
    let empty = Value::Object(Map::new());
    let a = args.present().or(Some(&empty));
    let get = |k: &str| a.at(k).cloned();
    let first = |ks: &[&str]| ks.iter().find_map(|k| a.at(k).present().cloned());
    let call = |name: &str, input: Value| ToolCall { name: name.into(), input };
    match name {
        "run_shell_command" => call("Bash", obj([("command", get("command")), ("description", get("description"))])),
        "read_file" | "read_many_files" => call("Read", obj([("file_path", Some(first(&["absolute_path", "file_path"]).unwrap_or_else(|| s(join(a.at("paths"), ", ")))))])),
        "write_file" => call("Write", obj([("file_path", first(&["file_path", "absolute_path"])), ("content", get("content"))])),
        "replace" | "edit" => call("Edit", obj([("file_path", first(&["file_path", "absolute_path"])), ("old_string", get("old_string")), ("new_string", get("new_string"))])),
        "glob" => call("Glob", obj([("pattern", get("pattern"))])),
        "search_file_content" | "grep" => call("Grep", obj([("pattern", get("pattern"))])),
        "list_directory" => call("LS", obj([("path", first(&["path", "dir_path"]))])),
        "web_fetch" => call("WebFetch", obj([("url", first(&["url", "prompt"]))])),
        "google_web_search" => call("WebSearch", obj([("query", get("query"))])),
        "write_todos" => {
            let todos = a.at("todos").arr().map(Vec::as_slice).unwrap_or_default();
            let todos = todos
                .iter()
                .map(|t| {
                    let t = Some(t);
                    let status = match t.at("status").str() {
                        Some("in_progress") => "in_progress",
                        Some("completed") => "completed",
                        _ => "pending",
                    };
                    obj([("content", t.at("description").or(t.at("content")).cloned()), ("activeForm", t.at("description").cloned()), ("status", Some(s(status)))])
                })
                .collect();
            call("TodoWrite", obj([("todos", Some(Value::Array(todos)))]))
        }
        _ => call(name, a.cloned().unwrap_or(empty.clone())),
    }
}

/// What the session file says about itself, apart from its messages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeminiMeta {
    pub summary: Option<Value>,
    pub model: Option<Value>,
    pub tokens: Option<Value>,
}

/// Holds a Gemini session in memory, applying records in order.
#[derive(Debug, Clone, Default)]
pub struct GeminiDoc {
    order: Vec<String>,
    messages: HashMap<String, Value>,
    pub meta: GeminiMeta,
}

fn key(v: Option<&Value>) -> String {
    v.to_js_string()
}

impl GeminiDoc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, rec: &Value) {
        if !rec.is_object() {
            return;
        }
        let r = Some(rec);
        if r.at("$rewindTo").truthy() {
            let to = key(r.at("$rewindTo"));
            if let Some(i) = self.order.iter().position(|id| *id == to) {
                for id in self.order.split_off(i) {
                    self.messages.remove(&id);
                }
            }
            return;
        }
        if r.at("$set").truthy() {
            let set = r.at("$set");
            if set.at("summary").truthy() {
                self.meta.summary = set.at("summary").cloned();
            }
            if let Some(messages) = set.at("messages").arr() {
                self.order.clear();
                self.messages.clear();
                for m in messages {
                    self.put(m);
                }
            }
            return;
        }
        if r.at("$patch").truthy() {
            let p = r.at("$patch");
            let mut updates: Vec<&Value> = p.at("updates").arr().map(|a| a.iter().collect()).unwrap_or_default();
            if p.at("id").truthy() {
                updates.push(p.expect("truthy"));
            }
            for u in updates {
                let u = Some(u);
                let Some(m) = self.messages.get_mut(&key(u.at("id"))) else { continue };
                if let (Some(c), Some(o)) = (u.at("content"), m.as_object_mut()) {
                    o.insert("content".into(), c.clone());
                }
                for tc in u.at("toolCalls").arr().map(Vec::as_slice).unwrap_or_default() {
                    // an update without an id belongs to no call (two missing ids would look equal)
                    let (Some(id), Some(result)) = (tc.get("id").filter(|v| !v.is_null()), tc.get("result")) else { continue };
                    let calls = m.get_mut("toolCalls").and_then(Value::as_array_mut);
                    if let Some(t) = calls.and_then(|c| c.iter_mut().find(|x| x.get("id").is_some_and(|x| json::strict_equals(x, id)))) {
                        if let Some(o) = t.as_object_mut() {
                            o.insert("result".into(), result.clone());
                        }
                    }
                }
            }
            for id in p.at("removeIds").arr().map(Vec::as_slice).unwrap_or_default() {
                let id = key(Some(id));
                self.messages.remove(&id);
                self.order.retain(|x| *x != id);
            }
            if let Some(ids) = p.at("orderIds").arr() {
                self.order = ids.iter().map(|i| key(Some(i))).filter(|i| self.messages.contains_key(i)).collect();
            }
            return;
        }
        if r.at("sessionId").truthy() && !r.at("id").truthy() {
            if r.at("summary").truthy() {
                self.meta.summary = r.at("summary").cloned();
            }
            return;
        }
        if r.at("id").truthy() && r.at("type").truthy() {
            self.put(rec);
        }
    }

    fn put(&mut self, m: &Value) {
        let mm = Some(m);
        let id = key(mm.at("id"));
        if !self.messages.contains_key(&id) {
            self.order.push(id.clone());
        }
        self.messages.insert(id, m.clone());
        if mm.at("type").str() == Some("gemini") {
            if mm.at("model").truthy() {
                self.meta.model = mm.at("model").cloned();
            }
            if mm.at("tokens").truthy() {
                self.meta.tokens = mm.at("tokens").cloned();
            }
        }
    }

    /// The whole conversation as chat items.
    pub fn items(&self, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
        let mut out: Vec<ChatItem> = vec![];
        meta.stats.turns = 0;
        for id in &self.order {
            let Some(m) = self.messages.get(id) else { continue };
            let m = Some(m);
            let mid = key(m.at("id"));
            let base = Base::new(&mid, string_of(m.at("timestamp")).unwrap_or_default());
            let text = text_of(m.at("displayContent").or(m.at("content")));
            let text = text.jtrim();
            match m.at("type").str() {
                Some("user") => {
                    if !text.is_empty() {
                        out.push(base.user(text, 0));
                    }
                }
                Some("gemini") => {
                    for (n, t) in m.at("thoughts").arr().map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
                        let t = Some(t);
                        let body =
                            [t.at("subject"), t.at("description")].into_iter().filter(|v| v.truthy()).map(|v| value_string(v.expect("truthy"))).collect::<Vec<_>>().join(": ");
                        if !body.is_empty() {
                            out.push(base.with_uuid(format!("{mid}:t{n}")).assistant(&mid, None, thinking_block(body)));
                        }
                    }
                    if !text.is_empty() {
                        out.push(base.with_uuid(format!("{mid}:x")).assistant(&mid, string_of(m.at("model")), text_block(text)));
                    }
                    for tc in m.at("toolCalls").arr().map(Vec::as_slice).unwrap_or_default() {
                        let tc = Some(tc);
                        let tid = key(tc.at("id"));
                        let mapped = map_tool(&tc.at("name").to_js_string(), tc.at("args"));
                        out.push(base.with_uuid(format!("{mid}:{tid}")).assistant(&mid, None, tool_block(&tid, &mapped.name, mapped.input.clone())));
                        let status = tc.at("status").str();
                        if matches!(status, Some("success" | "error" | "cancelled")) || tc.at("result").truthy() {
                            let display = tc.at("resultDisplay").str().unwrap_or("");
                            let text = if display.is_empty() {
                                let output = tc.at("result").idx(0).at("functionResponse").at("response").at("output");
                                text_of(output.present().or(tc.at("result")))
                            } else {
                                display.to_owned()
                            };
                            let meta = (mapped.name == "Bash").then(|| stdout_meta(display.slice16(0, Some(8000))));
                            out.push(base.with_uuid(format!("{mid}:{tid}:r")).tool_result(&tid, status == Some("error"), text.slice16(0, Some(8000)), None, meta));
                        }
                    }
                    meta.stats.turns += 1;
                }
                Some(kind @ ("info" | "warning" | "error")) if !text.is_empty() => {
                    let level = match kind {
                        "info" => NoticeLevel::Info,
                        "warning" => NoticeLevel::Warn,
                        _ => NoticeLevel::Error,
                    };
                    out.push(base.notice(level, "info", text));
                }
                _ => {}
            }
        }
        if let Some(summary) = self.meta.summary.as_ref() {
            meta.title = Some(value_string(summary));
        }
        if let Some(model) = self.meta.model.as_ref() {
            meta.model = Some(value_string(model));
        }
        if let Some(t) = self.meta.tokens.as_ref() {
            let t = Some(t);
            let tokens = t.at("input").num().unwrap_or(0.0) + t.at("cached").num().unwrap_or(0.0);
            meta.stats.context_tokens = Some(Num(tokens));
            meta.status = Some(inyon_protocol::AgentStatus { model: self.meta.model.as_ref().map(value_string), context_used: Some(Num(tokens)), ..empty_status(now_ms() as f64) });
        }
        numbered(out)
    }
}

/// `guessGeminiSession(host, home, proc)`: runs the command above on the machine.
pub async fn find_gemini_session(host: &Host, home: &str, proc: &GeminiProc) -> HubResult<Option<String>> {
    let r = host.run(&guess_gemini_session_command(home, proc, now_ms() as f64 / 1000.0)).await?;
    Ok(guess_gemini_session(&r.stdout, proc))
}

#[cfg(test)]
#[path = "gemini_tests.rs"]
mod tests;
