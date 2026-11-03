//! The chat's rows (Messages.tsx and Tools.tsx): your messages as bubbles, the agent's turns with
//! their text, thinking and tool calls, commands, notices and goals, and the rows that follow the
//! conversation: messages on their way, queued or held, the reply being written and the working
//! line. Each row is drawn from chat-core's `ListRow`.

use dioxus::prelude::*;
use inyon_chat_core::chat::{AssistantTurn, Part, Row, ToolCall};
use inyon_chat_core::format::{clock, duration};
use inyon_chat_core::js::parse_date;
use inyon_chat_core::pending::{PendingMessage, PendingStatus};
use inyon_chat_core::tools::{summarize, ToolIcon};
use inyon_chat_core::ListRow;
use inyon_protocol::{AgentKind, ChatItem, HeldPrompt, LiveItem, LiveKind, NoticeLevel};
use serde_json::Value;

use super::icons::{AgentMark, Glyph, Icon};
use super::md::{CopyButton, Markdown};
use super::menu::{self, Action, Item};
use crate::platform;

/// What every row of one chat shares.
#[derive(Debug, Clone, PartialEq)]
pub struct Ctx {
    /// the chat's key (`host:pane`)
    pub key: String,
    pub host: String,
    pub pane: String,
    pub agent: Option<AgentKind>,
    /// "Claude Code", or "the agent" when it isn't known
    pub agent_name: String,
    /// the reply font's class
    pub font: &'static str,
    /// every tool call as its own row (the Tool detail setting), not long runs folded
    pub all_tools: bool,
}

fn tz() -> i32 {
    platform::clock().tz_minutes
}

#[component]
pub fn RowView(row: ListRow, ctx: Ctx, working: bool) -> Element {
    match row {
        ListRow::Row(Row::User { item, .. }) => rsx! { UserMessage { item } },
        ListRow::Row(Row::Assistant(turn)) => {
            let live = working && turn.open;
            rsx! { Turn { turn, live, ctx } }
        }
        ListRow::Row(Row::Command { name, args, output, .. }) => rsx! { CommandRow { name, args, output } },
        ListRow::Row(Row::Notice { item, .. }) => rsx! { NoticeRow { item } },
        ListRow::Row(Row::Goal { item, .. }) => rsx! { GoalRow { item } },
        ListRow::Pending(p) => rsx! { PendingBubble { p, ctx } },
        ListRow::Queued { text, .. } => rsx! { QueuedBubble { text, agent: ctx.agent_name.clone() } },
        ListRow::Held { prompt, index, count } => rsx! { HeldBubble { prompt, index, count, agent: ctx.agent_name.clone() } },
        ListRow::CatchingUp { done } => rsx! {
            div { class: "line-row fade-in", role: "status",
                span { class: "line-mark", span { class: "dot dot-background" } }
                span { "Catching up on what you missed…" }
                span { class: "tabular", {format!("{}%", (done * 100.0).round())} }
            }
        },
        ListRow::Live { items } => rsx! { LiveTurn { items, ctx } },
        ListRow::Working { activity, since, output, joins_turn } => rsx! {
            WorkingLine { activity, since, output, joins: joins_turn, agent: ctx.agent }
        },
        ListRow::Background { text } => rsx! {
            div { class: "line-row fade-in",
                span { class: "line-mark", span { class: "dot dot-background" } }
                {crate::format::background_text(&text)}
            }
        },
    }
}

// ---------- your messages ----------

/// Upload paths (`…/.inyon/uploads/<file>`, maybe with an `@` in front) in a message, split out
/// from the text around them: `(true, path)` for each upload.
pub fn upload_spans(text: &str) -> Vec<(bool, &str)> {
    const MARK: &str = "/.inyon/uploads/";
    let mut out = vec![];
    let mut rest = 0;
    let mut from = 0;
    while let Some(i) = text[from..].find(MARK).map(|i| i + from) {
        let start = text[..i].rfind(char::is_whitespace).map_or(0, |w| w + 1).max(rest);
        let end = text[i..].find(char::is_whitespace).map_or(text.len(), |w| w + i);
        // a path starts at "/" (or "@/"), and has a file name after the folder
        let path = &text[start..end];
        if (path.starts_with('/') || path.starts_with("@/")) && end > i + MARK.len() {
            if start > rest {
                out.push((false, &text[rest..start]));
            }
            out.push((true, path));
            rest = end;
        }
        from = end.max(i + MARK.len());
    }
    if rest < text.len() {
        out.push((false, &text[rest..]));
    }
    out
}

/// An upload's name as it was picked: the hub puts `<time>-<id>-` in front (uploadPath).
pub fn upload_name(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let b = name.as_bytes();
    let digits = |r: std::ops::Range<usize>, hex: bool| {
        b.len() > r.end && b[r.clone()].iter().all(|c| if hex { c.is_ascii_hexdigit() && !c.is_ascii_uppercase() } else { c.is_ascii_digit() })
    };
    if !(digits(0..6, false) && b[6] == b'-') {
        return name;
    }
    if digits(7..13, true) && b[13] == b'-' {
        &name[14..]
    } else {
        &name[7..]
    }
}

fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "avif", "bmp", "svg"].iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// A message long enough to fold until "Show more".
pub fn long_message(text: &str) -> bool {
    text.chars().count() > 1200 || text.split('\n').count() > 18
}

#[component]
fn LinkedPaths(text: String) -> Element {
    rsx! {
        for (upload , part) in upload_spans(&text) {
            if upload {
                span { class: "upload-chip mono",
                    Glyph { icon: if is_image(part) { Icon::Image } else { Icon::File }, size: 12 }
                    span { class: "truncate", {upload_name(part).to_string()} }
                }
            } else {
                "{part}"
            }
        }
    }
}

#[component]
fn UserMessage(item: ChatItem) -> Element {
    let mut expanded = use_signal(|| false);
    let ChatItem::User { text, ts, images, queued, shell, .. } = item else { return rsx! {} };
    if shell == Some(true) {
        return rsx! {
            div { class: "msg-user",
                div { class: "bubble bubble-shell mono", oncontextmenu: menu::open(vec![Item::copy("Copy command", &text)]),
                    span { class: "shell-mark", "!" }
                    "{text}"
                }
            }
        };
    }
    let long = long_message(&text);
    let folded = long && !expanded();
    rsx! {
        div { class: "msg-user",
            div { class: if folded { "bubble bubble-folded" } else { "bubble" }, oncontextmenu: menu::open(vec![Item::copy("Copy message", &text)]),
                if images > 0 {
                    span { class: "image-chip",
                        Glyph { icon: Icon::Image, size: 14 }
                        {crate::format::plural(images as usize, "image")}
                    }
                }
                LinkedPaths { text: text.clone() }
                if folded {
                    button { class: "bubble-more", r#type: "button", onclick: move |_| expanded.set(true), "Show more" }
                }
            }
            div { class: "msg-meta",
                if queued == Some(true) {
                    span { "Sent while the agent was working ·" }
                }
                span { {clock(&ts, tz())} }
                CopyButton { text: text.clone(), label: "Copy message" }
            }
        }
    }
}

#[component]
fn PendingBubble(p: PendingMessage, ctx: Ctx) -> Element {
    let chats = use_context::<Signal<crate::state::Chats>>();
    let failed = p.status == PendingStatus::Failed;
    let mut items = vec![Item::copy("Copy message", &p.text)];
    if failed && !p.remote {
        items.push(Item::new("Retry", Icon::RotateCcw, Action::Retry { host: ctx.host.clone(), pane: ctx.pane.clone(), id: p.id.clone(), text: p.text.clone() }));
        items.push(Item::new("Dismiss", Icon::X, Action::Dismiss { key: ctx.key.clone(), id: p.id.clone() }).danger());
    }
    let (retry_host, retry_pane, retry_id, retry_text) = (ctx.host.clone(), ctx.pane.clone(), p.id.clone(), p.text.clone());
    let (key, id) = (ctx.key.clone(), p.id.clone());
    rsx! {
        div { class: "msg-user",
            // the same box as the message it becomes, so nothing moves when the agent takes it
            div { class: if failed { "bubble" } else { "bubble bubble-pending" }, oncontextmenu: menu::open(items),
                LinkedPaths { text: p.text.clone() }
            }
            div { class: "msg-meta msg-meta-shown",
                match p.status {
                    PendingStatus::Sending => rsx! {
                        Glyph { icon: Icon::Spinner, size: 12, class: "spin" }
                        if p.remote { "Sending from another device" } else { "Sending" }
                    },
                    PendingStatus::Sent => rsx! {
                        Glyph { icon: Icon::Check, size: 12 }
                        if p.remote { "Sent from another device" } else { "Sent" }
                    },
                    PendingStatus::Failed => rsx! {
                        button { class: "msg-retry", r#type: "button",
                            onclick: move |_| super::composer::retry(chats, retry_host.clone(), retry_pane.clone(), retry_id.clone(), retry_text.clone()),
                            Glyph { icon: Icon::CircleAlert, size: 12 }
                            {format!("{} · ", p.error.clone().unwrap_or_else(|| "Not sent".into()))}
                            Glyph { icon: Icon::RotateCcw, size: 12 }
                            "Retry"
                        }
                    },
                }
                CopyButton { text: p.text.clone(), label: "Copy message" }
                if failed {
                    button { class: "icon-button press", r#type: "button", title: "Dismiss", "aria-label": "Dismiss",
                        onclick: move |_| super::composer::dismiss(chats, &key, &id),
                        Glyph { icon: Icon::X, size: 14 }
                    }
                }
            }
        }
    }
}

/// A message in the agent's own queue: a bubble once the agent takes it.
#[component]
fn QueuedBubble(text: String, agent: String) -> Element {
    rsx! {
        div { class: "msg-user",
            div { class: "bubble bubble-queued", oncontextmenu: menu::open(vec![Item::copy("Copy message", &text)]),
                LinkedPaths { text: text.clone() }
            }
            div { class: "msg-meta msg-meta-shown",
                Glyph { icon: Icon::Clock3, size: 12 }
                "Queued · {agent} reads it at its next step"
            }
        }
    }
}

/// A message Inyon holds and types in once the agent is free.
#[component]
fn HeldBubble(prompt: HeldPrompt, index: usize, count: usize, agent: String) -> Element {
    let place = if count > 1 { format!(" · {} of {count}", index + 1) } else { String::new() };
    rsx! {
        div { class: "msg-user",
            div { class: "bubble bubble-queued", oncontextmenu: menu::open(vec![Item::copy("Copy message", &prompt.text)]),
                LinkedPaths { text: prompt.text.clone() }
            }
            div { class: "msg-meta msg-meta-shown",
                Glyph { icon: Icon::Hourglass, size: 12 }
                if let Some(e) = prompt.error.as_ref() {
                    span { class: "danger", "{e}" }
                } else {
                    "Held{place} · goes in when {agent} is free"
                }
            }
        }
    }
}

// ---------- the agent's turns ----------

#[component]
fn Mark(agent: Option<AgentKind>, #[props(default)] active: bool) -> Element {
    let class = if active { "turn-mark-img pulse-soft" } else { "turn-mark-img" };
    match agent {
        Some(kind) => rsx! { AgentMark { kind, size: 19, class } },
        None => rsx! { Glyph { icon: Icon::Bot, size: 18, class: "fg-3" } },
    }
}

#[component]
fn Turn(turn: AssistantTurn, live: bool, ctx: Ctx) -> Element {
    let text = turn.parts.iter().filter_map(|p| if let Part::Text { text, .. } = p { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n\n");
    let items = if !turn.open && !text.is_empty() { vec![Item::copy("Copy reply", &text)] } else { vec![] };
    rsx! {
        div { class: "turn",
            div { class: "turn-mark", Mark { agent: ctx.agent } }
            div { class: "turn-body", oncontextmenu: menu::open(items),
                for p in turn.parts.iter() {
                    PartView { key: "{p.key()}", part: p.clone(), live, ctx: ctx.clone() }
                }
                if !turn.open && text.contains("/pages/") {
                    super::pages::PageCards { text: text.clone(), host: ctx.host.clone() }
                }
                if !turn.open {
                    div { class: "turn-meta",
                        if !text.is_empty() {
                            CopyButton { text: text.clone(), label: "Copy reply", class: "turn-copy" }
                        }
                        span { {clock(&turn.ts, tz())} }
                        if let Some(ms) = turn.duration_ms.filter(|ms| *ms > 0.0) {
                            span { "· worked {duration(ms)}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PartView(part: Part, live: bool, ctx: Ctx) -> Element {
    match part {
        Part::Text { text, .. } => rsx! { Markdown { text, class: "part-text", font: ctx.font } },
        Part::Thinking { text, .. } => rsx! { Thinking { text } },
        Part::Tools { calls, .. } => rsx! { ToolGroup { calls, live, all: ctx.all_tools } },
        Part::Special { call, .. } => rsx! { SpecialTool { call, live, agent: ctx.agent_name.clone(), all: ctx.all_tools } },
        Part::Notice { item, .. } => match &item {
            ChatItem::Notice { subtype, .. } if subtype == "task" => rsx! { JobFinished { item } },
            _ => rsx! { div { class: "part-note", "Interrupted" } },
        },
    }
}

/// Grid rows from 0fr to 1fr: a fold that opens on the spring and closes quicker. What's inside is
/// drawn from the first time it opens.
#[component]
fn Fold(open: bool, children: Element) -> Element {
    let drawn = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(false)));
    if open {
        drawn.set(true);
    }
    rsx! {
        div { class: "expand", "data-open": "{open}",
            div { if drawn.get() { {children} } }
        }
    }
}

#[component]
fn Thinking(text: String) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "thinking",
            button { class: "fold-button", r#type: "button", "aria-expanded": "{open}", onclick: move |_| open.toggle(),
                Glyph { icon: Icon::Lightbulb, size: 15 }
                span { class: "fold-label", "Thinking" }
                Glyph { icon: Icon::ChevronRight, size: 14, class: if open() { "chevron chevron-open" } else { "chevron" } }
            }
            Fold { open: open(), div { class: "thinking-text", "{text}" } }
        }
    }
}

/// A background job finishing, where it happened in the conversation.
#[component]
fn JobFinished(item: ChatItem) -> Element {
    let ChatItem::Notice { level, text, ts, .. } = item else { return rsx! {} };
    let failed = level != NoticeLevel::Info;
    rsx! {
        div { class: "job-row",
            Glyph { icon: if failed { Icon::CircleX } else { Icon::CircleCheck }, size: 16, class: if failed { "danger" } else { "fg-3" } }
            span { class: "truncate", "{text}" }
            span { class: "job-time tabular", {clock(&ts, tz())} }
        }
    }
}

// ---------- tool calls ----------

fn tool_icon(i: ToolIcon) -> Icon {
    match i {
        ToolIcon::Bell => Icon::Bell,
        ToolIcon::BookMarked => Icon::BookMarked,
        ToolIcon::Bot => Icon::Bot,
        ToolIcon::ClipboardList => Icon::ClipboardList,
        ToolIcon::FileCode2 => Icon::FileCode2,
        ToolIcon::FilePen => Icon::FilePen,
        ToolIcon::FilePlus2 => Icon::FilePlus2,
        ToolIcon::FileSearch => Icon::FileSearch,
        ToolIcon::FileText => Icon::FileText,
        ToolIcon::FolderSearch => Icon::FolderSearch,
        ToolIcon::Globe => Icon::Globe,
        ToolIcon::Hourglass => Icon::Hourglass,
        ToolIcon::Image => Icon::Image,
        ToolIcon::ListChecks => Icon::ListChecks,
        ToolIcon::MessageCircleQuestion => Icon::MessageCircleQuestion,
        ToolIcon::MessageSquare => Icon::MessageSquare,
        ToolIcon::Plug => Icon::Plug,
        ToolIcon::ScrollText => Icon::ScrollText,
        ToolIcon::Search => Icon::Search,
        ToolIcon::SquareTerminal => Icon::SquareTerminal,
        ToolIcon::Timer => Icon::Timer,
        ToolIcon::Workflow => Icon::Workflow,
        ToolIcon::Wrench => Icon::Wrench,
    }
}

/// How long a call took, from the call to its result ("0.4s", "12s", "2m 5s").
pub fn took(call: &ToolCall) -> Option<String> {
    let start = parse_date(call.ts.as_deref()?);
    let ChatItem::ToolResult { ts, .. } = call.result.as_ref()? else { return None };
    let ms = parse_date(ts) - start;
    if ms.is_nan() || ms < 0.0 {
        return None;
    }
    Some(if ms < 1000.0 { format!("{}s", (ms / 100.0).round().max(1.0) / 10.0) } else { duration(ms) })
}

fn result_of(call: &ToolCall) -> Option<(&str, bool)> {
    match call.result.as_ref()? {
        ChatItem::ToolResult { text, is_error, .. } => Some((text.as_str(), *is_error)),
        _ => None,
    }
}

fn input_str<'a>(call: &'a ToolCall, field: &str) -> Option<&'a str> {
    call.input.get(field).and_then(Value::as_str)
}

/// An edit's lines as a patch (`+`/`-`/` ` in front), from the hunks the agent recorded, else from
/// the edit itself: the old text out, the new text in.
pub fn edit_lines(call: &ToolCall) -> Vec<String> {
    if let Some(patch) = call.meta().and_then(|m| m.patch.as_ref()).filter(|p| !p.is_empty()) {
        let mut out = vec![];
        for (i, h) in patch.iter().enumerate() {
            if i > 0 {
                out.push("⋯".into());
            }
            out.extend(h.lines.iter().filter(|l| !l.starts_with('\\')).cloned());
        }
        return out;
    }
    let one = |old: &str, new: &str, out: &mut Vec<String>| {
        out.extend(old.split('\n').filter(|_| !old.is_empty()).map(|l| format!("-{l}")));
        out.extend(new.split('\n').filter(|_| !new.is_empty()).map(|l| format!("+{l}")));
    };
    let mut out = vec![];
    if let Some(edits) = call.input.get("edits").and_then(Value::as_array) {
        for e in edits {
            one(e.get("old_string").and_then(Value::as_str).unwrap_or(""), e.get("new_string").and_then(Value::as_str).unwrap_or(""), &mut out);
        }
    } else if call.input.get("old_string").is_some() {
        one(input_str(call, "old_string").unwrap_or(""), input_str(call, "new_string").unwrap_or(""), &mut out);
    } else if let Some(content) = input_str(call, "content") {
        one("", content, &mut out);
    }
    out
}

#[component]
fn Output(text: String, #[props(default)] label: String) -> Element {
    if text.trim().is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "tool-output",
            if !label.is_empty() {
                div { class: "tool-output-label", "{label}" }
            }
            pre { "{text}" }
        }
    }
}

#[component]
fn PatchView(lines: Vec<String>) -> Element {
    rsx! {
        pre { class: "patch",
            for (i , l) in lines.iter().enumerate() {
                div {
                    key: "{i}",
                    class: if l.starts_with('+') && !l.starts_with("+++") { "diff-add" } else if l.starts_with('-') && !l.starts_with("---") { "diff-del" } else if l.starts_with("@@") || l == "⋯" { "diff-hunk" } else { "diff-same" },
                    if l.is_empty() { " " } else { "{l}" }
                }
            }
        }
    }
}

#[component]
fn Details(call: ToolCall) -> Element {
    let result = result_of(&call).map(|(t, e)| (t.to_string(), e));
    let meta = call.meta().cloned();
    let label = |error: bool, ok: &str| if error { "Error".to_string() } else { ok.to_string() };
    match call.name.as_str() {
        "Bash" => {
            let command = input_str(&call, "command").unwrap_or("").to_string();
            let stdout = meta.as_ref().and_then(|m| m.stdout.clone());
            let stderr = meta.as_ref().and_then(|m| m.stderr.clone()).filter(|s| !s.is_empty());
            rsx! {
                div { class: "tool-command",
                    span { class: "tool-prompt mono", "$" }
                    pre { "{command}" }
                    CopyButton { text: command.clone(), label: "Copy command" }
                }
                if let Some((text, _)) = result {
                    Output { text: stdout.clone().unwrap_or(text.clone()) }
                    if let Some(err) = stderr.clone() {
                        Output { text: err, label: "stderr" }
                    }
                    if text.trim().is_empty() && stdout.is_none() && stderr.is_none() {
                        div { class: "tool-empty", "No output" }
                    }
                }
            }
        }
        "Edit" | "MultiEdit" | "Write" | "ApplyPatch" if !edit_lines(&call).is_empty() => {
            let path = input_str(&call, "file_path").map(|p| inyon_chat_core::format::short_path(Some(p), 80));
            rsx! {
                if let Some(p) = path {
                    div { class: "tool-path mono", "{p}" }
                }
                PatchView { lines: edit_lines(&call) }
                if let Some((text, true)) = result {
                    Output { text }
                }
            }
        }
        "Read" => rsx! { Output { text: result.map(|r| r.0).unwrap_or_default() } },
        "Task" | "Agent" => {
            let prompt = input_str(&call, "prompt").map(str::to_string);
            rsx! {
                div { class: "tool-agent",
                    if let Some(p) = prompt {
                        div { class: "tool-output-label", "Prompt" }
                        div { class: "tool-agent-prompt", "{p}" }
                    }
                    if let Some((text, _)) = result {
                        Markdown { text, class: "tool-agent-result" }
                    }
                }
            }
        }
        _ => {
            let input = serde_json::to_string_pretty(&call.input).unwrap_or_default();
            rsx! {
                Output { text: input, label: "Input" }
                if let Some((text, error)) = result {
                    Output { text, label: label(error, "Result") }
                }
            }
        }
    }
}

#[component]
fn ToolRow(call: ToolCall, live: bool, all: bool) -> Element {
    let mut open = use_signal(|| false);
    let s = summarize(&call);
    let error = result_of(&call).is_some_and(|r| r.1);
    let time = if all { took(&call) } else { None };
    // the command itself, under its description
    let command = (all && matches!(call.name.as_str(), "Bash" | "Monitor") && input_str(&call, "description").is_some())
        .then(|| input_str(&call, "command").map(|c| c.lines().next().unwrap_or("").to_string()))
        .flatten();
    let status = if error {
        Some(rsx! { Glyph { icon: Icon::CircleAlert, size: 14, class: "danger" } })
    } else if call.done() {
        all.then(|| rsx! { Glyph { icon: Icon::Check, size: 14, class: "fg-3" } })
    } else if live {
        Some(rsx! { Glyph { icon: Icon::Spinner, size: 14, class: "spin fg-3" } })
    } else {
        Some(rsx! { Glyph { icon: Icon::CircleDashed, size: 14, class: "fg-3" } })
    };
    rsx! {
        div { class: if open() { "tool tool-open" } else { "tool" },
            button { class: "tool-head", r#type: "button", "aria-expanded": "{open}", onclick: move |_| open.toggle(),
                Glyph { icon: tool_icon(s.icon), size: 16, class: if error { "danger tool-icon" } else { "fg-3 tool-icon" } }
                span { class: if command.is_some() { "tool-words tool-words-stacked" } else { "tool-words" },
                    span { class: "tool-line",
                        span { class: "tool-verb", "{s.verb}" }
                        if let Some(t) = s.target.as_ref() {
                            span { class: if s.mono { "tool-target mono" } else { "tool-target" }, "{t}" }
                        }
                    }
                    if let Some(c) = command.as_ref() {
                        span { class: "tool-sub mono", "$ {c}" }
                    }
                }
                if let Some(st) = s.stats.filter(|st| st.add > 0 || st.del > 0) {
                    span { class: "tool-stats mono",
                        span { class: "ok", "+{st.add}" }
                        " "
                        span { class: "danger", "−{st.del}" }
                    }
                }
                span { class: "tool-end",
                    if let Some(t) = time {
                        span { class: "tabular fg-3 tool-time", "{t}" }
                    }
                    {status}
                    Glyph { icon: Icon::ChevronRight, size: 14, class: if open() { "chevron chevron-open" } else { "chevron" } }
                }
            }
            Fold { open: open(),
                div { class: "tool-details", Details { call: call.clone() } }
            }
        }
    }
}

#[component]
fn ToolGroup(calls: Vec<ToolCall>, live: bool, all: bool) -> Element {
    let mut expanded = use_signal(|| false);
    let collapsible = !all && calls.len() > 6;
    let hidden = if collapsible && !expanded() { calls.len() - 4 } else { 0 };
    rsx! {
        div { class: "tools",
            if hidden > 0 {
                button { class: "tool-more", r#type: "button", onclick: move |_| expanded.set(true),
                    Glyph { icon: Icon::ChevronRight, size: 16 }
                    "{hidden} more steps"
                }
            }
            for c in calls.iter().skip(hidden) {
                ToolRow { key: "{c.id}", call: c.clone(), live, all }
            }
        }
    }
}

#[component]
fn Card(icon: Icon, title: String, #[props(default)] accent: bool, children: Element) -> Element {
    rsx! {
        div { class: if accent { "tool-card tool-card-accent" } else { "tool-card" },
            div { class: "tool-card-head",
                Glyph { icon, size: 16 }
                "{title}"
            }
            div { class: "tool-card-body", {children} }
        }
    }
}

#[component]
fn SpecialTool(call: ToolCall, live: bool, agent: String, all: bool) -> Element {
    let result = result_of(&call).map(|(t, e)| (t.to_string(), e));
    let meta = call.meta().cloned();
    match call.name.as_str() {
        "ExitPlanMode" => {
            let plan = meta.and_then(|m| m.plan).or_else(|| input_str(&call, "plan").map(str::to_string)).unwrap_or_default();
            let status = match (&result, live) {
                (Some((_, true)), _) => "Not approved",
                (Some(_), _) => "Approved",
                (None, true) => "Waiting for approval",
                (None, false) => "Proposed",
            };
            rsx! {
                Card { icon: Icon::ClipboardList, title: "Plan · {status}", accent: true, PlanBody { text: plan } }
            }
        }
        "AskUserQuestion" => {
            let answers = meta.and_then(|m| m.answers).unwrap_or_default();
            let qs = call.input.get("questions").and_then(Value::as_array).cloned().unwrap_or_default();
            let title = if result.is_some() {
                if qs.len() == 1 {
                    "Question answered".to_string()
                } else {
                    "Questions answered".to_string()
                }
            } else {
                format!("{agent} is asking")
            };
            rsx! {
                Card { icon: Icon::MessageCircleQuestion, title,
                    div { class: "asked",
                        for (n , q) in qs.iter().enumerate() {
                            Asked { key: "{n}", q: q.clone(), answer: q.get("question").and_then(Value::as_str).and_then(|k| answers.get(k)).cloned() }
                        }
                        if let Some((text, true)) = result.as_ref() {
                            div { class: "fg-3", {text.chars().take(300).collect::<String>()} }
                        }
                    }
                }
            }
        }
        "TodoWrite" => {
            let todos = call.input.get("todos").and_then(Value::as_array).cloned().unwrap_or_default();
            rsx! {
                Card { icon: Icon::ListTodo, title: "To-dos",
                    ul { class: "todos",
                        for (n , t) in todos.iter().enumerate() {
                            Todo { key: "{n}", t: t.clone() }
                        }
                    }
                }
            }
        }
        _ => rsx! { ToolRow { call, live, all } },
    }
}

#[component]
fn Asked(q: Value, answer: Option<String>) -> Element {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (header, question) = (s(&q, "header"), s(&q, "question"));
    let options = q.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
    let described = options.iter().any(|o| !s(o, "description").is_empty() || !s(o, "preview").is_empty());
    rsx! {
        div {
            if !header.is_empty() && header != question {
                div { class: "asked-header", "{header}" }
            }
            div { class: "asked-question", "{question}" }
            if let Some(a) = answer {
                div { class: "asked-answer",
                    Glyph { icon: Icon::Check, size: 14 }
                    "{a}"
                }
            } else if described {
                // what each choice means, and its preview: the terminal shows only the highlighted one's
                ul { class: "asked-options",
                    for (k , o) in options.iter().enumerate() {
                        li { key: "{k}",
                            div { {s(o, "label")} }
                            if !s(o, "description").is_empty() {
                                div { class: "asked-detail", {s(o, "description")} }
                            }
                            if !s(o, "preview").is_empty() {
                                pre { class: "asked-preview", {s(o, "preview")} }
                            }
                        }
                    }
                }
            } else {
                div { class: "asked-detail", {options.iter().map(|o| s(o, "label")).collect::<Vec<_>>().join(" · ")} }
            }
        }
    }
}

#[component]
fn Todo(t: Value) -> Element {
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let status = s("status");
    let text = if status == "in_progress" && !s("activeForm").is_empty() { s("activeForm") } else { s("content") };
    rsx! {
        li { class: "todo todo-{status}",
            span { class: "todo-box",
                if status == "completed" {
                    Glyph { icon: Icon::Check, size: 12 }
                } else if status == "in_progress" {
                    span { class: "todo-dot" }
                }
            }
            span { class: "todo-text", "{text}" }
        }
    }
}

#[component]
fn PlanBody(text: String) -> Element {
    let mut full = use_signal(|| false);
    let long = text.chars().count() > 1600;
    rsx! {
        div { class: "plan",
            div { class: if long && !full() { "plan-folded" } else { "" }, Markdown { text } }
            if long && !full() {
                div { class: "plan-more",
                    button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| full.set(true), "Show full plan" }
                }
            }
        }
    }
}

