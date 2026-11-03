//! Port of agents/antigravity.ts: Antigravity CLI's (`agy`) transcript into chat items, and its
//! status line payload into the agent's status.
//!
//! agy keeps each conversation under
//! `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`: one line per
//! step, with a step_index. A model step (PLANNER_RESPONSE) carries its text, thinking and tool
//! calls (each argument value JSON-encoded on its own); the steps that follow hold the tools'
//! results, in order.

use std::collections::{BTreeMap, HashMap, VecDeque};

use indexmap::IndexMap;
use inyon_protocol::{AgentStatus, BackgroundTask, ChatGoal, ChatItem, NoticeLevel, Num, RateWindow, TaskStatus, TaskType, ToolResultMeta};
use serde::Serialize;
use serde_json::{Map, Value};

use super::status::empty_status;
use crate::claude::transcript::{numbered, result_meta, stdout_meta, string_of, text_block, thinking_block, tool_block, Base, ToolCall, TranscriptMeta};
use crate::js::json::{self, obj, s, value_string, Js};
use crate::js::{date, js_round, JsStr};
use crate::re;
use crate::tmux::tmux::AgyStatusPane;

pub fn agy_transcript(home: &str, id: &str) -> String {
    format!("{home}/.gemini/antigravity-cli/brain/{id}/.system_generated/logs/transcript.jsonl")
}

fn decode(v: &Value) -> Value {
    match v {
        Value::String(s) => json::parse(s).unwrap_or_else(|| v.clone()),
        _ => v.clone(),
    }
}

/// Antigravity tool names → the names Inyon's tool rows know.
pub fn map_agy_tool(name: &str, raw: &Map<String, Value>) -> ToolCall {
    let a: Map<String, Value> = raw.iter().map(|(k, v)| (k.clone(), decode(v))).collect();
    let a = &a;
    let get = |k: &str| a.get(k).cloned();
    let first = |ks: &[&str]| ks.iter().find_map(|k| a.get(*k).filter(|v| !v.is_null()).cloned());
    let description = first(&["toolSummary", "Description"]);
    let call = |name: &str, input: Value| ToolCall { name: name.into(), input };
    match name {
        "run_command" => call("Bash", obj([("command", get("CommandLine")), ("description", description), ("cwd", get("Cwd"))])),
        "view_file" => call("Read", obj([("file_path", first(&["AbsolutePath", "TargetFile"]))])),
        "write_to_file" => call("Write", obj([("file_path", get("TargetFile")), ("content", get("CodeContent").filter(Value::is_string))])),
        "replace_file_content" => call(
            "Edit",
            obj([
                ("file_path", first(&["TargetFile", "AbsolutePath"])),
                ("old_string", get("TargetContent")),
                ("new_string", get("ReplacementContent")),
                ("description", description),
            ]),
        ),
        "multi_replace_file_content" => {
            let chunks = a.get("ReplacementChunks").and_then(Value::as_array).cloned().unwrap_or_default();
            let edits = chunks.iter().map(|c| obj([("old_string", Some(c).at("TargetContent").cloned()), ("new_string", Some(c).at("ReplacementContent").cloned())])).collect();
            call("MultiEdit", obj([("file_path", first(&["TargetFile", "AbsolutePath"])), ("edits", Some(Value::Array(edits)))]))
        }
        "grep_search" => call("Grep", obj([("pattern", first(&["Query", "SearchPattern", "Pattern"])), ("path", first(&["SearchPath", "SearchDirectory"]))])),
        "find_by_name" => call("Glob", obj([("pattern", first(&["Pattern", "SearchPattern", "Query"])), ("path", first(&["SearchDirectory", "SearchPath"]))])),
        "list_dir" => call("LS", obj([("path", first(&["DirectoryPath", "AbsolutePath"]))])),
        "search_web" => call("WebSearch", obj([("query", first(&["query", "Query"]))])),
        "read_url_content" => call("WebFetch", obj([("url", first(&["Url", "url"]))])),
        "invoke_subagent" | "browser_subagent" => {
            let description = description.or_else(|| first(&["Name"])).unwrap_or_else(|| s("Subagent"));
            call("Task", obj([("description", Some(description)), ("prompt", first(&["Prompt", "Task", "Instruction"]))]))
        }
        _ => {
            let mut rest: Map<String, Value> = a.iter().filter(|(k, _)| *k != "toolAction" && *k != "toolSummary").map(|(k, v)| (k.clone(), v.clone())).collect();
            if let Some(d) = description.filter(|d| Some(d).truthy()) {
                rest.insert("description".into(), d);
            }
            call(name, Value::Object(rest))
        }
    }
}

