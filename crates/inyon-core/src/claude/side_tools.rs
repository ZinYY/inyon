//! Native BTW tool rows have no rollout entries. Preserve the visible output
//! as a tool card, including truncation, without pretending to recover hidden rows.
use inyon_protocol::{AssistantBlock, ChatItem};
use serde_json::{json, Value};
use crate::re;

pub(super) struct Tool {
    pub name: String,
    pub input: Value,
    pub output: String,
    pub truncated: bool,
    pub orphan: bool,
    pub is_error: bool,
}

#[derive(Clone, Copy)]
struct Header<'a> {
    name: &'static str,
    command: &'a str,
    pending: bool,
    is_error: bool,
}

fn header(text: &str) -> Option<Header<'_>> {
    let head = text.split_once("\n└").map_or(text, |(head, _)| head);
    for (prefix, name, pending) in [("Ran ", "Bash", false), ("Running ", "Bash", true),
        ("Called ", "Tool call", false), ("Calling ", "Tool call", true)] {
        if let Some(command) = head.strip_prefix(prefix) {
            return Some(Header { name, command: command.trim(), pending, is_error: false });
        }
    }
    // Failed executions use their own native heading, not "Ran". Keep the
    // command and error flag separate so they use the main chat's tool UI.
    let failed = re!(r"^Failed \(exit -?\d+\) (.+)$", "s").captures(head)?;
    Some(Header { name: "Bash", command: failed.get(1)?.as_str().trim(), pending: false, is_error: true })
}

pub(super) fn parse(text: &str) -> Option<Tool> {
    let (head, output) = text.split_once("\n└")?;
    let orphan = head == "Tool output";
    let (name, input, is_error) = if orphan { ("Tool output".to_owned(), json!({}), false) }
        else {
            let h = header(head)?;
            (h.name.to_owned(), if h.name == "Bash" { json!({"command":h.command}) } else { json!({"call":h.command}) }, h.is_error)
        };
    let output = output.strip_prefix(' ').unwrap_or(output).trim_end().to_owned();
    let truncated = output.lines().any(|l| l.contains("lines (") && l.contains("to expand"))
        || head.contains('…') || output.lines().any(|l| l.ends_with('…'));
    Some(Tool { name, input, output, truncated, orphan, is_error })
}

/// Tool headings can have the same long, truncated command prefix. Unlike
/// prose, that opening alone is not an identity: compare the visible output.
pub(super) fn compatible(a: &str, b: &str) -> Option<bool> {
    if a == b && (header(a).is_some() || parse(a).is_some()) { return Some(true); }
    if parse(a).is_some_and(|t| t.orphan) || parse(b).is_some_and(|t| t.orphan) {
        return Some(same_clipped(a, b));
    }
    let (a_head, b_head) = (header(a), header(b));
    let (Some(a_head), Some(b_head)) = (a_head, b_head) else {
        return (a_head.is_some() || b_head.is_some()).then_some(false);
    };
    let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let commands_match = a_head.command == b_head.command || {
        let (a, b) = (compact(a_head.command), compact(b_head.command));
        a == b || (a.ends_with('…') && b.starts_with(a.trim_end_matches('…')))
            || (b.ends_with('…') && a.starts_with(b.trim_end_matches('…')))
    };
    if a_head.name != b_head.name || !commands_match { return Some(false); }
    if a_head.pending || b_head.pending { return Some(true); }
    if a_head.is_error != b_head.is_error { return Some(false); }
    let (Some(a), Some(b)) = (parse(a), parse(b)) else { return Some(true); };
    let (a, b) = (visible_output(&a.output), visible_output(&b.output));
    Some(a == b || (!a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))))
}

fn visible_output(output: &str) -> String {
    output.lines().filter(|line| !re!(r"^\s*[+…] \d+ lines? \(.*to expand\)$").is_match(line))
        .flat_map(|line| line.trim_end_matches('…').chars()).filter(|c| !c.is_whitespace()).collect()
}

pub(super) fn incomplete_after_result(before: &str, now: &str) -> bool {
    parse(before).is_some() && header(now).is_some_and(|h| h.pending || parse(now).is_none())
}

/// Only a header clipped out of view may be matched by output. Two separately
/// executed commands with identical output still remain two calls.
pub(super) fn same_clipped(a: &str, b: &str) -> bool {
    let (Some(a), Some(b)) = (parse(a), parse(b)) else { return false };
    if !(a.orphan || b.orphan) { return false; }
    let a = visible_output(&a.output);
    let b = visible_output(&b.output);
    a.len().min(b.len()) >= 20 && (a.contains(&b) || b.contains(&a))
}

pub(super) fn identity(text: &str) -> String {
    // Running → Ran is one repaint of a call, not another assistant message.
    let text = header(text).map_or(text, |h| h.command);
    super::screen_reply::bare(text)
}

pub(super) fn render(items: Vec<ChatItem>) -> Vec<ChatItem> {
    let mut out = vec![];
    for mut item in items {
        // Reserve the next sequence number for a result. Keys remain stable as
        // the visible output grows or its Markdown source becomes available.
        match &mut item {
            ChatItem::User { seq, .. } | ChatItem::Assistant { seq, .. } => *seq *= 2,
            _ => {},
        }
        let ChatItem::Assistant { seq, uuid, ts, block: AssistantBlock::Text { text }, .. } = &item else { out.push(item); continue };
        let Some(tool) = parse(text) else { out.push(item); continue };
        let id = format!("{uuid}:tool");
        out.push(ChatItem::Assistant { seq: *seq, uuid: uuid.clone(), ts: ts.clone(), msg_id: uuid.clone(), model: None,
            block: AssistantBlock::ToolUse { id: id.clone(), name: tool.name, input: tool.input } });
        out.push(ChatItem::ToolResult { seq: *seq+1, uuid: format!("{uuid}:result"), ts: ts.clone(), tool_use_id: id,
            is_error: tool.is_error, text: tool.output, truncated: Some(tool.truncated), meta: None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_output_is_literal_and_marks_hidden_rows() {
        let t = parse("Ran printf 'a|b'\n└ a|b\n  + 17 lines (^t to expand)").unwrap();
        assert_eq!(t.name, "Bash");
        assert_eq!(t.input["command"], "printf 'a|b'");
        assert!(t.truncated);
        assert!(t.output.starts_with("a|b"));
        assert!(parse("Ran a marathon yesterday.").is_none());
        assert!(parse("A diagram\n└ its branch").is_none());
    }
}
