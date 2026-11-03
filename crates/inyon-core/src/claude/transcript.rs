//! Port of claude/transcript.ts: one Claude Code transcript line → chat items, with the running
//! summary of the session (`TranscriptMeta`) kept up to date. The other agents' readers build the
//! same items and fill the same summary, so the builders they share live here too.
//!
//! The TS returns `Draft`s, chat items without their `seq`; these return `ChatItem`s with `seq` 0,
//! which the caller numbers as the TS caller does.

use std::collections::{HashMap, HashSet, VecDeque};

use indexmap::IndexMap;
use inyon_protocol::{
    AgentStatus, ArtifactInfo, AssistantBlock, BackgroundTask, ChatGoal, ChatItem, NoticeLevel, Num, PatchHunk, RemoteControl, SessionStats, StringMap, SubagentInfo,
    SubagentStatus, TaskStatus, TaskType, ToolResultMeta, Turn,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::ids::random_uuid;
use crate::js::json::{value_string, Js};
use crate::js::{date, JsStr};
use crate::re;

pub const MAX_TEXT: usize = 24_000;
const MAX_TOOL_INPUT_STRING: usize = 60_000;

/// Claude saves a long paste wrapped in <pasted_content> tags; the message is what's inside.
pub fn unwrap_paste(s: &str) -> String {
    re!(r"<\/?pasted_content\b[^>]*>", "g").replace_all(s, "").jtrim().to_owned()
}

/// `clip(s, max)`: the text cut to `max` UTF-16 units, and whether it was.
pub fn clip(s: &str, max: usize) -> (String, bool) {
    if s.len16() > max {
        (s.to16(max as isize).to_owned(), true)
    } else {
        (s.to_owned(), false)
    }
}

/// The text of `clip`.
pub fn clip_text(s: &str, max: usize) -> String {
    clip(s, max).0
}

/// A tool call the summary remembers, so its result can be read when it comes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
}

/// Bookkeeping the Codex reader keeps per conversation, outside the summary (the TS keeps it in a
/// WeakMap keyed by the summary, so it's never saved either).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexScratch {
    /// diffs Codex reported since the last script output (FileChange items land between a call and its output)
    pub hunks: Vec<PatchHunk>,
    /// file changes already counted into the session's line totals
    pub counted: HashSet<String>,
    /// scripts still running, by the cell id Codex reads them back with
    pub cells: HashMap<String, String>,
    /// `wait` calls, by call id, and the script call each one reads
    pub waits: HashMap<String, String>,
}

/// Running summary of a session, built while reading its transcript. As JSON it is `SavedMeta`:
/// the maps become lists of [key, value] pairs, and only the latest 200 tool calls are kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatus>,
    pub stats: SessionStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<ChatGoal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rc: Option<RemoteControl>,
    /// the turn under way, from the agent's own record (Codex: task_started)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<Turn>,
    /// the conversation's public link while it's shared (OpenCode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<String>,
    #[serde(default)]
    pub queue: Vec<String>,
    #[serde(default, with = "pairs")]
    pub tasks: IndexMap<String, BackgroundTask>,
    #[serde(default, with = "pairs")]
    pub subagents: IndexMap<String, SubagentInfo>,
    #[serde(default, with = "pairs")]
    pub artifacts: IndexMap<String, ArtifactInfo>,
    #[serde(default)]
    pub tools: ToolCalls,
    #[serde(skip)]
    pub codex: CodexScratch,
}

/// A map as the list of its entries, `[...map]` in TS. Keys made by `map_key` (undefined, null)
/// are written as JSON writes those values.
mod pairs {
    use super::*;
    use crate::js::json::{key_of, key_value};

    pub fn serialize<V: Serialize, S: Serializer>(m: &IndexMap<String, V>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter().map(|(k, v)| (key_value(k), v)))
    }

    pub fn deserialize<'de, V: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<IndexMap<String, V>, D::Error> {
        let v: Vec<(Value, V)> = Vec::deserialize(d)?;
        Ok(v.into_iter().map(|(k, v)| (key_of(&k), v)).collect())
    }
}

/// The tool calls a session made, oldest first, as the TS's `Map`: setting a call again keeps its
/// place, and dropping the oldest takes constant time however many there are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCalls {
    calls: HashMap<String, ToolCall>,
    order: VecDeque<String>,
}

impl ToolCalls {
    pub fn insert(&mut self, id: String, call: ToolCall) {
        if let Some(cur) = self.calls.get_mut(&id) {
            *cur = call;
            return;
        }
        self.order.push_back(id.clone());
        self.calls.insert(id, call);
    }
    pub fn get(&self, id: &str) -> Option<&ToolCall> {
        self.calls.get(id)
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// Drops the call made first.
    pub fn remove_oldest(&mut self) {
        if let Some(id) = self.order.pop_front() {
            self.calls.remove(&id);
        }
    }
    /// The calls, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&String, &ToolCall)> + ExactSizeIterator {
        self.order.iter().map(|id| (id, &self.calls[id]))
    }
}

// a tool's result follows its call closely, so only the latest calls can still be answered
impl Serialize for ToolCalls {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.iter().skip(self.len().saturating_sub(200)).map(|(k, v)| (crate::js::json::key_value(k), v)))
    }
}

impl<'de> Deserialize<'de> for ToolCalls {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut out = ToolCalls::default();
        for (k, v) in Vec::<(Value, ToolCall)>::deserialize(d)? {
            out.insert(crate::js::json::key_of(&k), v);
        }
        Ok(out)
    }
}

pub fn new_stats() -> SessionStats {
    SessionStats {
        context_tokens: None,
        cost_usd: None,
        duration_ms: None,
        api_duration_ms: None,
        lines_added: None,
        lines_removed: None,
        turns: 0,
        compactions: 0,
        compacted_at: None,
        context_at: None,
        started_at: None,
    }
}