// ---------- commands, notices, goals ----------

/// What a command row's pill says.
pub fn command_label(name: &str, args: &str) -> String {
    match name {
        "!" => "shell output".into(),
        "" => "command output".into(),
        _ => {
            let name = name.trim_start_matches('/');
            if args.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {args}")
            }
        }
    }
}

/// A command's output without its colour codes.
pub fn plain_output(out: &str) -> String {
    let mut s = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&d) = chars.peek() {
                chars.next();
                if !(d.is_ascii_digit() || d == ';') {
                    break;
                }
            }
            continue;
        }
        s.push(c);
    }
    s.trim().to_string()
}

#[component]
fn CommandRow(name: String, args: String, output: Option<String>) -> Element {
    let mut open = use_signal(|| false);
    let out = output.as_deref().map(plain_output).filter(|o| !o.is_empty());
    let has_out = out.is_some();
    rsx! {
        div { class: "command-row",
            button { class: if has_out { "command-pill command-pill-open" } else { "command-pill" }, r#type: "button",
                onclick: move |_| if has_out { open.toggle() },
                if name == "!" {
                    Glyph { icon: Icon::SquareTerminal, size: 14, class: "fg-3" }
                }
                span { class: "truncate mono", {command_label(&name, &args)} }
                if has_out {
                    Glyph { icon: Icon::ChevronRight, size: 14, class: if open() { "chevron chevron-open" } else { "chevron" } }
                }
            }
            if let Some(o) = out {
                Fold { open: open(), pre { class: "command-out", "{o}" } }
            }
        }
    }
}

