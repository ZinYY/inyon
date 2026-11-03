//! `tools.ts`: a verb, a short target and an icon for each tool call, for the rows in a turn.

use serde::Deserialize;
use serde_json::Value;

use crate::chat::ToolCall;
use crate::format::short_path;
use crate::js::{cut16, len16, lines};

/// The icon a summary shows, named after the lucide icon the web app draws for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolIcon {
    Bell,
    BookMarked,
    Bot,
    ClipboardList,
    FileCode2,
    FilePen,
    FilePlus2,
    FileSearch,
    FileText,
    FolderSearch,
    Globe,
    Hourglass,
    Image,
    ListChecks,
    MessageCircleQuestion,
    MessageSquare,
    Plug,
    ScrollText,
    Search,
    SquareTerminal,
    Timer,
    Workflow,
    Wrench,
}

/// One call inside a Codex exec script (see codexScript in the hub).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ScriptStep {
    pub tool: String,
    pub arg: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub verb: String,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub add: usize,
    pub del: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSummary {
    pub icon: ToolIcon,
    pub verb: String,
    pub target: Option<String>,
    /// monospace target (commands, patterns)
    pub mono: bool,
    pub stats: Option<Stats>,
}

/// The files a Codex patch touches.
fn patch_files(patch: &str) -> Vec<String> {
    lines(patch)
        .filter_map(|l| {
            let rest = l.strip_prefix("*** ")?;
            let (word, file) = rest.split_once(" File: ")?;
            (matches!(word, "Update" | "Add" | "Delete") && !file.is_empty()).then(|| short_path(Some(file), 40))
        })
        .collect()
}

fn first_line(s: Option<&str>) -> String {
    let Some(s) = s.filter(|s| !s.is_empty()) else { return String::new() };
    let l = s.split('\n').next().unwrap_or("");
    if len16(l) > 120 {
        format!("{}…", cut16(l, 120))
    } else {
        l.to_string()
    }
}

/// A verb and a short target for one step of a script.
pub fn describe_step(s: &ScriptStep) -> Step {
    let arg = s.arg.as_deref().filter(|a| !a.is_empty());
    let path = || arg.map(|a| short_path(Some(a), 40));
    let (verb, target) = match s.tool.as_str() {
        "exec_command" => ("Ran".to_string(), Some(first_line(arg))),
        "write_stdin" => ("Typed".into(), Some(first_line(arg))),
        "apply_patch" => ("Patched".into(), arg.map(|a| patch_files(a).join(", "))),
        "view_image" => ("Viewed".into(), path()),
        "read_file" => ("Read".into(), path()),
        "write_file" => ("Wrote".into(), path()),
        "list_dir" => ("Listed".into(), path()),
        "grep_files" => ("Searched".into(), s.arg.clone()),
        other => (other.replace('_', " "), arg.map(|a| first_line(Some(a)))),
    };
    Step { verb, target }
}

fn patch_stats(call: &ToolCall) -> Option<Stats> {
    let patch = call.meta()?.patch.as_ref()?;
    let mut stats = Stats { add: 0, del: 0 };
    for l in patch.iter().flat_map(|h| &h.lines) {
        if l.starts_with('+') {
            stats.add += 1;
        } else if l.starts_with('-') {
            stats.del += 1;
        }
    }
    Some(stats)
}

/// `new URL(u).host + pathname` without its trailing slash, or `u` when it isn't a URL.
fn host(u: &str) -> String {
    web_address(u).unwrap_or_else(|| u.to_string())
}