/// `content.replace(/<([A-Z_]+)>[\s\S]*?<\/\1>/g, '')`, by hand (the pattern has a back reference).
fn drop_tags(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut at = 0;
    while at < content.len() {
        let rest = &content[at..];
        let Some(m) = re!(r"<([A-Z_]+)>").captures(rest) else { break };
        let whole = m.get(0).expect("match");
        let close = format!("</{}>", &m[1]);
        match rest[whole.end()..].find(&close) {
            Some(end) => {
                out.push_str(&rest[..whole.start()]);
                at += whole.end() + end + close.len();
            }
            None => {
                // no closing tag for this one: it stays, and the search goes on after its "<"
                let skip = whole.start() + 1;
                out.push_str(&rest[..skip]);
                at += skip;
            }
        }
    }
    out.push_str(&content[at.min(content.len())..]);
    out
}

/// The text you typed, without the request wrapper and the metadata agy appends.
pub fn user_text(content: &str) -> String {
    if let Some(m) = re!(r"<USER_REQUEST>\n?([\s\S]*?)\n?<\/USER_REQUEST>").captures(content) {
        return m[1].jtrim().to_owned();
    }
    drop_tags(content).jtrim().to_owned()
}

struct ResultText {
    text: String,
    stdout: Option<String>,
    error: bool,
}

/// Tool output without the "Created At / Completed At" header agy puts first.
fn result_text(content: &str) -> ResultText {
    let text = re!(r"^(?:(?:Created|Completed) At: [^\n]*\n)+\n?").replace(content, "").jtrim_end().to_owned();
    let code = re!(r"exited with code (-?\d+)").captures(&text).map(|c| c[1].to_owned());
    let out = re!(r"\nOutput:\n([\s\S]*)$").captures(&text).or_else(|| re!(r"^Output:\n([\s\S]*)$").captures(&text));
    ResultText {
        stdout: out.map(|o| o[1].replace("\r\n", "\n").jtrim_end().to_owned()),
        error: code.is_some_and(|c| c != "0") || re!(r"^(error|failed)\b", "i").is_match(&text),
        text,
    }
}

const GOAL_DONE: &str = "<!-- GOAL_COMPLETE -->";

/// Holds an Antigravity conversation in memory; later lines for the same step replace earlier ones.
#[derive(Debug, Clone, Default)]
pub struct AntigravityDoc {
    steps: BTreeMap<StepIndex, Value>,
}

/// A `step_index` as the TS's `Map<number, …>` keys it: any number, 3.5 apart from 3, -0 the same
/// as 0, and in numeric order once sorted.
#[derive(Debug, Clone, Copy)]
struct StepIndex(f64);

impl PartialEq for StepIndex {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for StepIndex {}
impl PartialOrd for StepIndex {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for StepIndex {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // + 0.0 turns -0 into 0
        (self.0 + 0.0).total_cmp(&(other.0 + 0.0))
    }
}

impl std::fmt::Display for StepIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::js::fmt_number(self.0))
    }
}