#[component]
fn NoticeRow(item: ChatItem) -> Element {
    let ChatItem::Notice { level, subtype, text, .. } = &item else { return rsx! {} };
    match subtype.as_str() {
        "task" => rsx! { JobFinished { item: item.clone() } },
        "compact" => rsx! {
            div { class: "notice-rule",
                span { class: "rule" }
                Glyph { icon: Icon::Layers, size: 14 }
                "{text}"
                span { class: "rule" }
            }
        },
        "rc" => rsx! { div { class: "notice-rc", "Remote Control turned on" } },
        "recap" => rsx! {
            div { class: "notice-recap",
                div { class: "notice-recap-head",
                    Glyph { icon: Icon::CornerDownLeft, size: 14 }
                    "While you were away"
                }
                "{text}"
            }
        },
        _ => {
            let (class, icon) = match level {
                NoticeLevel::Error => ("notice notice-error", Icon::TriangleAlert),
                NoticeLevel::Warn => ("notice notice-warn", Icon::TriangleAlert),
                NoticeLevel::Info => ("notice", Icon::Info),
            };
            rsx! {
                div { class,
                    Glyph { icon, size: 16, class: "notice-icon" }
                    span { "{text}" }
                }
            }
        }
    }
}

#[component]
fn GoalRow(item: ChatItem) -> Element {
    let ChatItem::Goal { condition, met, reason, cleared, .. } = item else { return rsx! {} };
    let title = if met {
        "Goal met"
    } else if cleared == Some(true) {
        "Goal cleared"
    } else {
        "Goal set"
    };
    rsx! {
        div { class: if met { "goal goal-met" } else { "goal" },
            Glyph { icon: Icon::Target, size: 16, class: if met { "goal-icon" } else { "goal-icon fg-3" } }
            div { class: "goal-text",
                div { class: "goal-title", "{title}" }
                div { class: "goal-condition", "{condition}" }
                if let Some(r) = reason.filter(|_| met) {
                    div { class: "goal-reason", "{r}" }
                }
            }
        }
    }
}