/// What the WHATWG URL parser makes of a web address's host, port and path, by hand: the `url`
/// crate brings Unicode tables a page this size doesn't want. Covers what agents fetch (http,
/// https, ws, wss and ftp with `://`): the host lowercased, a default port dropped, dot
/// segments taken out and the path percent-encoded as `new URL()` does. Anything else, a host
/// outside ASCII among them, is left to show as it was written.
fn web_address(u: &str) -> Option<String> {
    let u = u.trim_matches(|c: char| c <= ' ');
    let (scheme, rest) = u.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        "ftp" => 21,
        _ => return None,
    };
    if rest.contains(['\\', '\t', '\n', '\r']) {
        return None;
    }
    // more slashes than two before the host are skipped
    let rest = rest.trim_start_matches('/');
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (name, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.starts_with('[') || h.ends_with(']') => (h, Some(p)),
        _ => (hostport, None),
    };
    let forbidden = |c: char| !c.is_ascii() || c.is_ascii_control() || " #%/:<>?@[\\]^|".contains(c);
    let name = if name.starts_with('[') && name.ends_with(']') {
        name.to_ascii_lowercase()
    } else if name.contains(forbidden)
        || name.rsplit('.').find(|l| !l.is_empty()).is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()) || l.starts_with("0x")) && !dotted_quad(name)
    {
        // percent escapes, Unicode, and numbers the parser reads as an IPv4 address of its own
        return None;
    } else {
        name.to_ascii_lowercase()
    };
    if name.is_empty() {
        return None;
    }
    let mut out = name;
    match port.filter(|p| !p.is_empty()) {
        Some(p) if !p.bytes().all(|b| b.is_ascii_digit()) => return None,
        Some(p) => {
            let n: u32 = match p.trim_start_matches('0') {
                "" => 0,
                digits if digits.len() <= 5 => digits.parse().ok()?,
                _ => return None,
            };
            if n > 65535 {
                return None;
            }
            if n != default_port {
                out.push_str(&format!(":{n}"));
            }
        }
        None => {}
    }
    let path = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
    let path = web_path(path);
    Some(out + path.strip_suffix('/').unwrap_or(&path))
}

/// Four numbers from 0 to 255, as the parser writes an IPv4 address back.
fn dotted_quad(h: &str) -> bool {
    let parts: Vec<&str> = h.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0')) && p.parse::<u16>().is_ok_and(|n| n <= 255))
}

/// A special URL's path: `.` and `..` segments (written `%2e` too) resolved, and what the path
/// percent-encode set holds escaped.
fn web_path(path: &str) -> String {
    let segments: Vec<&str> = path.strip_prefix('/').unwrap_or(path).split('/').collect();
    let n = segments.len();
    let mut out: Vec<String> = Vec::new();
    for (i, seg) in segments.into_iter().enumerate() {
        let last = i + 1 == n;
        let lower = seg.to_ascii_lowercase();
        if matches!(lower.as_str(), ".." | ".%2e" | "%2e." | "%2e%2e") {
            out.pop();
            if last {
                out.push(String::new());
            }
        } else if matches!(lower.as_str(), "." | "%2e") {
            if last {
                out.push(String::new());
            }
        } else {
            let mut e = String::new();
            for b in seg.bytes() {
                if b <= b' ' || b > b'~' || b"\"#<>?`{}".contains(&b) {
                    e.push_str(&format!("%{b:02X}"));
                } else {
                    e.push(b as char);
                }
            }
            out.push(e);
        }
    }
    format!("/{}", out.join("/"))
}

/// A tool input's field when it's a non-empty string (JavaScript's `i.x || …`).
fn s<'a>(i: &'a Value, key: &str) -> Option<&'a str> {
    i.get(key).and_then(Value::as_str).filter(|v| !v.is_empty())
}

/// `i.a || i.b || …`: the first field that is a non-empty string.
fn first(i: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| s(i, k)).map(str::to_string)
}

fn summary(icon: ToolIcon, verb: impl Into<String>, target: Option<String>) -> ToolSummary {
    ToolSummary { icon, verb: verb.into(), target, mono: false, stats: None }
}

