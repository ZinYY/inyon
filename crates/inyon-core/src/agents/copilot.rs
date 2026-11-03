//! Port of agents/copilot.ts: which session file a Copilot CLI process owns, and its events as
//! chat items. Event schema: github/copilot-sdk nodejs/src/generated/session-events.ts.

use std::collections::HashSet;

use inyon_protocol::{AgentStatus, ChatItem, NoticeLevel, Num};
use serde_json::{Map, Value};

use super::status::empty_status;
use crate::claude::transcript::{stdout_meta, string_of, text_block, thinking_block, tool_block, Base, ToolCall, TranscriptMeta};
use crate::clock::now_ms;
use crate::hosts::host::{shq, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{obj, Js};
use crate::js::{date, to_number, JsStr};
use crate::re;

/// A Copilot process as the session picker sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopilotProc {
    pub pid: i64,
    pub resume: Option<String>,
}

/// One line of `ps -eo pid=,ppid=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsRow {
    pub pid: i64,
    pub ppid: i64,
}

const MAX_SAFE: i64 = 9_007_199_254_740_991;

/// Prefer locks held by the detected process, then its nearest descendants (npm launcher).
pub fn pick_copilot_session(locks: &[String], processes: &[PsRow], proc: &CopilotProc) -> Option<String> {
    if proc.pid <= 0 || proc.pid > MAX_SAFE {
        return None;
    }
    let mut level: HashSet<i64> = HashSet::from([proc.pid]);
    let mut visited: HashSet<i64> = HashSet::new();
    let mut depth = 0;
    while depth <= 4 && !level.is_empty() {
        let mut sessions: Vec<String> = vec![];
        for file in locks {
            let Some(m) = re!(r"\/inuse\.(\d+)\.lock$").captures(file) else { continue };
            if !level.contains(&(to_number(&m[1]) as i64)) {
                continue;
            }
            let session = re!(r"\/inuse\.\d+\.lock$").replace(file, "/events.jsonl").into_owned();
            if !sessions.contains(&session) {
                sessions.push(session);
            }
        }
        if !sessions.is_empty() {
            if let Some(resume) = proc.resume.as_deref().filter(|r| !r.is_empty()) {
                if let Some(exact) = sessions.iter().find(|f| f.split('/').rev().nth(1) == Some(resume)) {
                    return Some(exact.clone());
                }
            }
            return (sessions.len() == 1).then(|| sessions.remove(0));
        }
        visited.extend(level.iter().copied());
        level = processes.iter().filter(|p| level.contains(&p.ppid) && !visited.contains(&p.pid)).map(|p| p.pid).collect();
        depth += 1;
    }
    None
}

/// The command listing Copilot's inuse locks; each names the process holding the session.
pub fn copilot_locks_command(home: &str) -> String {
    format!("find {} -mindepth 2 -maxdepth 2 -name 'inuse.*.lock' -print 2>/dev/null", shq(&format!("{home}/.copilot/session-state")))
}

/// The process tree, read with [`parse_ps_tree`].
pub const PS_TREE_COMMAND: &str = "ps -eo pid=,ppid=";

pub fn parse_ps_tree(stdout: &str) -> Vec<PsRow> {
    stdout.split('\n').filter_map(|line| re!(r"^\s*(\d+)\s+(\d+)\s*$").captures(line)).map(|m| PsRow { pid: to_number(&m[1]) as i64, ppid: to_number(&m[2]) as i64 }).collect()
}

/// Copilot's inuse lock identifies ownership without guessing from cwd or newest transcript:
/// the session from what the two commands printed.
pub fn guess_copilot_session(locks_stdout: &str, ps_stdout: &str, proc: &CopilotProc) -> Option<String> {
    if proc.pid <= 0 || proc.pid > MAX_SAFE {
        return None;
    }
    let locks: Vec<String> = locks_stdout.split('\n').filter(|l| !l.is_empty()).map(str::to_owned).collect();
    pick_copilot_session(&locks, &parse_ps_tree(ps_stdout), proc)
}

fn tool(name: &str, a: Option<&Value>) -> ToolCall {
    let get = |k: &str| a.at(k).cloned();
    let call = |name: &str, input: Value| ToolCall { name: name.into(), input };
    match name {
        "bash" | "powershell" | "local_shell" => call("Bash", obj([("command", get("command")), ("description", get("description"))])),
        "view" => call("Read", obj([("file_path", get("path"))])),
        "create" => call("Write", obj([("file_path", get("path")), ("content", get("file_text"))])),
        "edit" => call("Edit", obj([("file_path", get("path")), ("old_string", get("old_str")), ("new_string", get("new_str"))])),
        "grep" => call("Grep", obj([("pattern", get("pattern")), ("path", get("path"))])),
        "glob" => call("Glob", obj([("pattern", get("pattern")), ("path", get("path"))])),
        "web_fetch" => call("WebFetch", obj([("url", get("url"))])),
        _ => call(name, a.cloned().unwrap_or_else(|| Value::Object(Map::new()))),
    }
}

fn clip(text: Option<&Value>, max: usize) -> String {
    text.str().map_or_else(String::new, |t| t.slice16(0, Some(max as isize)).to_owned())
}