// ---------- what the agent is doing now ----------

/// The spinner text split into its verb and the agent's own detail: "Reading… (12s · esc to
/// interrupt)" → ("Reading…", Some("12s")).
pub fn working_parts(activity: Option<&str>) -> (String, Option<String>) {
    let a = activity.filter(|a| !a.is_empty()).unwrap_or("Working…");
    if let Some(open) = a.rfind(" (").filter(|_| a.ends_with(')')) {
        let rest = &a[open + 2..a.len() - 1];
        let rest = match rest.to_ascii_lowercase().find("esc to interrupt") {
            Some(i) => rest[..i].trim_end().trim_end_matches('·').trim_end().to_string(),
            None => rest.to_string(),
        };
        return (a[..open].to_string(), Some(rest).filter(|r| !r.is_empty()));
    }
    (a.to_string(), None)
}

#[component]
fn WorkingLine(activity: Option<String>, since: Option<f64>, output: Option<String>, joins: bool, agent: Option<AgentKind>) -> Element {
    let (verb, rest) = working_parts(activity.as_deref());
    let mut tick = use_signal(|| 0u32);
    // a tick each second while the elapsed time is shown
    let counting = since.is_some() && rest.is_none();
    use_future(move || async move {
        loop {
            platform::sleep(1000).await;
            if counting {
                tick += 1;
            }
        }
    });
    let _ = tick();
    let elapsed = since.map(|s| (platform::clock().now - s).max(0.0)).unwrap_or(0.0);
    let tail = output.as_deref().and_then(|o| o.trim_end().lines().last()).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    rsx! {
        div { class: if joins { "working joins fade-in" } else { "working fade-in" },
            span { class: "line-mark", Mark { agent, active: true } }
            span { class: "working-text truncate",
                span { class: "working-verb", "{verb}" }
                if let Some(r) = rest {
                    span { class: "fg-3", " {r}" }
                } else if since.is_some() && elapsed >= 1000.0 {
                    span { class: "fg-3 tabular", " {duration(elapsed)}" }
                }
                if let Some(t) = tail {
                    span { class: "working-tail mono", "{t}" }
                }
            }
        }
    }
}