pub fn summarize(call: &ToolCall) -> ToolSummary {
    use ToolIcon::*;
    let empty = Value::Object(Default::default());
    let i = if call.input.is_null() { &empty } else { &call.input };
    let done = call.done();
    let path60 = |k: &str| Some(short_path(s(i, k), 60));
    let pick = |a: &'static str, b: &'static str| if done { a } else { b };
    match call.name.as_str() {
        "Bash" => {
            let d = s(i, "description");
            ToolSummary { mono: d.is_none(), ..summary(SquareTerminal, pick("Ran", "Running"), Some(d.map_or_else(|| first_line(s(i, "command")), str::to_string))) }
        }
        "BashOutput" | "TaskOutput" => summary(SquareTerminal, "Read output", first(i, &["bash_id", "task_id"])),
        "Read" => summary(FileText, "Read", path60("file_path")),
        "Edit" | "MultiEdit" => ToolSummary { stats: patch_stats(call), ..summary(FilePen, pick("Edited", "Editing"), path60("file_path")) },
        "Write" => {
            let rewrote = call.meta().and_then(|m| m.created) == Some(false);
            let verb = if !done {
                "Writing"
            } else if rewrote {
                "Rewrote"
            } else {
                "Created"
            };
            ToolSummary { stats: patch_stats(call), ..summary(if rewrote { FilePen } else { FilePlus2 }, verb, path60("file_path")) }
        }
        "NotebookEdit" => summary(FileCode2, "Edited notebook", path60("notebook_path")),
        "Grep" => ToolSummary { mono: true, ..summary(Search, "Searched", first(i, &["pattern"])) },
        "Glob" => ToolSummary { mono: true, ..summary(FolderSearch, "Found files", first(i, &["pattern"])) },
        "LS" => summary(FolderSearch, "Listed", path60("path")),
        "WebFetch" => summary(Globe, "Fetched", Some(host(i.get("url").and_then(Value::as_str).unwrap_or("")))),
        "WebSearch" => summary(Globe, "Searched the web", first(i, &["query"])),
        "Task" | "Agent" => summary(Bot, "Agent", first(i, &["description", "subagent_type"])),
        "Skill" => summary(BookMarked, "Skill", first(i, &["skill", "command"])),
        "Workflow" => summary(Workflow, "Workflow", Some(first(i, &["name"]).unwrap_or_else(|| "script".into()))),
        "TaskCreate" | "TaskUpdate" | "TaskList" => summary(ListChecks, "Tasks", first(i, &["subject", "title", "status"])),
        "ToolSearch" => summary(FileSearch, "Loaded tools", first(i, &["query"])),
        "Monitor" => summary(Timer, "Monitor", Some(first(i, &["description"]).unwrap_or_else(|| first_line(s(i, "command"))))),
        "PushNotification" => summary(Bell, "Notified", first(i, &["title", "message"])),
        "EnterPlanMode" => summary(ClipboardList, "Entered plan mode", None),
        "AskUserQuestion" => summary(MessageCircleQuestion, "Asked", None),
        "ApplyPatch" => {
            let patch = i.get("patch").map(js_string).unwrap_or_default();
            // JavaScript's /^\+(?!\+\+)/gm: lines that start with one +, not a +++ header
            let count = |c: &str| lines(&patch).filter(|l| l.starts_with(c) && !l[1..].starts_with(&c.repeat(2))).count();
            let (add, del) = (count("+"), count("-"));
            ToolSummary { stats: Some(patch_stats(call).unwrap_or(Stats { add, del })), ..summary(FilePen, pick("Patched", "Patching"), Some(patch_files(&patch).join(", "))) }
        }
        "Script" => {
            // several tools from one Codex script: say what each did, in order
            let steps: Vec<ScriptStep> =
                i.get("steps").and_then(Value::as_array).map(|a| a.iter().filter_map(|s| serde_json::from_value(s.clone()).ok()).collect()).unwrap_or_default();
            let parts: Vec<String> = steps
                .iter()
                .map(|s| {
                    let d = describe_step(s);
                    match d.target.filter(|t| !t.is_empty()) {
                        Some(t) => format!("{} {t}", d.verb.to_lowercase()),
                        None => d.verb.to_lowercase(),
                    }
                })
                .collect();
            let target = if parts.is_empty() { first_line(Some(&i.get("code").map(js_string).unwrap_or_default())) } else { parts.join(" · ") };
            ToolSummary { icon: ScrollText, verb: pick("Ran a script", "Running a script").into(), target: Some(target), mono: parts.is_empty(), stats: patch_stats(call) }
        }
        "ImageGeneration" => summary(Image, pick("Generated an image", "Generating an image"), Some(first_line(s(i, "prompt")))),
        // Codex's multi-agent tools
        "spawn_agent" => summary(Bot, pick("Started an agent", "Starting an agent"), first(i, &["task_name", "name"])),
        "followup_task" => summary(Bot, "Gave an agent a task", first(i, &["agent_id", "task_name"])),
        "send_message" => summary(MessageSquare, "Messaged an agent", first(i, &["agent_id", "recipient"])),
        "wait_agent" | "wait_agents" => summary(Hourglass, pick("Waited for agents", "Waiting for agents"), None),
        "resume_agent" => summary(Bot, "Resumed an agent", first(i, &["agent_id"])),
        "interrupt_agent" => summary(Bot, "Interrupted an agent", first(i, &["agent_id"])),
        "close_agent" => summary(Bot, "Closed an agent", first(i, &["agent_id"])),
        "list_agents" => summary(Bot, "Listed agents", None),
        name if name.starts_with("mcp__") => {
            let mut parts = name.split("__").skip(1);
            let server = parts.next().unwrap_or("");
            let tool = parts.collect::<Vec<_>>().join("__");
            summary(Plug, tool.replace('_', " "), Some(server.strip_prefix("claude_ai_").unwrap_or(server).replace('_', " ")))
        }
        name => {
            let values: Box<dyn Iterator<Item = &Value>> = match i {
                Value::Object(o) => Box::new(o.values()),
                Value::Array(a) => Box::new(a.iter()),
                _ => Box::new(std::iter::empty()),
            };
            let target = values.filter_map(Value::as_str).find(|v| len16(v) < 120).map(str::to_string);
            summary(Wrench, name, target)
        }
    }
}

