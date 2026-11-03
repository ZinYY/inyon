//! Port of agents/codex.ts: finding a Codex process's rollout file, reading its lines into chat
//! items, and its status and permissions. The commands are built and read by pure functions; the
//! async ones at the end run them on a machine.

use std::collections::HashMap;

use inyon_protocol::{
    AgentStatus, ChatGoal, ChatItem, NoticeLevel, Num, PatchHunk, Stale, StaleContext, StaleLimits, StringMap, SubagentInfo, SubagentStatus, ToolResultMeta, Turn,
};
use serde_json::{Map, Value};

use super::status::{status_from_codex, CodexContext};
use crate::claude::transcript::{clip_text, result_meta, stdout_meta, string_of, text_block, thinking_block, tool_block, Base, TranscriptMeta};
use crate::clock::now_ms;
use crate::hosts::host::{shq, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{obj, s, stringify, value_string, Js};
use crate::js::re::dynamic;
use crate::js::{date, to_number, JsStr};
use crate::re;
use crate::sha::sha256_hex;

/// A Codex process as the rollout picker sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexProc {
    pub pid: i64,
    pub cwd: Option<String>,
    pub started_at: f64,
    pub resume: Option<String>,
    /// the pane's title as Codex set it (`<thread name> | <folder>`)
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexRollout {
    pub path: String,
    pub id: Option<String>,
    pub cwd: Option<String>,
    /// seconds: when the thread was created (session_meta) and last written
    pub created: f64,
    pub mtime: f64,
}

/// The first of the rows with the highest mtime (`sort((a, b) => b.mtime - a.mtime)[0]`).
fn newest<'a>(rows: impl Iterator<Item = &'a CodexRollout>) -> Option<&'a CodexRollout> {
    rows.fold(None, |best: Option<&CodexRollout>, r| match best {
        Some(b) if b.mtime >= r.mtime => Some(b),
        _ => Some(r),
    })
}

/// Codex keeps one rollout file per thread under ~/.codex/sessions/YYYY/MM/DD/, all written by
/// its shared app-server daemon, so no process holds "its" file open. The thread is found by, in
/// order: the id it was resumed with; the thread name Codex put in the terminal title; else
/// creation time. A thread belongs to the newest Codex process in that folder that had started
/// when the thread was made, so two sessions in one folder don't both claim the latest file, while
/// /new in the older one still lands on its own new thread once nothing newer was started.
pub fn pick_codex_rollout(rows: &[CodexRollout], proc: &CodexProc, others: &[CodexProc], names: &HashMap<String, String>) -> Option<String> {
    if let Some(resume) = proc.resume.as_deref().filter(|r| !r.is_empty()) {
        if let Some(hit) = newest(rows.iter().filter(|r| r.id.as_deref() == Some(resume) || r.path.contains(resume))) {
            return Some(hit.path.clone());
        }
    }
    let cwd = proc.cwd.as_deref().filter(|c| !c.is_empty());
    let since = proc.started_at - 5.0;
    let here: Vec<&CodexRollout> = rows.iter().filter(|r| cwd.is_none() || r.cwd.as_deref() == cwd).collect();
    let mine: Vec<&CodexRollout> = here.iter().copied().filter(|r| r.created >= since).collect();
    // a thread picked with /resume was made before the process started, but is written since; only
    // its name (Codex names every thread) tells it from one a Codex outside tmux is writing
    let touched: Vec<&CodexRollout> = here.iter().copied().filter(|r| r.created >= since || r.mtime >= since).collect();
    let name = proc.title.as_deref().filter(|t| !t.is_empty()).map(|t| codex_title(t, proc.cwd.as_deref())).unwrap_or_default();
    if !name.is_empty() {
        let named: Vec<&CodexRollout> = touched.iter().copied().filter(|r| r.id.as_deref().filter(|i| !i.is_empty()).and_then(|i| names.get(i)) == Some(&name)).collect();
        if named.len() == 1 {
            return Some(named[0].path.clone());
        }
        let fresh: Vec<&&CodexRollout> = named.iter().filter(|r| r.created >= since).collect();
        if fresh.len() == 1 {
            return Some(fresh[0].path.clone());
        }
    }
    let rivals: Vec<&CodexProc> = others.iter().filter(|o| o.pid != proc.pid && o.cwd == proc.cwd && o.started_at > proc.started_at).collect();
    let owned = mine.into_iter().filter(|r| !rivals.iter().any(|o| o.started_at - 5.0 <= r.created));
    newest(owned).map(|r| r.path.clone())
}

/// The command `guessCodexRollout` runs: the recent rollout files with their first line, then the
/// thread names. `now_s` is the time in seconds.
pub fn guess_codex_rollout_command(home: &str, proc: &CodexProc, others: &[CodexProc], now_s: f64) -> String {
    let oldest = others.iter().map(|o| o.started_at).fold(proc.started_at, f64::min);
    let age_min = (((now_s - oldest) / 60.0).ceil() + 2.0).max(5.0);
    format!(
        r#"find {} -name 'rollout-*.jsonl' -mmin -{} 2>/dev/null | while read -r f; do printf '%s\t%s\t%s\n' "$(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f")" "$f" "$(head -c 4000 "$f" | head -n 1)"; done; echo '---'; tail -n 400 {} 2>/dev/null"#,
        shq(&format!("{home}/.codex/sessions")),
        crate::js::fmt_number(age_min),
        shq(&format!("{home}/.codex/session_index.jsonl"))
    )
}