/// The reply as the agent writes it. The finished message takes its place with the same text.
#[component]
fn LiveTurn(items: Vec<LiveItem>, ctx: Ctx) -> Element {
    let shown: Vec<&LiveItem> = items.iter().filter(|i| i.kind != LiveKind::Output && !i.text.trim().is_empty()).collect();
    if shown.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "turn fade-in",
            div { class: "turn-mark", Mark { agent: ctx.agent } }
            div { class: "turn-body",
                for i in shown {
                    if i.kind == LiveKind::Thinking {
                        Thinking { key: "{i.id}", text: i.text.clone() }
                    } else {
                        Markdown { key: "{i.id}", text: i.text.clone(), class: "part-text", font: ctx.font }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uploads_show_as_their_names() {
        let t = "look at @/home/me/.inyon/uploads/251031-a1b2c3-shot.png and\n/tmp/x/.inyon/uploads/120000-notes.md please";
        let spans = upload_spans(t);
        assert_eq!(
            spans,
            vec![
                (false, "look at "),
                (true, "@/home/me/.inyon/uploads/251031-a1b2c3-shot.png"),
                (false, " and\n"),
                (true, "/tmp/x/.inyon/uploads/120000-notes.md"),
                (false, " please")
            ]
        );
        assert_eq!(upload_name(spans[1].1), "shot.png");
        assert_eq!(upload_name(spans[3].1), "notes.md");
        assert_eq!(upload_name("/a/plain.txt"), "plain.txt");
        // the folder alone, or a path that doesn't start at the root, is text
        assert_eq!(upload_spans("see ~/.inyon/uploads/ there"), vec![(false, "see ~/.inyon/uploads/ there")]);
        assert_eq!(upload_spans("    indented\n  code"), vec![(false, "    indented\n  code")]);
    }

    #[test]
    fn long_messages_fold() {
        assert!(!long_message("short"));
        assert!(long_message(&"line\n".repeat(19)));
        assert!(long_message(&"x".repeat(1201)));
    }

    #[test]
    fn the_working_line_reads_the_spinner() {
        assert_eq!(working_parts(None), ("Working…".into(), None));
        assert_eq!(working_parts(Some("Reading… (12s · esc to interrupt)")), ("Reading…".into(), Some("12s".into())));
        assert_eq!(working_parts(Some("Thinking (esc to interrupt)")), ("Thinking".into(), None));
        assert_eq!(working_parts(Some("Baking…")), ("Baking…".into(), None));
    }

    fn call(name: &str, input: Value, result: Option<Value>) -> ToolCall {
        let result = result.map(|r| serde_json::from_value::<ChatItem>(r).unwrap());
        ToolCall { id: "t1".into(), name: name.into(), input, result, ts: Some("2026-10-31T10:00:00.000Z".into()) }
    }

    #[test]
    fn a_call_says_how_long_it_took() {
        let r = |ts: &str| Some(json!({ "kind": "tool_result", "seq": 2, "uuid": "u", "ts": ts, "toolUseId": "t1", "isError": false, "text": "" }));
        assert_eq!(took(&call("Bash", json!({}), r("2026-10-31T10:00:00.400Z"))).as_deref(), Some("0.4s"));
        assert_eq!(took(&call("Bash", json!({}), r("2026-10-31T10:02:05.000Z"))).as_deref(), Some("2m 5s"));
        assert_eq!(took(&call("Bash", json!({}), None)), None);
    }

    #[test]
    fn an_edit_shows_as_a_patch_with_its_indentation() {
        let c = call("Edit", json!({ "file_path": "/a.py", "old_string": "def f():\n    return 1", "new_string": "def f():\n    return 2" }), None);
        assert_eq!(edit_lines(&c), vec!["-def f():", "-    return 1", "+def f():", "+    return 2"]);
        let w = call("Write", json!({ "file_path": "/b.yml", "content": "a:\n  b: 1" }), None);
        assert_eq!(edit_lines(&w), vec!["+a:", "+  b: 1"]);
        assert!(edit_lines(&call("Read", json!({ "file_path": "/a" }), None)).is_empty());
    }

    #[test]
    fn commands_read_as_typed() {
        assert_eq!(command_label("/compact", ""), "/compact");
        assert_eq!(command_label("model", "opus"), "/model opus");
        assert_eq!(command_label("!", "ls"), "shell output");
        assert_eq!(plain_output("\x1b[1;32mok\x1b[0m\n"), "ok");
    }
}