/// `String(v)`: a string as it is, a number or a flag as JavaScript prints it.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::item;
    use serde_json::json;

    fn call(name: &str, input: Value) -> ToolCall {
        ToolCall { id: "t".into(), name: name.into(), input, result: None, ts: None }
    }
    fn answered(mut c: ToolCall, meta: Value) -> ToolCall {
        c.result = Some(item::with(&item::result("t", "ok"), json!({ "meta": meta })));
        c
    }
    fn line(s: &ToolSummary) -> (ToolIcon, &str, Option<&str>) {
        (s.icon, s.verb.as_str(), s.target.as_deref())
    }

    #[test]
    fn bash_says_what_it_ran_in_words_when_it_has_them() {
        let running = summarize(&call("Bash", json!({ "command": "cargo test\n--all" })));
        assert_eq!(line(&running), (ToolIcon::SquareTerminal, "Running", Some("cargo test")));
        assert!(running.mono);
        let ran = summarize(&answered(call("Bash", json!({ "command": "ls", "description": "List files" })), json!({})));
        assert_eq!(line(&ran), (ToolIcon::SquareTerminal, "Ran", Some("List files")));
        assert!(!ran.mono);
    }

    #[test]
    fn long_commands_are_cut_at_120_characters() {
        let long = "x".repeat(130);
        let s = summarize(&call("Bash", json!({ "command": long })));
        assert_eq!(s.target.unwrap(), format!("{}…", "x".repeat(120)));
    }

    #[test]
    fn file_tools_show_a_short_path_and_the_lines_changed() {
        let read = summarize(&call("Read", json!({ "file_path": "/home/me/code/src/main.rs" })));
        assert_eq!(line(&read), (ToolIcon::FileText, "Read", Some("~/code/src/main.rs")));
        let patch = json!({ "patch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 3, "lines": ["+a", "+b", "-c", " d"] }] });
        let edit = summarize(&answered(call("Edit", json!({ "file_path": "/srv/a.ts" })), patch));
        assert_eq!(line(&edit), (ToolIcon::FilePen, "Edited", Some("/srv/a.ts")));
        assert_eq!(edit.stats, Some(Stats { add: 2, del: 1 }));
        assert_eq!(summarize(&call("Edit", json!({ "file_path": "/srv/a.ts" }))).verb, "Editing");
    }

    #[test]
    fn write_tells_a_new_file_from_a_rewritten_one() {
        let w = |meta: Value| summarize(&answered(call("Write", json!({ "file_path": "/srv/a.ts" })), meta));
        assert_eq!((w(json!({ "created": true })).icon, w(json!({ "created": true })).verb), (ToolIcon::FilePlus2, "Created".into()));
        assert_eq!((w(json!({})).icon, w(json!({})).verb), (ToolIcon::FilePlus2, "Created".into()));
        assert_eq!((w(json!({ "created": false })).icon, w(json!({ "created": false })).verb), (ToolIcon::FilePen, "Rewrote".into()));
        assert_eq!(summarize(&call("Write", json!({}))).verb, "Writing");
    }

    #[test]
    fn searches_and_fetches() {
        let grep = summarize(&call("Grep", json!({ "pattern": "fn main" })));
        assert_eq!(line(&grep), (ToolIcon::Search, "Searched", Some("fn main")));
        assert!(grep.mono);
        assert_eq!(summarize(&call("WebFetch", json!({ "url": "https://Example.com/docs/" }))).target.as_deref(), Some("example.com/docs"));
        assert_eq!(summarize(&call("WebFetch", json!({ "url": "http://box:4777/" }))).target.as_deref(), Some("box:4777"));
        assert_eq!(summarize(&call("WebFetch", json!({ "url": "not a url" }))).target.as_deref(), Some("not a url"));
        assert_eq!(summarize(&call("WebFetch", json!({}))).target.as_deref(), Some(""));
    }

    #[test]
    fn agents_skills_and_tasks_take_the_first_field_they_have() {
        assert_eq!(summarize(&call("Agent", json!({ "subagent_type": "Explore" }))).target.as_deref(), Some("Explore"));
        assert_eq!(summarize(&call("Task", json!({ "description": "Find it", "subagent_type": "x" }))).target.as_deref(), Some("Find it"));
        assert_eq!(summarize(&call("Workflow", json!({}))).target.as_deref(), Some("script"));
        assert_eq!(summarize(&call("TaskUpdate", json!({ "status": "done" }))).target.as_deref(), Some("done"));
        assert_eq!(line(&summarize(&call("EnterPlanMode", json!({})))), (ToolIcon::ClipboardList, "Entered plan mode", None));
    }

    #[test]
    fn codex_patches_name_their_files_and_count_lines() {
        let patch = "*** Begin Patch\n*** Update File: /home/me/code/a.rs\n@@\n-old\n+new\n+more\n*** Add File: b.rs\n+++ not a line\n*** End Patch";
        let s = summarize(&call("ApplyPatch", json!({ "patch": patch })));
        assert_eq!(line(&s), (ToolIcon::FilePen, "Patching", Some("~/code/a.rs, b.rs")));
        assert_eq!(s.stats, Some(Stats { add: 2, del: 1 }));
    }

    #[test]
    fn a_codex_script_says_what_each_step_did() {
        let steps = json!([{ "tool": "exec_command", "arg": "ls -la\nmore" }, { "tool": "apply_patch", "arg": "*** Update File: x.rs\n" }, { "tool": "list_dir" }, { "tool": "web_search", "arg": "rust" }]);
        let s = summarize(&call("Script", json!({ "steps": steps })));
        assert_eq!(line(&s), (ToolIcon::ScrollText, "Running a script", Some("ran ls -la · patched x.rs · listed · web search rust")));
        assert!(!s.mono);
        let code = summarize(&call("Script", json!({ "code": "print(1)\nprint(2)" })));
        assert_eq!(code.target.as_deref(), Some("print(1)"));
        assert!(code.mono);
    }

    #[test]
    fn steps_on_their_own() {
        let step = |tool: &str, arg: Option<&str>| describe_step(&ScriptStep { tool: tool.into(), arg: arg.map(str::to_string) });
        assert_eq!(step("read_file", Some("/home/me/a.txt")), Step { verb: "Read".into(), target: Some("~/a.txt".into()) });
        assert_eq!(step("read_file", None), Step { verb: "Read".into(), target: None });
        assert_eq!(step("write_stdin", None), Step { verb: "Typed".into(), target: Some("".into()) });
        assert_eq!(step("grep_files", Some("todo")), Step { verb: "Searched".into(), target: Some("todo".into()) });
    }

    #[test]
    fn mcp_tools_read_as_tool_and_server() {
        let s = summarize(&call("mcp__claude_ai_Google_Drive__search_files", json!({})));
        assert_eq!(line(&s), (ToolIcon::Plug, "search files", Some("Google Drive")));
    }

    #[test]
    fn an_unknown_tool_shows_its_first_short_string() {
        let s = summarize(&call("Frobnicate", json!({ "n": 3, "long": "y".repeat(130), "what": "the thing", "other": "later" })));
        assert_eq!(line(&s), (ToolIcon::Wrench, "Frobnicate", Some("the thing")));
        assert_eq!(summarize(&call("Frobnicate", Value::Null)).target, None);
    }

    #[test]
    fn codex_agents() {
        assert_eq!(line(&summarize(&call("spawn_agent", json!({ "name": "fixer" })))), (ToolIcon::Bot, "Starting an agent", Some("fixer")));
        assert_eq!(line(&summarize(&answered(call("wait_agents", json!({})), json!({})))), (ToolIcon::Hourglass, "Waited for agents", None));
    }

    #[test]
    fn web_addresses_read_as_the_url_parser_reads_them() {
        let by_url = |u: &str| match url::Url::parse(u) {
            Ok(p) => {
                let mut h = p.host_str().unwrap_or("").to_string();
                if let Some(port) = p.port() {
                    h.push_str(&format!(":{port}"));
                }
                h + p.path().strip_suffix('/').unwrap_or(p.path())
            }
            Err(_) => u.to_string(),
        };
        let schemes = ["http://", "https://", "HTTPS://", "ws://", "wss://", "ftp://"];
        let hosts = ["Example.com", "box", "a-b.c", "127.0.0.1", "localhost", "sub.EXAMPLE.org", "[::1]", "user:pw@docs.rs", "@x.io", "a.123", "10.0.0.256", "0xg.io"];
        let ports = ["", ":80", ":443", ":4777", ":", ":0", ":021", ":65536", ":99999999999", ":x"];
        let pieces = ["/", "docs", "a b", "é", "..", ".", "%2e", "%2E%2e", "%20", "?q=1", "#top", "{}", "`", "\"", "<", ">", "~", "%", "\u{7f}", "x.y", "-_"];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..6000 {
            let scheme = schemes[next(schemes.len())];
            let mut u = format!("{scheme}{}{}", hosts[next(hosts.len())], ports[next(ports.len())]);
            let path = ["", "/", "?", "#"][next(4)];
            u.push_str(path);
            for _ in 0..if path.is_empty() { 0 } else { next(6) } {
                u.push_str(pieces[next(pieces.len())]);
            }
            assert_eq!(host(&u), by_url(&u), "{u:?}");
        }
        // left as written, where the parser would bring in its Unicode tables
        assert_eq!(host("https://bücher.de/x"), "https://bücher.de/x");
        assert_eq!(host("mailto:sam@example.com"), "mailto:sam@example.com");
        assert_eq!(host("file:///tmp/notes.md"), "file:///tmp/notes.md");
        // and a shortened IPv4 address, which the parser writes out in full (1.2.0.3)
        assert_eq!(host("http://1.2.3/"), "http://1.2.3/");
    }
}