impl AntigravityDoc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, rec: &Value) {
        if let Some(i) = Some(rec).at("step_index").num() {
            self.steps.insert(StepIndex(i), rec.clone());
        }
    }

    pub fn items(&self, meta: &mut TranscriptMeta) -> Vec<ChatItem> {
        let mut out: Vec<ChatItem> = vec![];
        let mut pending: VecDeque<(String, String)> = VecDeque::new();
        meta.stats.turns = 0;
        meta.stats.started_at = None;
        meta.goal = None;
        meta.tasks.clear();
        let mut calls: HashMap<String, ToolCall> = HashMap::new();
        for (idx, step) in &self.steps {
            let s = Some(step);
            let content = s.at("content").str().unwrap_or("");
            let base = Base::new(format!("s{idx}"), string_of(s.at("created_at")).unwrap_or_default());
            let timestamp = date::parse(&base.ts);
            if timestamp.is_finite() && meta.stats.started_at.as_deref().is_none_or(|st| st.is_empty() || timestamp < date::parse(st)) {
                meta.stats.started_at = date::to_iso(timestamp);
            }
            let kind = s.at("type").str();
            if kind == Some("USER_INPUT") {
                if let Some(m) = re!(r"`Model Selection` from .*? to (.+?)\. ").captures(content) {
                    meta.model = Some(m[1].to_owned());
                }
                let text = user_text(content);
                // "/goal …" keeps agy working until it writes the GOAL_COMPLETE marker
                if let Some(g) = re!(r"^\/goal\s+([\s\S]+)$").captures(&text) {
                    let condition = g[1].jtrim().to_owned();
                    meta.goal = Some(ChatGoal { condition: condition.clone(), met: false, reason: None, ts: base.ts.clone() });
                    out.push(base.goal(condition, false, None, None));
                } else if !text.is_empty() {
                    out.push(base.user(text, 0));
                }
                pending.clear();
                continue;
            }
            if kind == Some("PLANNER_RESPONSE") {
                let msg_id = format!("s{idx}");
                if let Some(t) = s.at("thinking").str().map(JsStr::jtrim).filter(|t| !t.is_empty()) {
                    out.push(base.with_uuid(format!("{msg_id}:t")).assistant(&msg_id, meta.model.clone(), thinking_block(t)));
                }
                let done = content.contains(GOAL_DONE);
                let text = content.replace(GOAL_DONE, "");
                let text = text.jtrim();
                if !text.is_empty() {
                    out.push(base.with_uuid(format!("{msg_id}:x")).assistant(&msg_id, meta.model.clone(), text_block(text)));
                }
                if let Some(goal) = meta.goal.as_mut().filter(|g| done && !g.met) {
                    goal.met = true;
                    goal.ts = base.ts.clone();
                    out.push(base.with_uuid(format!("{msg_id}:g")).goal(goal.condition.clone(), true, None, None));
                }
                let tool_calls = s.at("tool_calls").arr().map(Vec::as_slice).unwrap_or_default();
                for (n, tc) in tool_calls.iter().enumerate() {
                    let tc = Some(tc);
                    let id = format!("{msg_id}:{n}");
                    let name = tc.at("name").present().map_or_else(|| "tool".into(), value_string);
                    let mapped = map_agy_tool(&name, tc.at("args").obj().unwrap_or(&Map::new()));
                    out.push(base.with_uuid(&id).assistant(&msg_id, meta.model.clone(), tool_block(&id, &mapped.name, mapped.input.clone())));
                    pending.push_back((id.clone(), mapped.name.clone()));
                    calls.insert(id, mapped);
                }
                if let Some(input) = s.at("input_tokens").num() {
                    // the whole prompt: agy counts the cached part separately from input_tokens
                    meta.stats.context_tokens = Some(Num(input + s.at("cache_read_tokens").num().unwrap_or(0.0)));
                    let at = date::parse(s.at("created_at").str().unwrap_or(""));
                    if !at.is_nan() {
                        meta.stats.context_at = Some(at as u64);
                    }
                }
                let has_calls = match s.at("tool_calls") {
                    Some(Value::Array(a)) => !a.is_empty(),
                    Some(Value::String(t)) => !t.is_empty(),
                    _ => false,
                };
                if !content.jtrim().is_empty() && !has_calls {
                    meta.stats.turns += 1;
                }
                continue;
            }
            // notices agy injects for the model (restarts, reminders) are not tool output, but one
            // may say a background task finished
            if s.at("source").str() == Some("SYSTEM") || kind == Some("SYSTEM_MESSAGE") {
                let done = re!(r#"Task id "([^"]+)" finished with result:\s*([\s\S]*?)\s*(?:Log: \S+\s*)?<\/SYSTEM_MESSAGE>"#).captures(content);
                if let Some(done) = done {
                    if let Some(t) = meta.tasks.get_mut(&done[1]).filter(|t| t.status == TaskStatus::Running) {
                        let code = re!(r"exited with code (-?\d+)").captures(&done[2]).map(|c| c[1].to_owned());
                        let failed = code.is_some_and(|c| !c.is_empty() && c != "0");
                        t.status = if failed { TaskStatus::Failed } else { TaskStatus::Completed };
                        t.summary = Some(done[2].replace('\r', "").jtrim().slice16(0, Some(400)).to_owned());
                        t.ended_at = Some(base.ts.clone());
                        let level = if failed { NoticeLevel::Warn } else { NoticeLevel::Info };
                        out.push(base.notice(level, "task", format!("Background shell \"{}\" {}", t.description, if failed { "failed" } else { "finished" })));
                    }
                }
                continue;
            }
            // anything else answers the oldest tool call still waiting
            let call = pending.pop_front();
            match call {
                Some((id, name)) if !content.is_empty() => {
                    let r = result_text(content);
                    // a command that outlives its wait becomes a background task with a log file
                    if let Some(bg) = re!(r"running as a background task with task id: (\S+)").captures(content) {
                        let task = bg[1].to_owned();
                        let input = calls.get(&id).map(|c| c.input.clone()).unwrap_or(Value::Null);
                        let input = Some(&input);
                        let description = input
                            .at("description")
                            .present()
                            .map(value_string)
                            .or_else(|| re!(r"Task Description: (.*)").captures(content).map(|c| c[1].to_owned()))
                            .unwrap_or_else(|| task.clone());
                        meta.tasks.insert(
                            task.clone(),
                            BackgroundTask {
                                id: task.clone(),
                                r#type: TaskType::Shell,
                                description,
                                command: input.at("command").str().map(str::to_owned),
                                output_file: re!(r"Task logs are available at: file:\/\/(\S+)").captures(content).map(|c| c[1].to_owned()),
                                status: TaskStatus::Running,
                                summary: None,
                                started_at: Some(base.ts.clone()),
                                ended_at: None,
                            },
                        );
                        let short = task.rsplit('/').next().unwrap_or("");
                        let meta = ToolResultMeta { task_id: Some(task.clone()), ..result_meta() };
                        out.push(base.tool_result(&id, false, format!("Running in the background ({short})"), None, Some(meta)));
                        continue;
                    }
                    let is_error = r.error || s.at("status").str() == Some("ERROR");
                    let meta = r.stdout.as_deref().filter(|_| name == "Bash").map(|o| stdout_meta(o.slice16(0, Some(8000))));
                    out.push(base.tool_result(&id, is_error, r.text.slice16(0, Some(8000)), None, meta));
                }
                Some(_) => {}
                None => {
                    if re!(r"error", "i").is_match(&s.at("type").to_js_string()) && !content.jtrim().is_empty() {
                        out.push(base.notice(NoticeLevel::Error, "info", content.jtrim().slice16(0, Some(2000))));
                    }
                }
            }
        }
        numbered(out)
    }
}

