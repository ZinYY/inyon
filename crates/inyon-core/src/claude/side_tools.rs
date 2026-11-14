//! Native BTW tool rows have no rollout entries. Preserve the visible output
//! as a tool card, including truncation, without pretending to recover hidden rows.
use inyon_protocol::{AssistantBlock, ChatItem};
use serde_json::{json, Value};

pub(super) struct Tool {
    pub name: String,
    pub input: Value,
    pub output: String,
    pub truncated: bool,
    pub orphan: bool,
}

pub(super) fn parse(text: &str) -> Option<Tool> {
    let (head, output) = text.split_once("\n└")?;
    let orphan = head == "Tool output";
    let (name, input) = if let Some(command) = head.strip_prefix("Ran ").or_else(|| head.strip_prefix("Running ")) {
        ("Bash".to_owned(), json!({"command":command.trim()}))
    } else if let Some(call) = head.strip_prefix("Called ").or_else(|| head.strip_prefix("Calling ")) {
        ("Tool call".to_owned(), json!({"call":call.trim()}))
    } else if orphan { ("Tool output".to_owned(), json!({})) }
    else { return None };
    let output = output.strip_prefix(' ').unwrap_or(output).trim_end().to_owned();
    let truncated = output.lines().any(|l| l.contains("lines (") && l.contains("to expand"))
        || head.contains('…') || output.lines().any(|l| l.ends_with('…'));
    Some(Tool { name, input, output, truncated, orphan })
}

/// Only a header clipped out of view may be matched by output. Two separately
/// executed commands with identical output still remain two calls.
pub(super) fn same_clipped(a: &str, b: &str) -> bool {
    let (Some(a), Some(b)) = (parse(a), parse(b)) else { return false };
    if !(a.orphan || b.orphan) { return false; }
    let a = super::screen_reply::bare(&a.output);
    let b = super::screen_reply::bare(&b.output);
    a.len().min(b.len()) >= 20 && (a.starts_with(&b) || b.starts_with(&a))
}

pub(super) fn identity(text: &str) -> String {
    // Running → Ran is one repaint of a call, not another assistant message.
    let text = text.strip_prefix("Running ").or_else(|| text.strip_prefix("Ran ")).unwrap_or(text);
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
            is_error: false, text: tool.output, truncated: Some(tool.truncated), meta: None });
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