pub fn new_meta() -> TranscriptMeta {
    TranscriptMeta {
        title: None,
        custom_title: None,
        mode: None,
        model: None,
        effort: None,
        cwd: None,
        git_branch: None,
        version: None,
        status: None,
        stats: new_stats(),
        goal: None,
        rc: None,
        turn: None,
        share: None,
        queue: vec![],
        tasks: IndexMap::new(),
        subagents: IndexMap::new(),
        artifacts: IndexMap::new(),
        tools: ToolCalls::default(),
        codex: CodexScratch::default(),
    }
}

impl Default for TranscriptMeta {
    fn default() -> Self {
        new_meta()
    }
}

/// A summary as JSON, to save with a conversation's cached copy (the recent tool calls only).
pub fn save_meta(m: &TranscriptMeta) -> Value {
    serde_json::to_value(m).unwrap_or(Value::Null)
}

/// The summary back from what `save_meta` wrote.
pub fn load_meta(s: &Value) -> Option<TranscriptMeta> {
    serde_json::from_value(s.clone()).ok()
}

// ---- building chat items (shared by every agent's reader) ----

/// The id and time every item of one record shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub uuid: String,
    pub ts: String,
}

impl Base {
    pub fn new(uuid: impl Into<String>, ts: impl Into<String>) -> Self {
        Base { uuid: uuid.into(), ts: ts.into() }
    }
    /// the same record under another id (`{ ...base, uuid }`)
    pub fn with_uuid(&self, uuid: impl Into<String>) -> Self {
        Base { uuid: uuid.into(), ts: self.ts.clone() }
    }
    pub fn user(&self, text: impl Into<String>, images: u32) -> ChatItem {
        ChatItem::User { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), text: text.into(), images, queued: None, shell: None }
    }
    pub fn queued_user(&self, text: impl Into<String>) -> ChatItem {
        ChatItem::User { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), text: text.into(), images: 0, queued: Some(true), shell: None }
    }
    pub fn shell_user(&self, text: impl Into<String>) -> ChatItem {
        ChatItem::User { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), text: text.into(), images: 0, queued: None, shell: Some(true) }
    }
    pub fn assistant(&self, msg_id: impl Into<String>, model: Option<String>, block: AssistantBlock) -> ChatItem {
        ChatItem::Assistant { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), msg_id: msg_id.into(), model, block }
    }
    pub fn tool_result(&self, tool_use_id: impl Into<String>, is_error: bool, text: impl Into<String>, truncated: Option<bool>, meta: Option<ToolResultMeta>) -> ChatItem {
        ChatItem::ToolResult { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), tool_use_id: tool_use_id.into(), is_error, text: text.into(), truncated, meta }
    }
    pub fn command(&self, name: impl Into<String>, args: impl Into<String>, output: Option<String>) -> ChatItem {
        ChatItem::Command { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), name: name.into(), args: args.into(), output }
    }
    pub fn notice(&self, level: NoticeLevel, subtype: &str, text: impl Into<String>) -> ChatItem {
        ChatItem::Notice { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), level, subtype: subtype.into(), text: text.into() }
    }
    pub fn turn_end(&self, duration_ms: f64) -> ChatItem {
        ChatItem::TurnEnd { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), duration_ms: Num(duration_ms) }
    }
    pub fn goal(&self, condition: impl Into<String>, met: bool, reason: Option<String>, cleared: Option<bool>) -> ChatItem {
        ChatItem::Goal { seq: 0, uuid: self.uuid.clone(), ts: self.ts.clone(), condition: condition.into(), met, reason, cleared }
    }
}

pub fn text_block(text: impl Into<String>) -> AssistantBlock {
    AssistantBlock::Text { text: text.into() }
}
pub fn thinking_block(text: impl Into<String>) -> AssistantBlock {
    AssistantBlock::Thinking { text: text.into() }
}
pub fn tool_block(id: impl Into<String>, name: impl Into<String>, input: Value) -> AssistantBlock {
    AssistantBlock::ToolUse { id: id.into(), name: name.into(), input }
}

/// An empty ToolResultMeta, to fill in.
pub fn result_meta() -> ToolResultMeta {
    ToolResultMeta { patch: None, file_path: None, created: None, stdout: None, stderr: None, interrupted: None, answers: None, plan: None, num_lines: None, task_id: None }
}

/// `{ stdout }`
pub fn stdout_meta(stdout: impl Into<String>) -> ToolResultMeta {
    ToolResultMeta { stdout: Some(stdout.into()), ..result_meta() }
}

/// Numbers the items as the TS does at the end of a whole-document reader (`seq` = position).
pub fn numbered(items: Vec<ChatItem>) -> Vec<ChatItem> {
    items.into_iter().enumerate().map(|(i, it)| with_seq(it, i as i64)).collect()
}

/// The item with another `seq`.
pub fn with_seq(mut it: ChatItem, n: i64) -> ChatItem {
    match &mut it {
        ChatItem::User { seq, .. }
        | ChatItem::Assistant { seq, .. }
        | ChatItem::ToolResult { seq, .. }
        | ChatItem::Command { seq, .. }
        | ChatItem::Notice { seq, .. }
        | ChatItem::TurnEnd { seq, .. }
        | ChatItem::Goal { seq, .. } => *seq = n,
    }
    it
}

/// `String(v)` for a value that is there, None for undefined and null (`v ?? undefined`, as a
/// string field of the wire types).
pub fn string_of(v: Option<&Value>) -> Option<String> {
    v.present().map(value_string)
}

/// `Date.parse(s) || undefined` as unix ms
fn parsed_ms(s: &str) -> Option<u64> {
    let t = date::parse(s);
    (!t.is_nan() && t != 0.0).then_some(t as u64)
}

// ---- Claude Code's lines ----

/// Strips huge strings from tool inputs (e.g. Write content) while keeping structure.
fn trim_input(v: &Value, depth: usize) -> Value {
    match v {
        Value::String(s) => {
            if s.len16() > MAX_TOOL_INPUT_STRING {
                Value::String(format!("{}\n…", s.to16(MAX_TOOL_INPUT_STRING as isize)))
            } else {
                v.clone()
            }
        }
        _ if depth > 6 => v.clone(),
        Value::Array(a) => Value::Array(a.iter().take(200).map(|x| trim_input(x, depth + 1)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), trim_input(x, depth + 1))).collect()),
        _ => v.clone(),
    }
}

