//! Port of agents/opencode.ts: OpenCode's sessions and messages into chat items.
//!
//! OpenCode's terminal app talks to its own local HTTP server (the same API `opencode serve`
//! exposes). Inyon finds that server's port from the process's listening sockets and reads the
//! same session you see in tmux. The pure functions read what the commands and requests return;
//! the async ones run them on the machine.

use inyon_protocol::{AgentStatus, ChatItem, NoticeLevel, Num};
use serde_json::{Map, Value};

use super::status::empty_status;
use crate::claude::transcript::{numbered, stdout_meta, string_of, text_block, thinking_block, tool_block, Base, ToolCall, TranscriptMeta};
use crate::clock::now_ms;
use crate::hosts::host::Host;
use crate::hosts::http_error::HubResult;
use crate::js::json::{obj, value_string, Js};
use crate::js::re::split_plain;
use crate::js::{date, JsStr};
use crate::re;

/// The session a terminal shows, from the server's `/session` list: the one `-s` named, the one
/// the terminal title names, or the only top-level one in the folder. Never a guess.
pub fn select_open_code_session(sessions: &[Value], cwd: Option<&str>, explicit_session: Option<&str>, title: Option<&str>) -> Option<String> {
    let cwd = cwd.filter(|c| !c.is_empty());
    let mine: Vec<Option<&Value>> = sessions.iter().map(Some).filter(|s| s.at("id").str().is_some() && cwd.is_none_or(|c| s.at("directory").str() == Some(c))).collect();
    let id = |s: &Option<&Value>| s.at("id").str().map(str::to_owned);
    if let Some(explicit) = explicit_session.filter(|e| !e.is_empty()) {
        return mine.iter().find(|s| s.at("id").str() == Some(explicit)).and_then(id);
    }
    let roots: Vec<Option<&Value>> = mine.into_iter().filter(|s| !s.at("parentID").truthy() && !s.at("time").at("archived").truthy()).collect();
    // the terminal's title names the conversation on screen ("OC | <title>"): with several in the
    // folder, that's the one (the newest when two share a title)
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        let mut named: Vec<&Option<&Value>> = roots.iter().filter(|s| s.at("title").str() == Some(title)).collect();
        let updated = |s: &Option<&Value>| s.at("time").at("updated").num().unwrap_or(0.0);
        named.sort_by(|a, b| updated(b).total_cmp(&updated(a)));
        if let Some(first) = named.first() {
            return id(first);
        }
    }
    // There is no endpoint for the TUI's active session. Never guess another conversation.
    if roots.len() == 1 {
        id(&roots[0])
    } else {
        None
    }
}

/// The TUI puts the conversation's title in the terminal title: `OC | <title>`.
pub fn titled_session(pane_title: Option<&str>) -> Option<String> {
    re!(r"^OC \| (.+)$").captures(pane_title?).map(|m| m[1].jtrim().to_owned())
}

/// The commands `opencodeEndpoint` runs for process `pid`: its listening sockets, its command
/// line and its children. None for a pid that can't be one.
pub fn opencode_endpoint_commands(pid: i64) -> Option<[String; 3]> {
    if pid <= 0 || pid > 9_007_199_254_740_991 {
        return None;
    }
    Some(["ss -ltnpH 2>/dev/null".into(), format!("cat /proc/{pid}/cmdline 2>/dev/null"), format!("pgrep -P {pid} 2>/dev/null")])
}

/// What those commands say: the ports the process (or a child) listens on, in order, and the
/// session its command line names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndpointProbe {
    pub ports: Vec<String>,
    pub explicit: Option<String>,
}