/// One Copilot event → chat items.
pub fn normalize_copilot(e: &Value, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
    let e = Some(e);
    if !e.is_object() || !e.at("data").truthy() || e.at("agentId").truthy() {
        return vec![];
    }
    let d = e.at("data");
    let base = Base::new(e.at("id").string_or_empty(), e.at("timestamp").string_or_empty());
    let kind = e.at("type").str().unwrap_or("");
    match kind {
        "session.start" => {
            meta.version = string_of(d.at("copilotVersion"));
            meta.cwd = string_of(d.at("context").at("cwd"));
            meta.model = string_of(d.at("selectedModel"));
            meta.effort = string_of(d.at("reasoningEffort"));
            meta.stats.started_at = string_of(d.at("startTime").or(e.at("timestamp")));
            vec![]
        }
        "session.title_changed" => {
            meta.title = string_of(d.at("title"));
            vec![]
        }
        "session.model_change" => {
            meta.model = string_of(d.at("newModel"));
            meta.effort = string_of(d.at("reasoningEffort"));
            vec![]
        }
        "session.permissions_changed" => {
            meta.mode = match string_of(d.at("mode")) {
                Some(m) => Some(m),
                None if d.at("allowAllPermissionMode").str() == Some("auto") => Some("assisted".into()),
                None => match d.at("allowAllPermissions") {
                    Some(Value::Bool(true)) => Some("allow-all".into()),
                    Some(Value::Bool(false)) => Some("default".into()),
                    _ => meta.mode.take(),
                },
            };
            vec![]
        }
        "user.message" => {
            if !d.at("content").truthy() {
                return vec![];
            }
            let images = d.at("attachments").arr().map_or(0, |a| a.iter().filter(|x| re!(r"image\/").is_match(&Some(*x).at("mimeType").string_or_empty())).count());
            vec![base.user(clip(d.at("content"), 24_000), images as u32)]
        }
        "assistant.message" => {
            // Tool requests are also followed by execution_start records. Render once, at execution.
            let text = clip(d.at("content"), 24_000);
            if text.is_empty() {
                return vec![];
            }
            let msg_id = string_of(d.at("messageId")).unwrap_or_else(|| base.uuid.clone());
            vec![base.assistant(msg_id, string_of(d.at("model")), text_block(text))]
        }
        "assistant.reasoning" => {
            let text = clip(d.at("content"), 24_000);
            if text.is_empty() {
                return vec![];
            }
            let msg_id = string_of(d.at("reasoningId")).unwrap_or_else(|| base.uuid.clone());
            vec![base.assistant(msg_id, None, thinking_block(text))]
        }
        "tool.user_requested" | "tool.execution_start" => {
            if !d.at("toolCallId").truthy() || !d.at("toolName").truthy() {
                return vec![];
            }
            let id = d.at("toolCallId").to_js_string();
            let name = d.at("toolName").to_js_string();
            let mapped = tool(&name, d.at("arguments").present());
            meta.tools.insert(id.clone(), mapped.clone());
            let mut out = vec![];
            if kind == "tool.user_requested" && name == "local_shell" {
                out.push(base.with_uuid(format!("{}:input", base.uuid)).user(format!("!{}", d.at("arguments").at("command").string_or_empty()), 0));
            }
            out.push(base.assistant(&base.uuid, None, tool_block(id, mapped.name, mapped.input)));
            out
        }
        "tool.execution_complete" => {
            let r = d.at("result");
            let text = clip(r.at("detailedContent").or(r.at("content")).or(d.at("error").at("message")), 8000);
            let id = string_of(d.at("toolCallId")).unwrap_or_default();
            let bash = meta.tools.get(&id).is_some_and(|t| t.name == "Bash");
            let is_error = d.at("success") == Some(&Value::Bool(false));
            vec![base.tool_result(id, is_error, text.clone(), None, bash.then(|| stdout_meta(text)))]
        }
        "assistant.turn_end" => {
            meta.stats.turns += 1;
            vec![base.turn_end(0.0)]
        }
        "session.compaction_complete" => {
            if d.at("success") != Some(&Value::Bool(false)) {
                meta.stats.compactions += 1;
                meta.stats.context_tokens = None;
            }
            vec![]
        }
        "session.usage_info" => {
            let current = d.at("currentTokens").num();
            if let Some(c) = current {
                meta.stats.context_tokens = Some(Num(c));
            }
            let limit = d.at("tokenLimit").num();
            let parsed = date::parse(&e.at("timestamp").to_js_string());
            let updated_at = if parsed.is_nan() || parsed == 0.0 { now_ms() as f64 } else { parsed };
            let prev = meta.status.take().unwrap_or_else(|| empty_status(updated_at));
            meta.status = Some(AgentStatus {
                model: meta.model.clone(),
                model_id: meta.model.clone(),
                effort: meta.effort.clone(),
                context_used: current.map(Num),
                context_size: limit.map(Num),
                context_pct: limit.filter(|l| *l > 0.0).map(|l| Num(current.unwrap_or(f64::NAN) / l * 100.0)),
                updated_at: updated_at as u64,
                ..prev
            });
            vec![]
        }
        // assistant.usage.cost is a model multiplier / AI-credit measure, NOT dollars.
        "session.error" | "session.warning" => {
            let level = if kind == "session.error" { NoticeLevel::Error } else { NoticeLevel::Warn };
            vec![base.notice(level, "info", clip(d.at("message").or(d.at("error")), 24_000))]
        }
        _ => vec![],
    }
}

/// `guessCopilotSession(host, home, proc)`: both commands at once, as the TS's Promise.all.
pub async fn find_copilot_session(host: &Host, home: &str, proc: &CopilotProc) -> HubResult<Option<String>> {
    if proc.pid <= 0 || proc.pid > MAX_SAFE {
        return Ok(None);
    }
    let (locks, tree) = futures_util::future::join(host.run(&copilot_locks_command(home)), host.run("ps -eo pid=,ppid=")).await;
    Ok(guess_copilot_session(&locks?.stdout, &tree?.stdout, proc))
}

#[cfg(test)]
#[path = "copilot_tests.rs"]
mod tests;