fn unix(v: Option<&Value>) -> Option<Num> {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|x| x.is_finite()).map(|x| Num(if x > 1e12 { js_round(x / 1000.0) } else { x })),
        Some(Value::String(t)) => {
            let t = date::parse(t);
            (!t.is_nan()).then(|| Num(js_round(t / 1000.0)))
        }
        _ => None,
    }
}

fn bucket(q: Option<&Value>) -> Option<RateWindow> {
    let f = q.at("remaining_fraction").num()?;
    Some(RateWindow { pct: Num(((1.0 - f) * 100.0).clamp(0.0, 100.0)), resets_at: unix(q.at("reset_time")) })
}

/// "Gemini 3.8 Flash (High)" → "Gemini 3.8 Flash"
pub fn agy_model_name(display: Option<&str>) -> Option<String> {
    display.map(|d| re!(r"\s*\((?:low|medium|high|minimal|xhigh|max)\)\s*$", "i").replace(d, "").jtrim().to_owned())
}

pub fn agy_mode(data: &Value) -> Option<String> {
    let d = Some(data);
    let mode = d.at("cycle_mode").or(d.at("execution_mode")).str()?;
    Some(mode.to_lowercase().replace('_', "-"))
}

/// What `agyStatus` returns: the agent's status and what else agy's status line says.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgyStatus {
    #[serde(flatten)]
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<Num>,
    pub confirming: bool,
}