/// What `guessCodexRollout` reads out of that command's output: the rollout files and the latest
/// name per thread (/rename appends a line).
pub fn codex_rollouts(stdout: &str) -> (Vec<CodexRollout>, HashMap<String, String>) {
    let mut halves = stdout.split("\n---\n");
    let list = halves.next().unwrap_or("");
    let index = halves.next().unwrap_or("");
    let rows = list
        .split('\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let m = re!(r"^(\d+)\t([^\t]+)\t(.*)$").captures(l)?;
            let head = m.get(3).map_or("", |x| x.as_str());
            // read the fields straight from the text: since 0.162 the first line is ~20 kB, so the
            // head we read is cut off and not valid JSON
            let field = |k: &str| -> Option<String> {
                let v = dynamic(&format!(r#""{k}":"((?:[^"\\]|\\.)*)""#), "").captures(head)?.get(1)?.as_str().to_owned();
                Some(crate::js::json::parse(&format!("\"{v}\"")).and_then(|x| x.as_str().map(str::to_owned)).unwrap_or(v))
            };
            let mtime = to_number(&m[1]);
            let parsed = date::parse(&field("timestamp").unwrap_or_default()) / 1000.0;
            let created = if parsed.is_nan() || parsed == 0.0 { mtime } else { parsed };
            Some(CodexRollout { mtime, path: m[2].to_owned(), cwd: field("cwd"), id: field("id"), created })
        })
        .collect();
    let mut names = HashMap::new();
    for l in index.split('\n') {
        let Some(e) = crate::js::json::parse(l) else { continue };
        let e = Some(&e);
        if let (Some(id), true) = (e.at("id").str().filter(|i| !i.is_empty()), e.at("thread_name").truthy()) {
            // the TS keeps the name as it is, and a number is never `===` a pane title
            match e.at("thread_name").str() {
                Some(name) => names.insert(id.to_owned(), name.to_owned()),
                None => names.remove(id),
            };
        }
    }
    (rows, names)
}

/// After a rewind (or with paginated history) Codex starts a new segment file for the thread,
/// `rollout-…-<thread>_<segment>.jsonl`, whose session_meta says where the kept history ends:
/// history_base {thread_id, end_byte_offset}. This reads that from the segment's first line;
/// `codexHistory` follows bases of bases with the commands below.
pub fn codex_history_base(first_line: &str) -> Option<(String, u64)> {
    let m = re!(r#""history_base":\{"thread_id":"([0-9a-f-]{36})"[^}]*?"end_byte_offset":(\d+)"#).captures(first_line)?;
    Some((m[1].to_owned(), to_number(&m[2]) as u64))
}

/// The command that prints a rollout's first line (where its history_base is).
pub fn codex_history_head_command(path: &str) -> String {
    format!("head -c 200000 {} 2>/dev/null | head -n 1", shq(path))
}

/// The command that finds the file a segment continues: the thread's first file ends
/// `-<thread>.jsonl`, a later segment `<thread>_<segment>.jsonl`.
pub fn codex_history_find_command(path: &str, id: &str) -> String {
    let dir = re!(r"\/sessions\/.*$").replacen(path, 1, "/sessions").into_owned();
    format!("find {} -name {} ! -path {} 2>/dev/null | head -n 1", shq(&dir), shq(&format!("rollout-*{id}.jsonl")), shq(path))
}

/// The command that prints the kept part of the earlier file.
pub fn codex_history_body_command(prev: &str, offset: u64) -> String {
    format!("head -c {offset} {} 2>/dev/null", shq(prev))
}

/// The permission preset from a settings event or turn context: "full-access" (no sandbox, never
/// asks), "auto-review" (a reviewer approves for you), "read-only" (no folder to write), else "default".
pub fn codex_permission(p: &Value) -> Option<String> {
    let p = Some(p);
    let t = p.at("thread_settings").or(p);
    if !t.is_object() {
        return None;
    }
    let profile = t.at("permission_profile");
    if profile.at("type").str() == Some("disabled") || t.at("sandbox_policy").at("type").str() == Some("danger-full-access") {
        return Some("full-access".into());
    }
    if t.at("approvals_reviewer").str() == Some("auto_review") {
        return Some("auto-review".into());
    }
    let entries = profile.at("file_system").at("entries");
    let read_only = match entries.arr() {
        Some(e) => !e.iter().any(|x| Some(x).at("access").str() == Some("write")),
        None => t.at("sandbox_policy").at("type").str() == Some("read-only"),
    };
    if read_only {
        return Some("read-only".into());
    }
    (profile.truthy() || t.at("sandbox_policy").truthy()).then(|| "default".into())
}

/// Codex's faster service tiers, as its settings name them; "default" is the normal speed.
const FAST_TIERS: [&str; 2] = ["fast", "priority"];

/// The command `codexStatus` runs on a rollout file.
pub fn codex_status_command(path: &str) -> String {
    format!(
        r#"stat -c %Y "${{CODEX_HOME:-$HOME/.codex}}/auth.json" 2>/dev/null || stat -f %m "${{CODEX_HOME:-$HOME/.codex}}/auth.json" 2>/dev/null || echo 0; grep -E '"type":"turn_context"|"thread_settings_applied"' {p} 2>/dev/null | tail -n 1; tail -c 400000 {p} 2>/dev/null | grep -E '"token_count"|"turn_context"|"thread_settings_applied"|"type":"compacted"|"type":"(context_)?compaction"' | tail -n 12"#,
        p = shq(path)
    )
}

fn mark_stale(status: &mut AgentStatus, limits: Option<StaleLimits>, context: Option<StaleContext>) {
    let stale = status.stale.get_or_insert(Stale { limits: None, context: None });
    if limits.is_some() {
        stale.limits = limits;
    }
    if context.is_some() {
        stale.context = context;
    }
}

/// Latest token usage and rate limits from the end of a rollout file (that command's output). A
/// compaction after the last token count makes its context size wrong, and a newer sign-in
/// (auth.json rewritten) makes its limits someone else's; both come back with the next turn.
pub fn codex_status(stdout: &str) -> (Option<AgentStatus>, Option<String>) {
    let mut lines = stdout.split('\n');
    let auth = to_number(lines.next().unwrap_or("")) * 1000.0;
    let auth_at = if auth.is_nan() { 0.0 } else { auth };
    let mut ctx = CodexContext::default();
    let mut status: Option<AgentStatus> = None;
    let mut permission: Option<String> = None;
    let mut tier: Option<String> = None;
    for line in lines {
        let Some(v) = crate::js::json::parse(line) else { continue };
        if v.is_null() {
            continue;
        }
        let e = Some(&v);
        let parsed = e.at("timestamp").str().map_or(f64::NAN, date::parse);
        let ts = if parsed.is_nan() || parsed == 0.0 { now_ms() as f64 } else { parsed };
        let ptype = e.at("payload").at("type").str();
        if e.at("type").str() == Some("turn_context") || ptype == Some("thread_settings_applied") {
            permission = e.at("payload").and_then(codex_permission).or(permission);
        }
        if ptype == Some("thread_settings_applied") {
            if let Some(t) = e.at("payload").at("thread_settings").at("service_tier").str() {
                tier = Some(t.to_owned());
            }
        }
        if e.at("type").str() == Some("turn_context") {
            ctx = CodexContext { model: string_of(e.at("payload").at("model")), effort: Some(codex_effort(e.at("payload"))) };
        } else if ptype == Some("token_count") {
            status = Some(status_from_codex(e.at("payload").unwrap_or(&Value::Null), &ctx, ts));
        } else if let Some(st) = status.as_mut().filter(|_| e.at("type").str() == Some("compacted") || ptype.is_some_and(|t| t.ends_with("compaction"))) {
            st.context_pct = None;
            st.context_used = None;
            mark_stale(st, None, Some(StaleContext::Compacted));
        }
    }
    if let Some(st) = status.as_mut() {
        st.model_id = ctx.model.clone().or(st.model_id.take());
        st.effort = ctx.effort.clone().or(st.effort.take());
        // `if (tier)`: an empty tier says nothing about speed
        if let Some(t) = tier.as_deref().filter(|t| !t.is_empty()) {
            st.fast = Some(FAST_TIERS.contains(&t));
        }
        if auth_at > st.updated_at as f64 && (st.five_hour.is_some() || st.seven_day.is_some() || st.credits.is_some()) {
            st.five_hour = None;
            st.seven_day = None;
            st.credits = None;
            st.plan = None;
            mark_stale(st, Some(StaleLimits::Account), None);
        }
    }
    (status, permission)
}

/// `Date.parse(ts) || Date.now()`
fn ms_or_now(ts: &str) -> f64 {
    let t = date::parse(ts);
    if t.is_nan() || t == 0.0 {
        now_ms() as f64
    } else {
        t
    }
}

/// The last token count described the context before compacting; it's unknown until the next one.
fn compacted(meta: &mut TranscriptMeta, ts: &str) {
    meta.stats.compactions += 1;
    let at = ms_or_now(ts) as u64;
    meta.stats.compacted_at = Some(at);
    meta.stats.context_at = Some(at);
    meta.stats.context_tokens = None;
    if let Some(st) = meta.status.as_mut() {
        st.context_pct = None;
        st.context_used = None;
        mark_stale(st, None, Some(StaleContext::Compacted));
    }
}

fn clip(s: &str) -> String {
    clip_text(s, 24_000)
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.num().filter(|x| x.is_finite())
}

fn text_of(content: Option<&Value>) -> String {
    match content.arr() {
        Some(a) => a.iter().map(|c| Some(c).at("text").or(None)).filter(|t| t.truthy()).map(|t| value_string(t.expect("truthy"))).collect::<Vec<_>>().join("\n"),
        None => content.str().unwrap_or("").to_owned(),
    }
}

fn parse_json(v: Option<&Value>) -> Option<Value> {
    match v {
        Some(Value::String(s)) => Some(crate::js::json::parse(s).unwrap_or_else(|| Value::String(s.clone()))),
        other => other.cloned(),
    }
}

/// Shell argv like ["bash","-lc","cmd"] → "cmd".
fn shell_command(cmd: Option<&Value>) -> String {
    let Some(a) = cmd.arr() else { return cmd.string_or_empty() };
    let part = |i: usize| a.get(i).map(|v| if v.is_null() { String::new() } else { value_string(v) }).unwrap_or_default();
    let words: Vec<String> = (0..a.len()).map(part).collect();
    if a.len() >= 3 && re!(r"(^|\/)(ba|z)?sh$").is_match(&part(0)) && re!(r"^-l?c$").is_match(&part(1)) {
        return words[2..].join(" ");
    }
    words.join(" ")
}

/// The JS string literal ("…", '…' or `…`) starting at byte `at` of `src`. Read in UTF-16 units
/// as the TS reads it, so a `\u` takes the four units after it whatever they are.
fn js_string(src: &str, at: usize) -> Option<String> {
    let src: Vec<u16> = src[at..].encode_utf16().collect();
    let q = *src.first()?;
    if !b"\"'`".iter().any(|&c| u16::from(c) == q) {
        return None;
    }
    let unit = |c: char| c as u16;
    let mut out: Vec<u16> = vec![];
    let mut i = 1;
    while i < src.len() {
        let c = src[i];
        if c == q {
            return Some(String::from_utf16_lossy(&out));
        }
        if c != unit('\\') {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        let n = *src.get(i)?;
        if n == unit('u') {
            let hex = String::from_utf16_lossy(&src[(i + 1).min(src.len())..(i + 5).min(src.len())]);
            let code = crate::js::num::parse_int(&hex, 16);
            out.push(if code.is_nan() { 0 } else { code as u16 });
            i += 4;
        } else {
            out.push(match n {
                x if x == unit('n') => unit('\n'),
                x if x == unit('t') => unit('\t'),
                x if x == unit('r') => unit('\r'),
                x => x,
            });
        }
        i += 1;
    }
    None
}

/// The text of the `{…}` object literal starting at byte `at` of `src`, brackets and strings balanced.
fn js_object(src: &str, at: usize) -> Option<&str> {
    if !src[at..].starts_with('{') {
        return None;
    }
    let mut depth = 0;
    let b = src.as_bytes();
    let mut i = at;
    while i < b.len() {
        let c = b[i];
        if matches!(c, b'"' | b'\'' | b'`') {
            js_string(src, i)?;
            // step over the literal: its raw length isn't the decoded length, so scan for the closing quote
            let mut j = i + 1;
            while j < b.len() && b[j] != c {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            i = j + 1;
            continue;
        }
        match c {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[at..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// A string literal, or the value of the `const x = "…"` it names.
fn string_arg(code: &str, at: usize) -> Option<String> {
    if let Some(direct) = js_string(code, at) {
        return Some(direct);
    }
    let id = re!(r"^\w+").find(&code[at..])?.as_str();
    let def = dynamic(&format!(r"(?:const|let|var)\s+{id}\s*=\s*"), "").find(code)?;
    js_string(code, def.end())
}

/// The string under `key:` in an object literal's text.
fn object_field(obj: &str, key: &str) -> Option<String> {
    let m = dynamic(&format!(r#"(?:^|[{{,\s])["']?{key}["']?\s*:\s*"#), "").find(obj)?;
    js_string(obj, m.end())
}

/// One call a Codex exec script makes: the tool and the argument that matters for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptStep {
    pub tool: String,
    /// the command, the patch, the path or the text typed, whichever the tool takes
    pub arg: Option<String>,
}

/// The fields that say what a call does, per tool, in the order to try them.
fn step_fields(tool: &str) -> &'static [&'static str] {
    match tool {
        "exec_command" => &["cmd", "command"],
        "write_stdin" => &["chars", "input"],
        "view_image" => &["path"],
        "read_file" | "write_file" => &["path", "file_path"],
        "list_dir" => &["path"],
        "grep_files" => &["pattern", "query"],
        "spawn_agent" => &["task_name", "message"],
        "send_message" | "followup_task" => &["message"],
        _ => &["cmd", "command", "path", "patch", "message", "text"],
    }
}

/// Every `tools.<name>(…)` call in a script, with the argument that describes it.
pub fn script_steps(code: &str) -> Vec<ScriptStep> {
    re!(r"tools\.(\w+)\(\s*", "g")
        .captures_iter(code)
        .map(|m| {
            let tool = m[1].to_owned();
            let at = m.get(0).expect("match").end();
            let arg = match js_object(code, at) {
                Some(obj) => step_fields(&tool).iter().find_map(|key| object_field(obj, key)),
                None => string_arg(code, at),
            };
            ScriptStep { tool, arg }
        })
        .collect()
}

/// Since 0.16x Codex runs its tools from a small JS script (the "exec" tool):
/// `const r = await tools.exec_command({cmd:"ls"}); text(r.output)` or `tools.apply_patch(p)`.
/// A script that only runs commands reads as a command, one that only applies a patch as the
/// patch; anything else reads as a script with its steps listed, so a mix of a patch and a test
/// run still says what it did. Returns the tool's name and input.
pub fn codex_script(code: &str) -> (String, Value) {
    let steps = script_steps(code);
    if !steps.is_empty() && steps.iter().all(|s| s.tool == "exec_command") {
        let cmds: Vec<&str> = steps.iter().filter_map(|s| s.arg.as_deref()).collect();
        if !cmds.is_empty() {
            return ("Bash".into(), obj([("command", Some(s(cmds.join("\n"))))]));
        }
    }
    if steps.len() == 1 && steps[0].tool == "apply_patch" {
        if let Some(arg) = steps[0].arg.as_deref().filter(|a| !a.is_empty()) {
            return ("ApplyPatch".into(), obj([("patch", Some(s(arg)))]));
        }
    }
    let steps: Vec<Value> = steps.into_iter().map(|st| obj([("tool", Some(s(st.tool))), ("arg", st.arg.map(s))])).collect();
    ("Script".into(), obj([("code", Some(s(code))), ("steps", Some(Value::Array(steps)))]))
}

struct ScriptOutput {
    text: String,
    failed: bool,
    running: Option<String>,
}

/// "Script completed\nWall time 0.2 seconds\nOutput:\n…\nexit_code=1" → the output and whether it failed.
fn script_output(raw: Option<&Value>) -> ScriptOutput {
    let text = match raw {
        Some(Value::Array(a)) => a.iter().map(|c| Some(c).at("text").present().map(value_string).unwrap_or_default()).collect::<String>(),
        Some(Value::String(s)) => s.clone(),
        _ => raw.at("output").string_or_empty(),
    };
    // a script still going (waiting on an approval, a long command): its cell is read later with wait
    let running = re!(r"^Script running with cell ID (\w+)").captures(&text).map(|c| c[1].to_owned());
    let failed = re!(r"^Script (failed|error)", "i").is_match(&text);
    let mut text = re!(r"^Script \w+(?: with cell ID \w+)?\nWall time [\d.]+ seconds\nOutput:\n").replacen(&text, 1, "").into_owned();
    let exit = re!(r"(?:^|\n)exit_code=(\d+)\s*$").captures(&text).map(|c| (c.get(0).expect("match").start(), c[1].to_owned()));
    if let Some((at, _)) = &exit {
        text.truncate(*at);
    }
    ScriptOutput { text: text.jtrim_end().to_owned(), failed: failed || exit.is_some_and(|(_, code)| code != "0"), running }
}

/// Codex titles the terminal `[spinner] [Action Required | ]<thread name> | <folder>`; keep the
/// name. Before the thread has a name it's "codex" or the folder alone, which is no name.
pub fn codex_title(title: &str, cwd: Option<&str>) -> String {
    let folder = cwd.and_then(|c| c.split('/').rfind(|s| !s.is_empty()));
    let stripped = re!(r"^[^\w\s]+\s*", "u").replacen(title, 1, "");
    let mut parts: Vec<&str> = stripped.split(" | ").map(|s| s.jtrim()).filter(|s| !s.is_empty()).collect();
    while parts.len() > 1 && re!(r"^(Action Required|Working|Thinking|Ready|Waiting|Done|Error|Interrupted)$", "i").is_match(parts[0]) {
        parts.remove(0);
    }
    if parts.len() > 1 && (Some(parts[parts.len() - 1]) == folder || parts[parts.len() - 1] == "codex") {
        parts.pop();
    }
    let name = parts.join(" | ");
    if name.is_empty() || Some(name.as_str()) == folder || re!(r"^codex$", "i").is_match(&name) {
        String::new()
    } else {
        name
    }
}

/// "@@ -2 +2,5 @@\n ctx\n+new" → hunks with their line numbers.
pub fn unified_hunks(diff: &str) -> Vec<PatchHunk> {
    let mut hunks: Vec<PatchHunk> = vec![];
    for line in diff.split('\n') {
        if let Some(h) = re!(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").captures(line) {
            let n = |i: usize, default: f64| Num(h.get(i).map_or(default, |m| to_number(m.as_str())));
            hunks.push(PatchHunk { old_start: n(1, 0.0), old_lines: n(2, 1.0), new_start: n(3, 0.0), new_lines: n(4, 1.0), lines: vec![] });
        } else if let Some(last) = hunks.last_mut().filter(|_| re!(r"^[ +-]").is_match(line)) {
            last.lines.push(line.to_owned());
        }
    }
    hunks
}

/// A null effort means Codex is using the model's default, rather than an unknown setting.
pub fn codex_effort(p: Option<&Value>) -> String {
    p.at("effort").or(p.at("reasoning_effort")).or(p.at("collaboration_mode").at("settings").at("reasoning_effort")).present().map(value_string).unwrap_or_else(|| "default".into())
}

fn file_change(item: Option<&Value>, meta: &mut TranscriptMeta) {
    if item.at("status").truthy() && item.at("status").str() != Some("completed") {
        return;
    }
    let id = item.at("id").filter(|v| Some(*v).truthy()).map(value_string);
    if let Some(id) = &id {
        if meta.codex.counted.contains(id) {
            return;
        }
        meta.codex.counted.insert(id.clone());
    }
    let mut hunks: Vec<PatchHunk> = vec![];
    let changes: Vec<&Value> = match item.at("changes") {
        Some(Value::Object(o)) => o.values().collect(),
        Some(Value::Array(a)) => a.iter().collect(),
        _ => vec![],
    };
    for c in changes {
        let c = Some(c);
        if let Some(d) = c.at("unified_diff").str() {
            hunks.extend(unified_hunks(d));
        } else if let Some(content) = c.at("content").str() {
            let body = content.strip_suffix('\n').unwrap_or(content);
            let lines: Vec<String> = body.split('\n').map(|l| format!("+{l}")).collect();
            hunks.push(PatchHunk { old_start: Num(0.0), old_lines: Num(0.0), new_start: Num(1.0), new_lines: Num(lines.len() as f64), lines });
        }
    }
    if !hunks.is_empty() {
        let count = |p: char| hunks.iter().map(|h| h.lines.iter().filter(|l| l.starts_with(p)).count()).sum::<usize>() as f64;
        meta.stats.lines_added = Some(Num(meta.stats.lines_added.map_or(0.0, |n| n.0) + count('+')));
        meta.stats.lines_removed = Some(Num(meta.stats.lines_removed.map_or(0.0, |n| n.0) + count('-')));
        meta.codex.hunks.extend(hunks);
    }
}

/// A goal set, met or cleared ("/goal …", or Codex marking it complete) → a goal row and ChatInfo.goal.
fn codex_goal(g: Option<&Value>, meta: &mut TranscriptMeta, base: &Base) -> Vec<ChatItem> {
    if !g.is_object() || !g.at("objective").truthy() {
        if meta.goal.as_ref().is_none_or(|goal| goal.met) {
            return vec![];
        }
        meta.goal = None;
        return vec![base.notice(NoticeLevel::Info, "goal", "Goal cleared")];
    }
    let condition = g.at("objective").to_js_string();
    let met = g.at("status").str() == Some("complete");
    if meta.goal.as_ref().is_some_and(|goal| goal.condition == condition && goal.met == met) {
        return vec![];
    }
    let reason = (g.at("status").str() == Some("paused")).then(|| "Paused".to_owned());
    meta.goal = Some(ChatGoal { condition: condition.clone(), met, reason, ts: base.ts.clone() });
    vec![base.goal(condition, met, None, None)]
}

/// The sub-agent's name as the TUI shows it: its path under the root agent.
fn agent_name(path: Option<&Value>) -> String {
    let p = path.present().map(value_string).unwrap_or_else(|| "Sub-agent".into());
    let name = re!(r"^\/root\/?").replacen(&p, 1, "").into_owned();
    if name.is_empty() {
        "Sub-agent".into()
    } else {
        name
    }
}

/// A sub-agent started or finished (item_completed of type SubAgentActivity).
fn sub_agent_activity(item: Option<&Value>, meta: &mut TranscriptMeta, ts: &str) {
    let Some(id) = item.at("agent_thread_id").filter(|v| Some(*v).truthy()).map(value_string) else { return };
    let kind = item.at("kind").str();
    if kind == Some("started") {
        let started_at = meta.subagents.get(&id).and_then(|c| c.started_at.clone()).unwrap_or_else(|| ts.to_owned());
        meta.subagents.insert(
            id.clone(),
            SubagentInfo {
                tool_use_id: id,
                r#type: Some("codex".into()),
                description: agent_name(item.at("agent_path")),
                prompt: None,
                status: SubagentStatus::Running,
                background: None,
                started_at: Some(started_at),
                transcript_path: None,
            },
        );
    } else if let Some(cur) = meta.subagents.get_mut(&id) {
        cur.status = match kind {
            Some("completed") => SubagentStatus::Done,
            Some("interrupted") => SubagentStatus::Error,
            _ => cur.status,
        };
    }
}

/// Messages between Codex agents ("Message Type: FINAL_ANSWER … Payload:\n…") → what was said, by whom.
fn agent_message(p: Option<&Value>, base: &Base) -> Vec<ChatItem> {
    let text = text_of(p.at("content"));
    let payload = re!(r"\nPayload:\n([\s\S]*)$").captures(&text).map_or(text.as_str(), |c| c.get(1).map_or("", |m| m.as_str())).jtrim().to_owned();
    if payload.is_empty() {
        return vec![];
    }
    vec![base.notice(NoticeLevel::Info, "task", format!("{}: {}", agent_name(p.at("author")), clip_text(&payload, 600)))]
}

/// The questions Codex asked with request_user_input, in the shape the question card shows.
fn questions(args: Option<&Value>) -> Value {
    let list = args.at("questions").arr().map(|a| a.as_slice()).unwrap_or(&[]);
    Value::Array(
        list.iter()
            .map(|q| {
                let q = Some(q);
                let options: Vec<Value> = q
                    .at("options")
                    .arr()
                    .map(|a| a.as_slice())
                    .unwrap_or(&[])
                    .iter()
                    .map(|o| match o {
                        Value::String(l) => obj([("label", Some(s(l.as_str())))]),
                        _ => obj([("label", Some(s(Some(o).at("label").string_or_empty()))), ("description", Some(o).at("description").cloned())]),
                    })
                    .collect();
                obj([
                    ("id", q.at("id").cloned()),
                    ("header", q.at("header").cloned()),
                    ("question", Some(s(q.at("question").or(q.at("title")).string_or_empty()))),
                    ("options", Some(Value::Array(options))),
                    ("isOther", q.at("isOther").or(q.at("is_other")).cloned()),
                ])
            })
            .collect(),
    )
}

/// `{ answers }`, with the text the tool result shows for them. The TS builds a plain object, so
/// a question named "2" comes before one named "1. Colour" whatever order they were answered in.
fn answers_result(answers: StringMap) -> (String, ToolResultMeta) {
    let answers = crate::js::json::js_object_order(answers);
    let text = answers.iter().map(|(q, a)| format!("{q}: {a}")).collect::<Vec<_>>().join("\n");
    (text, ToolResultMeta { answers: Some(answers), ..result_meta() })
}

/// Your answer to an async question comes back as a user message that only names the question
/// ("questionItemId": ["request_user_input_async", "<call id>", 0]); it becomes the answer on
/// that question's card instead of a bubble.
fn question_reply(text: &str, base: &Base) -> Option<Vec<ChatItem>> {
    let m = re!(r"<send_user_message_question_reply>\s*([\s\S]*?)\s*<\/send_user_message_question_reply>").captures(text)?;
    let list = parse_json(Some(&Value::String(m[1].to_owned())));
    let Some(list) = list.as_ref().and_then(Value::as_array) else { return Some(vec![]) };
    let mut by_call: indexmap::IndexMap<String, StringMap> = indexmap::IndexMap::new();
    for a in list {
        let a = Some(a);
        let item_ref = parse_json(a.at("questionItemId"));
        let Some(call_id) = item_ref.as_ref().and_then(Value::as_array).and_then(|r| r.get(1)).and_then(Value::as_str) else { continue };
        by_call.entry(call_id.to_owned()).or_default().insert(a.at("question").string_or_empty(), a.at("answer").string_or_empty());
    }
    Some(
        by_call
            .into_iter()
            .map(|(call_id, answers)| {
                let (text, meta) = answers_result(answers);
                base.with_uuid(format!("{}:{call_id}", base.uuid)).tool_result(call_id, false, text, None, Some(meta))
            })
            .collect(),
    )
}

/// Tools Codex uses for its own bookkeeping while it waits: nothing to show for them.
const SILENT_TOOLS: [&str; 4] = ["sleep", "wait", "get_goal", "update_goal"];

/// Tool arguments as they can be shown: an encrypted message says so instead of showing noise.
fn scrub(args: Option<&Value>) -> Value {
    // the multi-agent tools (namespace "collaboration") have their messages encrypted in the record
    let encrypted = |v: &Value| v.as_str().is_some_and(|s| re!(r"^gAAAA[A-Za-z0-9_-]{24,}=*$").is_match(s));
    let clean = |v: &Value| if encrypted(v) { s("[encrypted]") } else { v.clone() };
    match args {
        Some(Value::Object(o)) => Value::Object(o.iter().map(|(k, v)| (k.clone(), clean(v))).collect()),
        Some(Value::Array(a)) => Value::Object(a.iter().enumerate().map(|(i, v)| (i.to_string(), clean(v))).collect()),
        Some(v) => v.clone(),
        None => Value::Null,
    }
}

/// A script's output (or a later read of its cell) → the tool result, with the diffs Codex reported since the call.
fn script_result(call_id: &str, raw: Option<&Value>, meta: &mut TranscriptMeta, base: &Base) -> Vec<ChatItem> {
    let out = script_output(raw);
    if let Some(running) = out.running {
        meta.codex.cells.insert(running, call_id.to_owned());
        return vec![];
    }
    if out.text.starts_with("{\"goal\":") {
        let parsed = parse_json(Some(&Value::String(out.text.clone())));
        return codex_goal(parsed.as_ref().and_then(|p| p.get("goal")), meta, base);
    }
    let patch = std::mem::take(&mut meta.codex.hunks);
    let text = clip(&out.text);
    let result = if patch.is_empty() { stdout_meta(text.clone()) } else { ToolResultMeta { patch: Some(patch), ..result_meta() } };
    vec![base.tool_result(call_id, out.failed, text, None, Some(result))]
}

/// One Codex rollout line → chat items (the same shapes Claude transcripts produce).
pub fn normalize_codex(e: &Value, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
    let e = Some(e);
    let p = e.at("payload");
    if !p.truthy() {
        return vec![];
    }
    let ts = e.at("timestamp").present().map(value_string).unwrap_or_default();
    // the record's own id where it has one: the live copy of a reply (streamed before it's written
    // down) carries the same id, so the app can tell the two apart. Without one, the id comes from
    // what the record says (the first 8 hex digits of SHA-256 over the payload as JSON), so every
    // read of the file names it the same
    let uuid = match p.at("id").nonempty() {
        Some(id) => id.to_owned(),
        None => format!("{ts}:{}:{}", p.at("call_id").or(p.at("type")).to_js_string(), &sha256_hex(&stringify(p.expect("payload")))[..8]),
    };
    let base = Base::new(uuid.clone(), ts.clone());
    let etype = e.at("type").str();
    let ptype = p.at("type").str();
    if etype == Some("session_meta") {
        meta.cwd = string_of(p.at("cwd"));
        meta.version = string_of(p.at("cli_version"));
        return vec![];
    }
    if etype == Some("turn_context") {
        if p.at("model").truthy() {
            meta.model = string_of(p.at("model"));
        }
        meta.effort = Some(codex_effort(p));
        if p.at("cwd").truthy() {
            meta.cwd = string_of(p.at("cwd"));
        }
        return vec![];
    }
    if etype == Some("event_msg") {
        let item = p.at("item");
        let item_type = item.at("type").str();
        return match ptype {
            Some("token_count") => {
                let st = status_from_codex(p.expect("payload"), &CodexContext { model: meta.model.clone(), effort: meta.effort.clone() }, ms_or_now(&ts));
                meta.stats.context_tokens = st.context_used;
                meta.stats.context_at = Some(st.updated_at);
                meta.status = Some(st);
                vec![]
            }
            Some("thread_goal_updated") => codex_goal(p.at("goal"), meta, &base),
            Some("task_started") => {
                let started_at = match num(p.at("started_at")).filter(|n| *n != 0.0) {
                    Some(s) => s * 1000.0,
                    None => ms_or_now(&ts),
                };
                meta.turn = Some(Turn { id: string_of(p.at("turn_id")), started_at: Num(started_at) });
                vec![]
            }
            Some("item_completed") if item_type == Some("CommandExecution") && item.at("source").str() == Some("user_shell") => {
                let command = shell_command(item.at("command"));
                vec![base.shell_user(command), base.with_uuid(format!("{uuid}:output")).command("!", "", Some(clip(&item.at("aggregated_output").string_or_empty())))]
            }
            Some("item_completed") if item_type == Some("FileChange") => {
                file_change(item, meta);
                vec![]
            }
            Some("item_completed") if item_type == Some("SubAgentActivity") => {
                sub_agent_activity(item, meta, &ts);
                vec![]
            }
            Some("turn_aborted") => {
                meta.turn = None;
                vec![base.notice(NoticeLevel::Warn, "interrupted", "Interrupted")]
            }
            Some("task_complete") => {
                meta.stats.turns += 1;
                meta.turn = None;
                let mut out = vec![];
                let error = match p.at("error") {
                    Some(Value::String(s)) => Some(Value::String(s.clone())),
                    other => Some(other).flatten().and_then(|v| Some(v).at("message").cloned()),
                };
                if error.as_ref().truthy() {
                    out.push(base.with_uuid(format!("{uuid}:error")).notice(NoticeLevel::Error, "api_error", value_string(error.as_ref().expect("error"))));
                }
                out.push(base.turn_end(num(p.at("duration_ms")).unwrap_or(0.0)));
                out
            }
            Some("error") if p.at("message").truthy() => vec![base.notice(NoticeLevel::Error, "api_error", p.at("message").to_js_string())],
            _ => vec![],
        };
    }
    if etype == Some("compacted") {
        compacted(meta, &ts);
        return vec![base.notice(NoticeLevel::Info, "compact", "Conversation compacted")];
    }
    if etype != Some("response_item") {
        return vec![];
    }

    match ptype {
        Some("message") => {
            let text = text_of(p.at("content"));
            if text.is_blank() {
                return vec![];
            }
            match p.at("role").str() {
                Some("user") => {
                    // Codex injects environment and instruction blocks as user messages
                    if re!(r"^\s*<(environment_context|user_instructions|user_shell_command|codex_internal_context)[\s>]").is_match(&text) {
                        return vec![];
                    }
                    if let Some(reply) = question_reply(&text, &base) {
                        return reply;
                    }
                    let images = p.at("content").arr().map_or(0, |c| c.iter().filter(|x| Some(*x).at("type").str() == Some("input_image")).count()) as u32;
                    vec![base.user(clip(&text), images)]
                }
                Some("assistant") => {
                    // in Plan mode the plan is a message wrapped in a tag; it shows as a plan card
                    if let Some(plan) = re!(r"^\s*<proposed_plan>\s*([\s\S]*?)\s*<\/proposed_plan>\s*$").captures(&text) {
                        return vec![base.assistant(&uuid, meta.model.clone(), tool_block(&uuid, "ExitPlanMode", obj([("plan", Some(s(clip(&plan[1]))))])))];
                    }
                    vec![base.assistant(&uuid, meta.model.clone(), text_block(clip(&text)))]
                }
                _ => vec![],
            }
        }
        Some("agent_message") => agent_message(p, &base),
        Some("compaction" | "context_compaction") => {
            compacted(meta, &ts);
            vec![base.notice(NoticeLevel::Info, "compact", "Context compacted")]
        }
        Some("web_search_call") => {
            let q = p.at("action").at("query").or(p.at("query")).present().cloned().unwrap_or_else(|| s(""));
            vec![base.assistant(&uuid, None, tool_block(string_of(p.at("id")).unwrap_or_else(|| uuid.clone()), "WebSearch", obj([("query", Some(q))])))]
        }
        Some("image_generation_call") => {
            let prompt = p.at("revised_prompt").present().cloned().unwrap_or_else(|| s(""));
            vec![base.assistant(&uuid, None, tool_block(string_of(p.at("id")).unwrap_or_else(|| uuid.clone()), "ImageGeneration", obj([("prompt", Some(prompt))])))]
        }
        Some("reasoning") => {
            let text = p
                .at("summary")
                .arr()
                .map(|a| a.iter().map(|x| Some(x).at("text").present().map(value_string).unwrap_or_default()).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            let text = text.jtrim();
            if text.is_empty() {
                vec![]
            } else {
                vec![base.assistant(&uuid, None, thinking_block(clip(text)))]
            }
        }
        Some("function_call" | "local_shell_call") => function_call(p, meta, &base, &uuid),
        Some("custom_tool_call") => {
            let call_id = string_of(p.at("call_id"));
            if p.at("name").str() == Some("exec") {
                let input = p.at("input").string_or_empty();
                // the goal bookkeeping shows as goal rows instead (from its output)
                if re!(r"^(?:text\()?\s*(?:await\s+)?tools\.(update_goal|get_goal)\(").is_match(input.jtrim()) {
                    return vec![];
                }
                let (name, input) = codex_script(&input);
                if let Some(id) = call_id.as_ref().filter(|_| p.at("call_id").truthy()) {
                    meta.tools.insert(id.clone(), crate::claude::transcript::ToolCall { name: name.clone(), input: obj([("steps", input.get("steps").cloned())]) });
                }
                // diffs reported for an earlier script that never answered don't belong to this one
                meta.codex.hunks.clear();
                return vec![base.assistant(&uuid, None, tool_block(call_id.unwrap_or_else(|| uuid.clone()), name, input))];
            }
            let name = if p.at("name").str() == Some("apply_patch") { "ApplyPatch".to_owned() } else { p.at("name").string_or_empty() };
            vec![base.assistant(&uuid, None, tool_block(call_id.unwrap_or_else(|| uuid.clone()), name, obj([("patch", p.at("input").cloned())])))]
        }
        Some("custom_tool_call_output") if p.at("output").arr().is_some() || re!(r"^Script \w+").is_match(&p.at("output").string_or_empty()) => {
            script_result(&p.at("call_id").string_or_empty(), p.at("output"), meta, &base)
        }
        Some("custom_tool_call_output" | "function_call_output") => function_call_output(p, meta, &base),
        _ => vec![],
    }
}

fn function_call(p: Option<&Value>, meta: &mut TranscriptMeta, base: &Base, uuid: &str) -> Vec<ChatItem> {
    let parsed = parse_json(p.at("arguments"));
    let args_value = parsed.filter(|v| !v.is_null()).or_else(|| p.at("action").present().cloned()).unwrap_or_else(|| Value::Object(Map::new()));
    let args = Some(&args_value);
    let call_id = string_of(p.at("call_id")).unwrap_or_else(|| uuid.to_owned());
    let tool = |name: &str, input: Value| base.assistant(uuid, None, tool_block(call_id.clone(), name, input));
    let name = p.at("name").str();
    if name == Some("update_plan") {
        let todos: Vec<Value> = args
            .at("plan")
            .arr()
            .map(|a| a.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|st| {
                let st = Some(st);
                obj([("content", st.at("step").cloned()), ("activeForm", st.at("step").cloned()), ("status", st.at("status").cloned())])
            })
            .collect();
        return vec![tool("TodoWrite", obj([("todos", Some(Value::Array(todos)))]))];
    }
    if matches!(name, Some("request_user_input" | "request_user_input_async")) {
        let input = obj([("questions", Some(questions(args)))]);
        meta.tools.insert(call_id.clone(), crate::claude::transcript::ToolCall { name: "AskUserQuestion".into(), input: input.clone() });
        return vec![tool("AskUserQuestion", input)];
    }
    // reading a running script's cell back: its output belongs to the script
    if name == Some("wait") && args.at("cell_id").is_some() {
        if let Some(script) = meta.codex.cells.get(&args.at("cell_id").to_js_string()).cloned() {
            meta.codex.waits.insert(call_id.clone(), script);
        }
        meta.tools.insert(call_id, crate::claude::transcript::ToolCall { name: "wait".into(), input: Value::Object(Map::new()) });
        return vec![];
    }
    if let Some(n) = name.filter(|n| SILENT_TOOLS.contains(n)) {
        meta.tools.insert(call_id, crate::claude::transcript::ToolCall { name: n.to_owned(), input: Value::Object(Map::new()) });
        return vec![];
    }
    if matches!(name, Some("shell" | "exec_command")) || p.at("type").str() == Some("local_shell_call") || args.at("command").truthy() || args.at("cmd").truthy() {
        let command = shell_command(args.at("command").or(args.at("cmd")));
        return vec![tool("Bash", obj([("command", Some(s(command))), ("workdir", args.at("workdir").cloned())]))];
    }
    if name == Some("apply_patch") {
        let patch = args.at("input").or(args.at("patch")).present().cloned().unwrap_or_else(|| s(""));
        return vec![tool("ApplyPatch", obj([("patch", Some(patch))]))];
    }
    vec![tool(&p.at("name").present().map(value_string).unwrap_or_else(|| "tool".into()), scrub(args))]
}

fn function_call_output(p: Option<&Value>, meta: &mut TranscriptMeta, base: &Base) -> Vec<ChatItem> {
    let call_id = p.at("call_id").string_or_empty();
    let call = meta.tools.get(&call_id).cloned();
    if call.as_ref().is_some_and(|c| c.name == "wait") {
        // the cell's output, read back later: it answers the script, if this wait was for one
        return match meta.codex.waits.remove(&call_id) {
            Some(script) => script_result(&script, p.at("output"), meta, base),
            None => vec![],
        };
    }
    if call.as_ref().is_some_and(|c| SILENT_TOOLS.contains(&c.name.as_str())) {
        return vec![];
    }
    let out = parse_json(p.at("output"));
    let out = out.as_ref();
    if let Some(call) = call.as_ref().filter(|c| c.name == "AskUserQuestion") {
        // the async form only acknowledges here; the answer comes as a message later
        if out.is_object() && !out.at("answers").truthy() {
            return vec![];
        }
        let mut answers = StringMap::new();
        let asked = Some(&call.input).at("questions").arr().map(|a| a.as_slice()).unwrap_or(&[]);
        if let Some(given) = out.at("answers").obj() {
            // Object.entries: integer-like ids first
            for (id, a) in crate::js::json::ordered(given) {
                let q = asked.iter().find(|x| Some(*x).at("id").str() == Some(id.as_str()));
                let key = q.and_then(|q| Some(q).at("question").present()).map(value_string).unwrap_or_else(|| id.clone());
                let list: Vec<String> = match Some(a).at("answers").arr() {
                    Some(l) => l.iter().map(value_string).collect(),
                    None => vec![value_string(a)],
                };
                answers.insert(key, list.join(", "));
            }
        }
        let (text, meta) = answers_result(answers);
        return vec![base.tool_result(call_id, false, text, None, Some(meta))];
    }
    let text = match out {
        Some(Value::String(s)) => s.clone(),
        _ => match out.at("output").present() {
            Some(o) => value_string(o),
            None if out.is_object() => stringify(out.expect("object")),
            None => String::new(),
        },
    };
    let exit = out.at("metadata").at("exit_code").num();
    let text = clip(&text);
    vec![base.tool_result(call_id, exit.is_some_and(|x| x != 0.0), text.clone(), None, exit.map(|_| stdout_meta(text)))]
}

// ---- run on a machine: the async halves of the TS functions above ----

/// `guessCodexRollout(host, home, proc, others)`
pub async fn find_codex_rollout(host: &Host, home: &str, proc: &CodexProc, others: &[CodexProc]) -> HubResult<Option<String>> {
    let cmd = guess_codex_rollout_command(home, proc, others, now_ms() as f64 / 1000.0);
    let r = host.run(&cmd).await?;
    let (rows, names) = codex_rollouts(&r.stdout);
    Ok(pick_codex_rollout(&rows, proc, others, &names))
}

/// `codexHistory(host, path)`: the earlier lines to replay before a segment, following bases of
/// bases, oldest first.
pub fn codex_history<'a>(host: &'a Host, path: &'a str, depth: u32) -> futures_util::future::LocalBoxFuture<'a, HubResult<Vec<String>>> {
    Box::pin(async move {
        if depth > 6 {
            return Ok(vec![]);
        }
        let head = host.run(&codex_history_head_command(path)).await?;
        let Some((id, offset)) = codex_history_base(&head.stdout) else { return Ok(vec![]) };
        let r = host.run(&codex_history_find_command(path, &id)).await?;
        let prev = r.stdout.jtrim().to_owned();
        if prev.is_empty() {
            return Ok(vec![]);
        }
        let body = host.exec(&codex_history_body_command(&prev, offset), crate::hosts::host::ExecOptions::timeout(30000)).await?;
        let mut out = codex_history(host, &prev, depth + 1).await?;
        out.extend(body.stdout.split('\n').filter(|l| !l.is_empty()).map(str::to_owned));
        Ok(out)
    })
}

/// What `codexStatus` reads from a rollout file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexStatusRead {
    pub status: Option<AgentStatus>,
    pub permission: Option<String>,
}

/// `codexStatus(host, path)`
pub async fn read_codex_status(host: &Host, path: &str) -> HubResult<CodexStatusRead> {
    let r = host.run(&codex_status_command(path)).await?;
    let (status, permission) = codex_status(&r.stdout);
    Ok(CodexStatusRead { status, permission })
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