fn block_text(content: Option<&Value>) -> String {
    if let Some(s) = content.str() {
        return s.to_owned();
    }
    let Some(arr) = content.arr() else { return String::new() };
    arr.iter()
        .map(|b| {
            let b = Some(b);
            match b.at("type").str() {
                Some("text") => b.at("text").cloned(),
                Some("image") => Some(Value::String("[image]".into())),
                _ => None,
            }
        })
        .filter(|v| v.as_ref().truthy())
        .map(|v| value_string(&v.expect("truthy")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What's between the first `<name>` and the next `</name>`.
pub fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    Some(&s[start..end])
}

/// `kTokens(n)`: the count as "5k", "?" when there is none, and anything else as `String(n)`
/// prints it (a null `postTokens` reads "null", as in the TS).
fn k_tokens(n: Option<&Value>) -> String {
    match n {
        None => "?".into(),
        Some(v) if crate::js::json::value_number(v) >= 1000.0 => format!("{}k", crate::js::fmt_number(crate::js::js_round(crate::js::json::value_number(v) / 1000.0))),
        Some(v) => value_string(v),
    }
}

fn num_of(v: Option<&Value>) -> Num {
    Num(v.num().unwrap_or(0.0))
}

fn result_meta_of(tur: Option<&Value>) -> Option<ToolResultMeta> {
    tur.obj()?;
    let mut meta = result_meta();
    if let Some(p) = tur.at("structuredPatch").arr().filter(|p| !p.is_empty()) {
        meta.patch = Some(
            p.iter()
                .take(60)
                .map(|h| {
                    let h = Some(h);
                    PatchHunk {
                        old_start: num_of(h.at("oldStart")),
                        old_lines: num_of(h.at("oldLines")),
                        new_start: num_of(h.at("newStart")),
                        new_lines: num_of(h.at("newLines")),
                        lines: h.at("lines").arr().map(|l| l.iter().take(400).map(value_string).collect()).unwrap_or_default(),
                    }
                })
                .collect(),
        );
    }
    if let Some(f) = tur.at("filePath").str() {
        meta.file_path = Some(f.to_owned());
    }
    match tur.at("type").str() {
        Some("create") => meta.created = Some(true),
        Some("update") => meta.created = Some(false),
        _ => {}
    }
    if let Some(s) = tur.at("stdout").str() {
        meta.stdout = Some(clip_text(s, 8000));
    }
    if let Some(s) = tur.at("stderr").nonempty() {
        meta.stderr = Some(clip_text(s, 4000));
    }
    if tur.at("interrupted").truthy() {
        meta.interrupted = Some(true);
    }
    if let Some(a) = tur.at("answers").obj() {
        meta.answers = Some(a.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect::<StringMap>());
    }
    if let Some(p) = tur.at("plan").str() {
        meta.plan = Some(p.to_owned());
    }
    if tur.at("file").truthy() {
        if let Some(n) = tur.at("file").at("numLines").num() {
            meta.num_lines = Some(n as u64);
        }
    }
    if let Some(t) = tur.at("backgroundTaskId").str() {
        meta.task_id = Some(t.to_owned());
    } else if let Some(t) = tur.at("taskId").str() {
        meta.task_id = Some(t.to_owned());
    }
    (meta != result_meta()).then_some(meta)
}

fn ended_status(status: &str, failed: bool) -> TaskStatus {
    if failed {
        TaskStatus::Failed
    } else if status.contains("stop") {
        TaskStatus::Stopped
    } else {
        TaskStatus::Completed
    }
}

/// `<task-notification>` blocks report background tasks finishing. The same notification shows up
/// in several transcript lines, so a notice comes back only the first time a task finishes.
fn task_notification(meta: &mut TranscriptMeta, text: &str, base: &Base) -> Vec<ChatItem> {
    let ts = &base.ts;
    let Some(id) = tag(text, "task-id").filter(|s| !s.is_empty()) else { return vec![] };
    let status = tag(text, "status").unwrap_or("completed");
    let failed = re!(r"fail|error|killed").is_match(status);
    let level = if failed { NoticeLevel::Warn } else { NoticeLevel::Info };
    let notice = |msg: String| vec![base.notice(level, "task", msg)];
    let tool_use_id = tag(text, "tool-use-id").unwrap_or("");
    if let Some(sub) = meta.subagents.get_mut(tool_use_id) {
        let was = sub.status;
        sub.status = if failed { SubagentStatus::Error } else { SubagentStatus::Done };
        let description = sub.description.clone();
        // a background subagent is also listed as a task from its start; it ends here too
        if let Some(task) = meta.tasks.get_mut(id) {
            if task.status == TaskStatus::Running {
                task.status = ended_status(status, failed);
                task.ended_at = Some(ts.clone());
            }
        }
        return if was == SubagentStatus::Running {
            notice(tag(text, "summary").map(str::to_owned).unwrap_or_else(|| format!("Subagent \"{description}\" {}", if failed { "failed" } else { "finished" })))
        } else {
            vec![]
        };
    }
    let cur = meta.tasks.get(id).cloned();
    let fresh = cur.as_ref().is_none_or(|c| c.status == TaskStatus::Running);
    meta.tasks.insert(
        id.to_owned(),
        BackgroundTask {
            id: id.to_owned(),
            r#type: cur.as_ref().map_or(TaskType::Shell, |c| c.r#type),
            description: cur.as_ref().map(|c| c.description.clone()).or_else(|| tag(text, "summary").map(str::to_owned)).unwrap_or_else(|| id.to_owned()),
            command: cur.as_ref().and_then(|c| c.command.clone()),
            output_file: tag(text, "output-file").map(str::to_owned).or_else(|| cur.as_ref().and_then(|c| c.output_file.clone())),
            status: ended_status(status, failed),
            summary: tag(text, "summary").map(str::to_owned),
            started_at: cur.as_ref().and_then(|c| c.started_at.clone()),
            ended_at: Some(ts.clone()),
        },
    );
    if !fresh {
        return vec![];
    }
    let what = match cur.as_ref().map(|c| c.r#type) {
        Some(TaskType::Monitor) => "Monitor",
        Some(TaskType::Workflow) => "Workflow",
        _ => "Background shell",
    };
    let description = cur.as_ref().map_or(id, |c| c.description.as_str());
    let how = if failed {
        "failed"
    } else if status.contains("stop") {
        "stopped"
    } else {
        "finished"
    };
    notice(tag(text, "summary").map(str::to_owned).unwrap_or_else(|| format!("{what} \"{description}\" {how}")))
}

/// Converts one Claude Code transcript line into zero or more chat items, and keeps the running
/// session summary in `meta` up to date. Bookkeeping lines produce no items.
pub fn normalize(e: &Value, meta: &mut TranscriptMeta, sidechain: bool) -> Vec<ChatItem> {
    if !e.is_object() {
        return vec![];
    }
    let e = Some(e);
    let base = Base { uuid: string_of(e.at("uuid")).unwrap_or_else(random_uuid), ts: string_of(e.at("timestamp")).unwrap_or_default() };
    if e.at("cwd").truthy() {
        meta.cwd = string_of(e.at("cwd"));
    }
    if e.at("gitBranch").truthy() {
        meta.git_branch = string_of(e.at("gitBranch"));
    }
    if e.at("version").truthy() {
        meta.version = string_of(e.at("version"));
    }
    match e.at("type").str() {
        Some("ai-title") => {
            meta.title = string_of(e.at("aiTitle"));
            vec![]
        }
        Some("custom-title") => {
            meta.custom_title = string_of(e.at("customTitle"));
            vec![]
        }
        Some("permission-mode") => {
            meta.mode = string_of(e.at("permissionMode"));
            vec![]
        }
        Some("cost-state") => {
            let n = |k: &str| e.at(k).num().map(Num);
            meta.stats.cost_usd = n("totalCostUSD");
            meta.stats.duration_ms = n("totalDuration");
            meta.stats.api_duration_ms = n("totalAPIDuration");
            meta.stats.lines_added = n("totalLinesAdded");
            meta.stats.lines_removed = n("totalLinesRemoved");
            if let Some(st) = e.at("startTime").filter(|v| Some(*v).truthy()) {
                // an invalid date keeps the start there was
                if let Some(iso) = date::to_iso(date::time_of(st)) {
                    meta.stats.started_at = Some(iso);
                }
            }
            vec![]
        }
        Some("queue-operation") => {
            let c = e.at("content").string_or_empty();
            let op = e.at("operation").str();
            if c.starts_with("<task-notification>") {
                return if op == Some("enqueue") { task_notification(meta, &c, &base) } else { vec![] };
            }
            if op == Some("enqueue") {
                meta.queue.push(unwrap_paste(&c));
            } else if op == Some("popAll") {
                // pressing Esc/↑ pulls every queued message back into the input box at once
                meta.queue.clear();
            } else {
                // a message from another session is logged with different attributes on its
                // <cross-session-message …> tag when it's queued and when it's taken: compare what's inside
                let key = queue_key(&unwrap_paste(&c));
                if let Some(i) = meta.queue.iter().position(|q| queue_key(q) == key) {
                    meta.queue.remove(i);
                } else if op == Some("dequeue") && !meta.queue.is_empty() {
                    meta.queue.remove(0);
                }
            }
            vec![]
        }
        Some("attachment") => attachment_items(e, &base, meta),
        Some("system") => system_item(e, &base, meta),
        Some("assistant") => {
            if e.at("isSidechain").truthy() && !sidechain {
                return vec![];
            }
            let msg = e.at("message");
            let model = string_of(msg.at("model"));
            if msg.at("model").truthy() && model.as_deref() != Some("<synthetic>") {
                meta.model = model.clone();
            }
            let u = msg.at("usage");
            if let Some(input) = u.at("input_tokens").num() {
                let n = |k: &str| u.at(k).present().num().unwrap_or(0.0);
                meta.stats.context_tokens = Some(Num(input + n("cache_creation_input_tokens") + n("cache_read_input_tokens") + n("output_tokens")));
                meta.stats.context_at = parsed_ms(&base.ts);
            }
            let msg_id = string_of(msg.at("id")).unwrap_or_else(|| base.uuid.clone());
            let mut out = vec![];
            for b in msg.at("content").arr().into_iter().flatten() {
                let b = Some(b);
                let at = |n: usize| base.with_uuid(format!("{}:{n}", base.uuid));
                match b.at("type").str() {
                    Some("text") if b.at("text").str().is_some_and(|t| !t.is_blank()) => {
                        out.push(at(out.len()).assistant(&msg_id, model.clone(), text_block(clip_text(b.at("text").str().unwrap_or(""), MAX_TEXT))));
                    }
                    Some("thinking") if b.at("thinking").str().is_some_and(|t| !t.is_blank()) => {
                        out.push(at(out.len()).assistant(&msg_id, model.clone(), thinking_block(clip_text(b.at("thinking").str().unwrap_or(""), MAX_TEXT))));
                    }
                    Some("tool_use") => {
                        track_tool_use(meta, b, &base.ts);
                        let input = b.at("input").map(|i| trim_input(i, 0)).unwrap_or(Value::Null);
                        out.push(at(out.len()).assistant(&msg_id, model.clone(), tool_block(b.at("id").string_or_empty(), b.at("name").string_or_empty(), input)));
                    }
                    _ => {}
                }
            }
            out
        }
        Some("user") => user_items(e, &base, meta, sidechain),
        _ => vec![],
    }
}

fn track_tool_use(meta: &mut TranscriptMeta, b: Option<&Value>, ts: &str) {
    let input = b.at("input");
    let prompt = input.at("prompt").str().map(|p| Value::String(p.to16(4000).to_owned()));
    // the TS keys its maps by `b.id` as it is; an Agent call without one is under undefined
    let key = crate::js::json::map_key(b.at("id"));
    let id = b.at("id").string_or_empty();
    let name = b.at("name").string_or_empty();
    let kept = crate::js::json::obj([
        ("description", input.at("description").cloned()),
        ("command", input.at("command").cloned()),
        ("run_in_background", input.at("run_in_background").cloned()),
        ("task_id", input.at("task_id").cloned()),
        ("subagent_type", input.at("subagent_type").cloned()),
        ("prompt", prompt.clone()),
        ("name", input.at("name").cloned()),
    ]);
    meta.tools.insert(key.clone(), ToolCall { name: name.clone(), input: kept });
    if meta.tools.len() > 4000 {
        meta.tools.remove_oldest();
    }
    if name == "Agent" || name == "Task" {
        let description = input.at("description").or(input.at("subagent_type")).string_or("Subagent");
        meta.subagents.insert(
            key,
            SubagentInfo {
                tool_use_id: id,
                r#type: string_of(input.at("subagent_type")),
                description,
                prompt: prompt.and_then(|p| p.as_str().map(str::to_owned)),
                status: SubagentStatus::Running,
                background: Some(input.at("run_in_background").truthy()),
                started_at: Some(ts.to_owned()),
                transcript_path: None,
            },
        );
    }
}

fn track_tool_result(meta: &mut TranscriptMeta, tool_use_id: &str, is_error: bool, tur: Option<&Value>, ts: &str) {
    // a command that runs past its timeout moves to the background without asking to, so a long
    // conversation's summary may not hold the call; a background task ID alone says it was Bash
    let use_ = match meta.tools.get(tool_use_id) {
        Some(u) => u.clone(),
        None if tur.at("backgroundTaskId").str().is_some() => ToolCall { name: "Bash".into(), input: Value::Object(Map::new()) },
        None => return,
    };
    if let Some(sub) = meta.subagents.get_mut(tool_use_id) {
        // background subagents return immediately; they finish later via a task notification
        if !(sub.background == Some(true) && tur.at("isAsync").truthy()) {
            sub.status = if is_error { SubagentStatus::Error } else { SubagentStatus::Done };
        }
    }
    let input = Some(&use_.input);
    let task_id = tur.at("backgroundTaskId").or(tur.at("taskId"));
    if task_id.truthy() && matches!(use_.name.as_str(), "Bash" | "Monitor" | "Workflow") {
        let task_id = task_id.to_js_string();
        meta.tasks.insert(
            task_id.clone(),
            BackgroundTask {
                id: task_id.clone(),
                r#type: match use_.name.as_str() {
                    "Bash" => TaskType::Shell,
                    "Monitor" => TaskType::Monitor,
                    _ => TaskType::Workflow,
                },
                description: input.at("description").or(input.at("name")).present().map_or(task_id, value_string),
                command: input.at("command").str().map(str::to_owned),
                status: TaskStatus::Running,
                output_file: None,
                summary: None,
                started_at: Some(ts.to_owned()),
                ended_at: None,
            },
        );
    }
    if matches!(use_.name.as_str(), "TaskStop" | "KillShell") && input.at("task_id").truthy() {
        if let Some(t) = meta.tasks.get_mut(&input.at("task_id").to_js_string()) {
            t.status = TaskStatus::Stopped;
            t.ended_at = Some(ts.to_owned());
        }
    }
    if use_.name == "Artifact" {
        if let Some(url) = tur.at("url").str() {
            let title = string_of(tur.at("title")).or_else(|| meta.artifacts.get(url).and_then(|a| a.title.clone()));
            meta.artifacts.insert(url.to_owned(), ArtifactInfo { url: url.to_owned(), title, updated_at: Some(ts.to_owned()) });
        }
    }
}

fn queue_key(text: &str) -> String {
    re!(r"^<cross-session-message\b[^>]*>").replacen(text, 1, "<cross-session-message>").into_owned()
}

fn attachment_items(e: Option<&Value>, base: &Base, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
    let a = e.at("attachment");
    if !a.truthy() {
        return vec![];
    }
    match a.at("type").str() {
        Some("queued_command") => {
            let Some(prompt) = a.at("prompt").str() else { return vec![] };
            if prompt.starts_with("<task-notification>") {
                return task_notification(meta, prompt, base);
            }
            if a.at("humanTurn").truthy() {
                return vec![base.queued_user(unwrap_paste(prompt))];
            }
            vec![]
        }
        Some("goal_status") => {
            let prev = meta.goal.clone();
            let met = a.at("met").truthy();
            // "/goal clear" is recorded as a met goal with the sentinel flag: the goal is gone, not achieved
            if a.at("sentinel").truthy() && met {
                meta.goal = None;
                return match prev {
                    Some(p) if !p.met => vec![base.goal(p.condition, false, None, Some(true))],
                    _ => vec![],
                };
            }
            let goal = ChatGoal { condition: a.at("condition").string_or_empty(), met, reason: string_of(a.at("reason")), ts: base.ts.clone() };
            let changed = prev.as_ref().is_none_or(|p| p.condition != goal.condition || p.met != goal.met);
            meta.goal = Some(goal.clone());
            if changed {
                return vec![base.goal(goal.condition, goal.met, goal.reason, None)];
            }
            vec![]
        }
        Some("task_status") => {
            let Some(task_id) = a.at("taskId").filter(|v| Some(*v).truthy()).map(value_string) else { return vec![] };
            let cur = meta.tasks.get(&task_id).cloned();
            let status = a.at("status").present().map(value_string).unwrap_or_else(|| "running".into());
            let task_type = a.at("taskType").str().unwrap_or("");
            let running = status == "running";
            meta.tasks.insert(
                task_id.clone(),
                BackgroundTask {
                    id: task_id.clone(),
                    r#type: if task_type == "local_bash" {
                        TaskType::Shell
                    } else if task_type.contains("monitor") {
                        TaskType::Monitor
                    } else if task_type.contains("workflow") {
                        TaskType::Workflow
                    } else {
                        cur.as_ref().map_or(TaskType::Other, |c| c.r#type)
                    },
                    description: string_of(a.at("description")).or_else(|| cur.as_ref().map(|c| c.description.clone())).unwrap_or_else(|| task_id.clone()),
                    command: string_of(a.at("shell").at("command")).or_else(|| cur.as_ref().and_then(|c| c.command.clone())),
                    output_file: string_of(a.at("outputFilePath")).or_else(|| cur.as_ref().and_then(|c| c.output_file.clone())),
                    status: if running {
                        TaskStatus::Running
                    } else if re!(r"fail|error").is_match(&status) {
                        TaskStatus::Failed
                    } else if re!(r"kill|stop").is_match(&status) {
                        TaskStatus::Stopped
                    } else {
                        TaskStatus::Completed
                    },
                    summary: None,
                    started_at: cur.as_ref().and_then(|c| c.started_at.clone()).or_else(|| Some(base.ts.clone())),
                    ended_at: if running { None } else { Some(base.ts.clone()) },
                },
            );
            vec![]
        }
        _ => vec![],
    }
}

fn system_item(e: Option<&Value>, base: &Base, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
    match e.at("subtype").str() {
        Some("turn_duration") => {
            meta.stats.turns += 1;
            vec![base.turn_end(e.at("durationMs").present().num().unwrap_or(0.0))]
        }
        Some("compact_boundary") => {
            meta.stats.compactions += 1;
            let m = e.at("compactMetadata");
            // what's left after compacting is the context now, until the next reply reports usage
            meta.stats.compacted_at = parsed_ms(&base.ts);
            meta.stats.context_at = meta.stats.compacted_at;
            let post = m.at("postTokens").num();
            meta.stats.context_tokens = post.map(Num);
            let how = if m.at("trigger").str() == Some("auto") { "Auto-compacted" } else { "Compacted" };
            let text = if m.at("preTokens").truthy() {
                format!("{how} · {} → {} tokens", k_tokens(m.at("preTokens")), k_tokens(m.at("postTokens")))
            } else {
                "Conversation compacted".into()
            };
            vec![base.notice(NoticeLevel::Info, "compact", text)]
        }
        Some("bridge_status") => {
            if let Some(url) = e.at("url").str() {
                meta.rc = Some(RemoteControl { url: url.to_owned(), ts: base.ts.clone() });
            }
            vec![base.notice(NoticeLevel::Info, "rc", string_of(e.at("content")).unwrap_or_else(|| "Remote Control is active".into()))]
        }
        Some("away_summary") => {
            let text = e.at("content").string_or_empty();
            vec![base.notice(NoticeLevel::Info, "recap", re!(r"\s*\(disable recaps.*\)$").replacen(&text, 1, "").into_owned())]
        }
        Some("api_error") => {
            let text = [e.at("content"), e.at("error").at("message")].into_iter().find(|v| v.truthy()).flatten().map(value_string).unwrap_or_else(|| "API error".into());
            vec![base.notice(NoticeLevel::Error, "api_error", text)]
        }
        Some("local_command") => {
            let c = e.at("content").string_or_empty();
            if let Some(name) = tag(&c, "command-name").filter(|n| !n.is_empty()) {
                return vec![base.command(name, tag(&c, "command-args").unwrap_or(""), None)];
            }
            if let Some(out) = tag(&c, "local-command-stdout").or_else(|| tag(&c, "local-command-stderr")) {
                if re!(r"Remote Control disconnected", "i").is_match(out) {
                    meta.rc = None;
                }
                return vec![base.command("", "", Some(out.to_owned()))];
            }
            vec![]
        }
        Some("informational") => match e.at("level").str() {
            Some(l @ ("warning" | "error")) => vec![base.notice(if l == "error" { NoticeLevel::Error } else { NoticeLevel::Warn }, "info", e.at("content").string_or_empty())],
            _ => vec![],
        },
        _ => vec![],
    }
}

fn user_items(e: Option<&Value>, base: &Base, meta: &mut TranscriptMeta, sidechain: bool) -> Vec<ChatItem> {
    if e.at("isSidechain").truthy() && !sidechain {
        return vec![];
    }
    let content = e.at("message").at("content");
    if e.at("isCompactSummary").truthy() {
        return vec![];
    }

    // tool results
    if let Some(blocks) = content.arr().filter(|c| c.iter().any(|b| Some(b).at("type").str() == Some("tool_result"))) {
        let mut out = vec![];
        for b in blocks {
            let b = Some(b);
            if b.at("type").str() != Some("tool_result") {
                continue;
            }
            // tool output is shown collapsed; 8k chars is plenty and keeps first loads light on phones
            let (text, truncated) = clip(&block_text(b.at("content")), 8000);
            let id = b.at("tool_use_id").string_or_empty();
            let is_error = b.at("is_error").truthy();
            track_tool_result(meta, &crate::js::json::map_key(b.at("tool_use_id")), is_error, e.at("toolUseResult"), &base.ts);
            out.push(base.with_uuid(format!("{}:{}", base.uuid, out.len())).tool_result(id, is_error, text, Some(truncated), result_meta_of(e.at("toolUseResult"))));
        }
        return out;
    }

    let text = block_text(content);
    if text.contains("<task-notification>") {
        return task_notification(meta, &text, base);
    }
    if e.at("isMeta").truthy() {
        return vec![];
    }
    let images = content.arr().map_or(0, |c| c.iter().filter(|b| Some(*b).at("type").str() == Some("image")).count()) as u32;

    // slash commands and their output
    if let Some(cmd) = tag(&text, "command-name").filter(|c| !c.is_empty()) {
        return vec![base.command(cmd, tag(&text, "command-args").unwrap_or(""), None)];
    }
    if let Some(stdout) = tag(&text, "local-command-stdout") {
        if re!(r"Remote Control disconnected", "i").is_match(stdout) {
            meta.rc = None;
        }
        return vec![base.command("", "", Some(stdout.to_owned()))];
    }
    if text.contains("<local-command-caveat>") {
        return vec![];
    }

    // "!" shell mode
    if let Some(bash_in) = tag(&text, "bash-input") {
        return vec![base.shell_user(bash_in)];
    }
    if let Some(bash_out) = tag(&text, "bash-stdout") {
        let err = tag(&text, "bash-stderr");
        let output = [Some(bash_out), err].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
        return vec![base.command("!", "", Some(output))];
    }

    if text.starts_with("[Request interrupted by user") {
        return vec![base.notice(NoticeLevel::Warn, "interrupted", "Interrupted")];
    }
    if text.is_blank() && images == 0 {
        return vec![];
    }
    vec![base.user(clip_text(&unwrap_paste(&text), MAX_TEXT), images)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_match;
    use serde_json::json;

    /// queued messages leave the queue when Claude pulls them all back into the input box
    #[test]
    fn queued_messages_leave_the_queue_when_claude_pulls_them_all_back_into_the_input_box() {
        let mut meta = new_meta();
        let mut op = |operation: &str, content: Option<&str>| {
            normalize(&json!({ "type": "queue-operation", "operation": operation, "content": content, "timestamp": "2026-10-10T10:00:00Z" }), &mut meta, false)
        };
        op("enqueue", Some("first"));
        op("enqueue", Some("second"));
        op("popAll", Some("first\nsecond"));
        drop(op);
        assert!(meta.queue.is_empty());
        let mut op = |operation: &str, content: Option<&str>| {
            normalize(&json!({ "type": "queue-operation", "operation": operation, "content": content, "timestamp": "2026-10-10T10:00:00Z" }), &mut meta, false)
        };
        op("enqueue", Some("first"));
        op("remove", Some("first"));
        assert!(meta.queue.is_empty());
    }

    /// a cleared goal is gone, not met; a met goal stays as the last one
    #[test]
    fn a_cleared_goal_is_gone_not_met_a_met_goal_stays_as_the_last_one() {
        let mut meta = new_meta();
        let mut n = 0;
        let mut goal = |meta: &mut TranscriptMeta, a: Value| {
            let mut att = json!({ "type": "goal_status" });
            att.as_object_mut().unwrap().extend(a.as_object().unwrap().clone());
            n += 1;
            normalize(&json!({ "type": "attachment", "uuid": format!("u{n}"), "timestamp": "2026-10-11T02:40:00Z", "attachment": att }), meta, false)
        };
        // what Claude Code 2.1 writes for "/goal …", a failed check, then "/goal clear"
        assert_match(&goal(&mut meta, json!({ "met": false, "sentinel": true, "condition": "tests pass" })), json!([{ "kind": "goal", "met": false, "condition": "tests pass" }]));
        assert_eq!(goal(&mut meta, json!({ "met": false, "condition": "tests pass", "reason": "two still fail" })), vec![]);
        assert_match(&meta.goal, json!({ "condition": "tests pass", "met": false }));
        assert_match(&goal(&mut meta, json!({ "met": true, "sentinel": true, "condition": "tests pass" })), json!([{ "kind": "goal", "cleared": true, "met": false }]));
        assert_eq!(meta.goal, None);
        // clearing when nothing is set adds nothing
        assert_eq!(goal(&mut meta, json!({ "met": true, "sentinel": true, "condition": "tests pass" })), vec![]);
        goal(&mut meta, json!({ "met": false, "sentinel": true, "condition": "build is green" }));
        assert_match(&goal(&mut meta, json!({ "met": true, "condition": "build is green", "reason": "it is" })), json!([{ "kind": "goal", "met": true }]));
        assert_match(&meta.goal, json!({ "condition": "build is green", "met": true }));
    }

    /// a message from another session leaves the queue once Claude takes it, though its tag changed
    #[test]
    fn a_message_from_another_session_leaves_the_queue_once_claude_takes_it_though_its_tag_changed() {
        let mut meta = new_meta();
        normalize(
            &json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-12T04:36:35Z", "content": "<cross-session-message from=\"uds:/run/user/1001/cc-socks/49259.sock\" hop-chain=\"d6c7\" from-name=\"custom-tmux-de\" from-mode=\"bypass\">\nAnswer: yes\n</cross-session-message>" }),
            &mut meta,
            false,
        );
        assert_eq!(meta.queue.len(), 1);
        normalize(
            &json!({ "type": "queue-operation", "operation": "remove", "timestamp": "2026-10-12T04:36:35Z", "reason": "absorbed_mid_turn", "content": "<cross-session-message from=\"uds:/run/user/1001/cc-socks/49259.sock\" from-name=\"custom-tmux-de\" from-mode=\"bypass\" hop-chain=\"d6c7\">\nAnswer: yes\n</cross-session-message>" }),
            &mut meta,
            false,
        );
        assert!(meta.queue.is_empty());
    }

    /// a background subagent leaves the running tasks when it finishes
    #[test]
    fn a_background_subagent_leaves_the_running_tasks_when_it_finishes() {
        let mut meta = new_meta();
        let mut n = 0;
        let mut line = |meta: &mut TranscriptMeta, e: Value| {
            let mut v = json!({ "uuid": format!("u{n}"), "timestamp": "2026-10-10T10:00:00Z" });
            n += 1;
            v.as_object_mut().unwrap().extend(e.as_object().unwrap().clone());
            normalize(&v, meta, false)
        };
        line(
            &mut meta,
            json!({ "type": "assistant", "message": { "role": "assistant", "content": [{ "type": "tool_use", "id": "toolu_1", "name": "Agent", "input": { "description": "Research Windows support", "prompt": "look", "run_in_background": true } }] } }),
        );
        // what Claude Code writes while the agent works, then when it's done
        line(
            &mut meta,
            json!({ "type": "attachment", "attachment": { "type": "task_status", "taskId": "a624", "taskType": "local_agent", "description": "Research Windows support", "status": "running" } }),
        );
        assert_eq!(meta.tasks["a624"].status, TaskStatus::Running);
        line(
            &mut meta,
            json!({ "type": "attachment", "attachment": { "type": "queued_command", "prompt": "<task-notification>\n<task-id>a624</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Research Windows support\" completed</summary>\n</task-notification>" } }),
        );
        assert_eq!(meta.tasks["a624"].status, TaskStatus::Completed);
        assert_eq!(meta.subagents["toolu_1"].status, SubagentStatus::Done);
    }

    #[test]
    fn a_saved_summary_loads_back_the_same() {
        let mut meta = new_meta();
        meta.title = Some("t".into());
        normalize(
            &json!({ "type": "assistant", "uuid": "a", "timestamp": "2026-10-10T10:00:00Z", "message": { "model": "opus", "content": [{ "type": "tool_use", "id": "x", "name": "Agent", "input": { "description": "d" } }] } }),
            &mut meta,
            false,
        );
        let saved = save_meta(&meta);
        assert_eq!(saved["subagents"][0][0], "x");
        assert_eq!(saved["tools"][0][1]["input"], json!({ "description": "d" }));
        assert_eq!(load_meta(&saved).unwrap(), meta);
    }

    /// review: a null description falls through to the default, as `??` does in the TS
    #[test]
    fn a_null_description_falls_through_to_the_default() {
        let mut meta = new_meta();
        let line = |meta: &mut TranscriptMeta, e: Value| {
            let mut v = json!({ "uuid": "u", "timestamp": "2026-10-10T10:00:00Z" });
            v.as_object_mut().unwrap().extend(e.as_object().unwrap().clone());
            normalize(&v, meta, false)
        };
        let tool = |id: &str, name: &str, input: Value| json!({ "type": "assistant", "message": { "role": "assistant", "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } });
        line(&mut meta, tool("t1", "Agent", json!({ "description": null, "subagent_type": null, "prompt": "go" })));
        assert_eq!(meta.subagents["t1"].description, "Subagent");
        line(&mut meta, tool("t2", "Bash", json!({ "command": "make", "description": null, "name": null })));
        line(
            &mut meta,
            json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t2", "content": "ok" }] }, "toolUseResult": { "backgroundTaskId": "b7" } }),
        );
        assert_eq!(meta.tasks["b7"].description, "b7");
    }

    /// review: past 4000 calls the oldest goes in constant time, in the TS's order
    #[test]
    fn the_oldest_tool_call_goes_in_constant_time_and_the_order_is_the_tss() {
        let call = |n: &str| ToolCall { name: n.into(), input: json!({}) };
        let mut tools = ToolCalls::default();
        let start = std::time::Instant::now();
        // shifting an IndexMap of 4000 a million times moved four billion entries
        for i in 0..1_000_000 {
            tools.insert(format!("t{i}"), call("Bash"));
            if tools.len() > 4000 {
                tools.remove_oldest();
            }
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "{:?}", start.elapsed());
        assert_eq!(tools.len(), 4000);
        assert_eq!(tools.iter().next().unwrap().0, "t996000");
        // setting a call again keeps its place, as Map.set does
        tools.insert("t996000".into(), call("Read"));
        assert_eq!(tools.iter().next().unwrap().1.name, "Read");
        assert_eq!(tools.iter().last().unwrap().0, "t999999");
        let saved = save_meta(&TranscriptMeta { tools, ..new_meta() });
        let pairs = saved["tools"].as_array().unwrap();
        assert_eq!((pairs.len(), pairs[0][0].as_str()), (200, Some("t999800")));
        assert_eq!(load_meta(&saved).unwrap().tools.iter().next().unwrap().0, "t999800");
    }

    /// review: "5k → null tokens", as String(null) prints it
    #[test]
    fn a_null_token_count_after_compacting_prints_as_the_ts_prints_it() {
        let mut meta = new_meta();
        let line = |m: Value| json!({ "type": "system", "subtype": "compact_boundary", "uuid": "c", "timestamp": "2026-10-10T10:00:00Z", "compactMetadata": m });
        assert_match(
            &normalize(&line(json!({ "trigger": "manual", "preTokens": 5000, "postTokens": null })), &mut meta, false),
            json!([{ "text": "Compacted · 5k → null tokens" }]),
        );
        assert_match(&normalize(&line(json!({ "trigger": "auto", "preTokens": "12000" })), &mut meta, false), json!([{ "text": "Auto-compacted · 12k → ? tokens" }]));
    }

    /// review: an Agent call without an id is under undefined, which a notification without a
    /// tool-use-id ('') doesn't find
    #[test]
    fn an_agent_call_without_an_id_is_not_found_by_a_notification_without_one() {
        let mut meta = new_meta();
        normalize(
            &json!({ "type": "assistant", "uuid": "a", "timestamp": "2026-10-10T10:00:00Z", "message": { "role": "assistant", "content": [{ "type": "tool_use", "name": "Agent", "input": { "description": "look" } }] } }),
            &mut meta,
            false,
        );
        let out = normalize(
            &json!({ "type": "user", "uuid": "n", "timestamp": "2026-10-10T10:01:00Z", "message": { "role": "user", "content": "<task-notification><task-id>x1</task-id><status>completed</status></task-notification>" } }),
            &mut meta,
            false,
        );
        assert_match(&out, json!([{ "kind": "notice", "text": "Background shell \"x1\" finished" }]));
        assert_eq!(meta.subagents.values().next().unwrap().status, SubagentStatus::Running);
        // saved as [...map] writes it: the key undefined as null
        let saved = save_meta(&meta);
        assert_eq!(saved["subagents"][0][0], Value::Null);
        assert_eq!(saved["tools"][0][0], Value::Null);
        assert_eq!(load_meta(&saved).unwrap().subagents.len(), 1);
    }

    /// an invalid startTime keeps the start there was
    #[test]
    fn an_invalid_start_time_keeps_the_start_there_was() {
        let mut meta = new_meta();
        let cost = |meta: &mut TranscriptMeta, start: Value| {
            normalize(&json!({ "type": "cost-state", "totalCostUSD": 0.5, "startTime": start, "timestamp": "2026-10-19T10:00:00Z" }), meta, false)
        };
        cost(&mut meta, json!("2026-10-19T09:00:00Z"));
        assert_eq!(meta.stats.started_at.as_deref(), Some("2026-10-19T09:00:00.000Z"));
        cost(&mut meta, json!("not a date"));
        cost(&mut meta, json!(1e20));
        assert_eq!(meta.stats.started_at.as_deref(), Some("2026-10-19T09:00:00.000Z"));
        assert_eq!(meta.stats.cost_usd, Some(inyon_protocol::Num(0.5)));
    }
}