/// Live numbers from agy's status line payload (saved per pane by Inyon's wrapper). Ignored when
/// it was written before this process started or names another conversation. `integration` is
/// Some(false) when Inyon's status line is missing from agy's settings.
pub fn agy_status(statuses: &IndexMap<String, AgyStatusPane>, integration: Option<bool>, pane: &str, started_at: f64, conversation: Option<&str>) -> Option<AgyStatus> {
    let st = statuses.get(pane);
    if integration == Some(false) {
        return None;
    }
    let st = st.filter(|st| st.ts >= started_at - 5.0)?;
    let empty = Value::Object(Map::new());
    let data = Some(&st.data).present().unwrap_or(&empty);
    let d = Some(data);
    // an empty conversation id checks nothing, as `conversation &&` doesn't in the TS
    if let (Some(c), true) = (conversation.filter(|c| !c.is_empty()), d.at("conversation_id").truthy()) {
        if d.at("conversation_id").and_then(Value::as_str) != Some(c) {
            return None;
        }
    }
    let cw = d.at("context_window");
    let display = d.at("model").at("display_name").str();
    // Claude and GPT models draw on the "3p" quota; Gemini models on their own
    let group = if re!(r"claude|gpt", "i").is_match(display.unwrap_or("")) { "3p" } else { "gemini" };
    let size = cw.at("context_window_size").num().filter(|x| *x > 0.0);
    let used = cw.at("total_input_tokens").num().filter(|x| *x > 0.0);
    let finite = |v: Option<&Value>| v.num().filter(|x| x.is_finite()).map(Num);
    let quota = d.at("quota");
    let status = AgentStatus {
        model: agy_model_name(display),
        model_id: d.at("model").at("id").str().map(str::to_owned),
        effort: string_of(d.at("model").at("effort")),
        cost_usd: finite(d.at("cost").at("total_cost_usd")),
        duration_ms: finite(d.at("cost").at("total_duration_ms")),
        version: string_of(d.at("version")),
        context_pct: size.and(cw.at("used_percentage").num()).map(Num),
        context_used: used.map(Num),
        context_size: size.map(Num),
        five_hour: bucket(quota.at(&format!("{group}-5h"))),
        seven_day: bucket(quota.at(&format!("{group}-weekly"))),
        plan: string_of(d.at("plan_tier")),
        ..empty_status(st.ts * 1000.0)
    };
    Some(AgyStatus {
        status,
        title: d.at("conversation_title").filter(|_| d.at("conversation_title").truthy()).map(value_string),
        state: string_of(d.at("agent_state")),
        mode: agy_mode(data),
        queued: d.at("pending_input_count").num().map(Num),
        confirming: d.at("tool_confirmation_pending").is_true(),
    })
}

#[cfg(test)]
#[path = "antigravity_tests.rs"]
mod tests;