pub fn opencode_endpoint_probe(pid: i64, sockets: &str, cmdline: &str, children: &str) -> EndpointProbe {
    let args: Vec<&str> = cmdline.split('\0').filter(|a| !a.is_empty()).collect();
    let explicit = match args.iter().position(|a| *a == "--session" || *a == "-s") {
        Some(i) => args.get(i + 1).map(|s| (*s).to_owned()),
        None => args.iter().find_map(|a| a.strip_prefix("--session=")).map(str::to_owned),
    };
    let mut pids = vec![pid.to_string()];
    pids.extend(split_plain(children.jtrim(), re!(r"\s+")).into_iter().map(str::to_owned));
    let mut ports: Vec<String> = vec![];
    for line in sockets.split('\n') {
        if !re!(r"pid=(\d+),", "g").captures_iter(line).any(|m| pids.iter().any(|p| *p == m[1])) {
            continue;
        }
        let field = split_plain(line.jtrim(), re!(r"\s+")).get(3).copied().unwrap_or("");
        if let Some(port) = re!(r":(\d+)$").captures(field).map(|m| m[1].to_owned()) {
            if !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    EndpointProbe { ports, explicit }
}

/// The endpoint for one port, once its `/global/health` said it's healthy and `/session` gave
/// `list`: the session's uri, the home screen's, or None to try the next port.
pub fn opencode_port_endpoint(port: &str, list: &Value, cwd: Option<&str>, explicit: Option<&str>, pane_title: Option<&str>) -> Option<String> {
    let list = list.as_array()?;
    if let Some(session) = select_open_code_session(list, cwd, explicit, titled_session(pane_title).as_deref()) {
        return Some(format!("opencode://127.0.0.1:{port}/{session}"));
    }
    // the home screen: no conversation yet (its own title says so), or none in this folder
    if list.is_empty() || pane_title.map(JsStr::jtrim) == Some("OpenCode") {
        return Some(format!("opencode://127.0.0.1:{port}/"));
    }
    None
}

/// An endpoint uri's server and session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub base: String,
    pub session: String,
}

pub fn parse_endpoint(uri: &str) -> Option<Endpoint> {
    let m = re!(r"^opencode:\/\/(127\.0\.0\.1):(\d+)\/([^?#]*)$").captures(uri)?;
    Some(Endpoint { base: format!("http://{}:{}", &m[1], &m[2]), session: m[3].to_owned() })
}

/// OpenCode started with --port has an HTTP API; plain `opencode` is read from its database and
/// driven through its screen.
pub fn via_open_code_api(uri: Option<&str>) -> bool {
    uri.is_some_and(|u| u.starts_with("opencode://"))
}

fn map_tool(tool: &str, input: Option<&Value>) -> ToolCall {
    let empty = Value::Object(Map::new());
    let a = input.present().or(Some(&empty));
    let get = |k: &str| a.at(k).cloned();
    let call = |name: &str, input: Value| ToolCall { name: name.into(), input };
    match tool {
        "bash" => call("Bash", obj([("command", get("command")), ("description", get("description"))])),
        "read" => call("Read", obj([("file_path", a.at("filePath").or(a.at("path")).cloned())])),
        "write" => call("Write", obj([("file_path", get("filePath")), ("content", get("content"))])),
        "edit" => call("Edit", obj([("file_path", get("filePath")), ("old_string", get("oldString")), ("new_string", get("newString"))])),
        "patch" => call("ApplyPatch", obj([("patch", a.at("patchText").or(a.at("patch")).cloned())])),
        "glob" => call("Glob", obj([("pattern", get("pattern"))])),
        "grep" => call("Grep", obj([("pattern", get("pattern"))])),
        "list" => call("LS", obj([("path", get("path"))])),
        "webfetch" => call("WebFetch", obj([("url", get("url"))])),
        "todowrite" => {
            let todos = a.at("todos").arr().map(Vec::as_slice).unwrap_or_default();
            let todos =
                todos.iter().map(|t| obj([("content", t.get("content").cloned()), ("activeForm", t.get("content").cloned()), ("status", t.get("status").cloned())])).collect();
            call("TodoWrite", obj([("todos", Some(Value::Array(todos)))]))
        }
        "task" => call("Agent", obj([("description", get("description")), ("prompt", get("prompt")), ("subagent_type", get("subagent_type"))])),
        // same shape as Claude's question tool
        "question" => call("AskUserQuestion", obj([("questions", get("questions"))])),
        _ => call(tool, a.cloned().unwrap_or(empty.clone())),
    }
}

/// `new Date(v).toISOString()`, empty where the TS would throw
fn iso(v: &Value) -> String {
    date::to_iso(date::time_of(v)).unwrap_or_default()
}

fn is_tool(p: &Value, tool: &str) -> bool {
    p.get("type").and_then(Value::as_str) == Some("tool") && p.get("tool").and_then(Value::as_str) == Some(tool)
}

/// OpenCode messages ({info, parts}[]) → chat items.
pub fn opencode_items(messages: &[Value], meta: &mut TranscriptMeta) -> Vec<ChatItem> {
    let mut out: Vec<ChatItem> = vec![];
    let mut cost = 0.0;
    let mut has_cost = false;
    meta.stats.turns = 0;
    for msg in messages {
        let msg = Some(msg);
        let info = msg.at("info").present().or(msg);
        let parts = msg.at("parts").arr().map(Vec::as_slice).unwrap_or_default();
        let created = info.at("time").at("created");
        let ts = if created.truthy() { iso(created.expect("truthy")) } else { String::new() };
        let id = info.at("id").string_or_empty();
        let base = Base::new(&id, ts);
        if meta.stats.started_at.as_deref().is_none_or(str::is_empty) && created.truthy() {
            meta.stats.started_at = Some(iso(created.expect("truthy")));
        }
        let model = info.at("model").at("modelID").or(info.at("modelID"));
        let provider = info.at("model").at("providerID").or(info.at("providerID"));
        if model.truthy() {
            let model = model.to_js_string();
            meta.model = Some(if provider.truthy() { format!("{}/{model}", provider.to_js_string()) } else { model });
        }
        if info.at("agent").truthy() {
            meta.mode = info.at("agent").map(value_string);
        }
        let kind = |p: &Value| p.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
        if info.at("role").str() == Some("user") {
            meta.effort = string_of(info.at("variant"));
            meta.stats.turns += 1;
            let text: Vec<String> = parts.iter().filter(|p| kind(p) == "text" && !Some(*p).at("synthetic").truthy()).map(|p| Some(p).at("text").string_or_empty()).collect();
            let text = text.join("\n");
            let text = text.jtrim();
            if !text.is_empty() {
                out.push(base.user(text, parts.iter().filter(|p| kind(p) == "file").count() as u32));
            } else if parts.iter().any(|p| kind(p) == "text" && Some(p).at("synthetic").truthy() && Some(p).at("text").str() == Some("The following tool was executed by the user"))
            {
                // the reply that ran it names this message as its parent; without an id there's none to name
                let result = info.at("id").present().and_then(|id| {
                    messages.iter().find(|m| {
                        let m = Some(*m);
                        m.at("info").at("parentID") == Some(id) && m.at("parts").arr().is_some_and(|ps| ps.iter().any(|p| is_tool(p, "bash")))
                    })
                });
                let command = result.and_then(|r| r["parts"].as_array()?.iter().find(|p| is_tool(p, "bash"))).and_then(|p| Some(p).at("state").at("input").at("command").str());
                if let Some(command) = command {
                    out.push(base.shell_user(command));
                }
            }
            continue;
        }
        if info.at("variant").truthy() {
            meta.effort = info.at("variant").map(value_string);
        }
        // stopping a turn isn't a failure
        let error = info.at("error");
        if error.at("name").str() == Some("MessageAbortedError") {
            out.push(base.with_uuid(format!("{id}:error")).notice(NoticeLevel::Info, "interrupted", "Stopped"));
        } else if error.truthy() {
            let text = error.at("data").at("message").or(error.at("message")).or(error.at("name")).string_or("OpenCode request failed");
            out.push(base.with_uuid(format!("{id}:error")).notice(NoticeLevel::Error, "api_error", text));
        }
        if let Some(c) = info.at("cost").num() {
            cost += c;
            has_cost = true;
        }
        // the context is what the last reply with usage took, counting everything the TUI's
        // sidebar counts: input, output, reasoning and both kinds of cache
        let t = info.at("tokens");
        let n = |v: Option<&Value>| v.num().unwrap_or(0.0);
        if t.truthy() && (n(t.at("output")) > 0.0 || n(t.at("total")) > 0.0) {
            let total =
                t.at("total").num().unwrap_or_else(|| n(t.at("input")) + n(t.at("output")) + n(t.at("reasoning")) + n(t.at("cache").at("read")) + n(t.at("cache").at("write")));
            meta.stats.context_tokens = Some(Num(total));
            let at = info.at("time").at("completed").or(info.at("time").at("created"));
            if at.truthy() {
                if let Some(at) = at.num() {
                    meta.stats.context_at = Some(at as u64);
                }
            }
        }
        for p in parts {
            let p = Some(p);
            let uuid = string_of(p.at("id")).unwrap_or_else(|| format!("{id}:{}", out.len()));
            let pb = base.with_uuid(&uuid);
            let text = p.at("text").str().filter(|t| !t.jtrim().is_empty());
            match p.at("type").str() {
                Some("text") if text.is_some() => out.push(pb.assistant(&id, string_of(info.at("modelID")), text_block(text.unwrap_or("")))),
                Some("reasoning") if text.is_some() => out.push(pb.assistant(&id, None, thinking_block(text.unwrap_or("")))),
                Some("tool") => {
                    let call_id = string_of(p.at("callID").or(p.at("id"))).unwrap_or_default();
                    let empty = Value::Object(Map::new());
                    let st = p.at("state").present().or(Some(&empty));
                    let mapped = map_tool(&p.at("tool").to_js_string(), st.at("input"));
                    out.push(pb.assistant(&id, None, tool_block(&call_id, &mapped.name, mapped.input.clone())));
                    let status = st.at("status").str();
                    if matches!(status, Some("completed" | "error")) {
                        let text = st.at("output").or(st.at("error")).string_or_empty();
                        let text = text.slice16(0, Some(8000)).to_owned();
                        let meta = (mapped.name == "Bash").then(|| stdout_meta(text.clone()));
                        out.push(base.with_uuid(format!("{uuid}:r")).tool_result(&call_id, status == Some("error"), text, None, meta));
                    }
                }
                _ => {}
            }
        }
    }
    meta.stats.cost_usd = has_cost.then_some(Num(cost));
    meta.status = Some(AgentStatus {
        model: meta.model.clone(),
        model_id: meta.model.clone(),
        effort: meta.effort.clone(),
        context_used: meta.stats.context_tokens,
        cost_usd: meta.stats.cost_usd,
        ..empty_status(now_ms() as f64)
    });
    numbered(out)
}

/// `opencodeEndpoint(host, pid, cwd, paneTitle)`: the server an OpenCode process started with
/// --port listens on, and the session it shows.
pub async fn opencode_endpoint(host: &Host, pid: i64, cwd: Option<&str>, pane_title: Option<&str>) -> HubResult<Option<String>> {
    let Some([a, b, c]) = opencode_endpoint_commands(pid) else { return Ok(None) };
    let (sockets, process, children) = futures_util::future::join3(host.run(&a), host.run(&b), host.run(&c)).await;
    let probe = opencode_endpoint_probe(pid, &sockets?.stdout, &process?.stdout, &children?.stdout);
    for port in &probe.ports {
        let base = format!("http://127.0.0.1:{port}");
        let healthy = opencode_get(host, &format!("{base}/global/health")).await;
        if !healthy.as_ref().is_some_and(|h| h.get("healthy") == Some(&Value::Bool(true))) {
            continue;
        }
        let query = cwd.filter(|c| !c.is_empty()).map(|c| format!("?directory={}", crate::js::text::encode_uri_component(c))).unwrap_or_default();
        let Some(list) = opencode_get(host, &format!("{base}/session{query}")).await.filter(Value::is_array) else { continue };
        if let Some(uri) = opencode_port_endpoint(port, &list, cwd, probe.explicit.as_deref(), pane_title) {
            return Ok(Some(uri));
        }
    }
    Ok(None)
}

/// `opencodeGet`: the answer, or None for any failure.
pub async fn opencode_get(host: &Host, url: &str) -> Option<Value> {
    super::opencode_api::opencode_request(host, url, "GET", None, 12).await.ok().flatten()
}

/// `opencodePost`: whether it went through (an answer of `false` counts as no).
pub async fn opencode_post(host: &Host, url: &str, body: &Value) -> bool {
    match super::opencode_api::opencode_request(host, url, "POST", Some(body), 12).await {
        Ok(v) => v != Some(Value::Bool(false)),
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "opencode_tests.rs"]
mod tests;
