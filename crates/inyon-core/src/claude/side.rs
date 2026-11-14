//! Codex's ephemeral /btw threads have no rollout and reject history/resume APIs.
//! Keep the visible native conversation separately; never mix it with the parent.
use inyon_protocol::{AssistantBlock, ChatItem};
use super::screen::{codex_composer_footer, unstyle};
use super::screen_reply::carry_on;
use crate::re;

fn text(item: &ChatItem) -> (bool, &str) {
    match item {
        ChatItem::User { text, .. } => (true, text),
        ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } => (false, text),
        _ => (false, ""),
    }
}

pub fn merge(previous: &[ChatItem], captured: &[String], id: &str) -> Option<Vec<ChatItem>> {
    let lines: Vec<_> = captured.iter().map(|l| unstyle(l).text.trim_end().to_owned()).collect();
    let (end, _) = codex_composer_footer(&lines)?;
    let mut blocks: Vec<(bool, String)> = vec![];
    let mut continuation = String::new();
    for line in &lines[..end] {
        if re!(r"^\s*Worked for |^\s*└ Tip: ").is_match(line) { continue; }
        if re!(r"^[•◦●○◉∙⋅\u{2800}-\u{28ff}] .*esc to interrupt|^\s*\d+ background terminals? running").is_match(line) { break; }
        if let Some(t) = line.strip_prefix("› ").or_else(|| line.strip_prefix("» ")) {
            blocks.push((true, t.to_owned()));
        } else if let Some(t) = line.strip_prefix("• ") {
            blocks.push((false, t.to_owned()));
        } else if let Some((_, t)) = blocks.last_mut() {
            t.push('\n'); t.push_str(line.strip_prefix("  ").unwrap_or(line));
        } else {
            continuation.push_str(line.strip_prefix("  ").unwrap_or(line)); continuation.push('\n');
        }
    }
    for (_, t) in &mut blocks { *t = t.trim().to_owned(); }
    blocks.retain(|(_, t)| !t.is_empty());
    let mut out = previous.to_vec();
    if !continuation.trim().is_empty() {
        if let Some(ChatItem::Assistant { block: AssistantBlock::Text { text }, .. }) = out.last_mut() {
            *text = carry_on(text, continuation.trim());
        }
    }
    // Longest suffix overlap keeps repeated questions separate while replacing a
    // streaming final block. Only matching roles and actual text can overlap.
    let overlap = (1..=blocks.len().min(out.len())).rev().find(|&n| {
        out[out.len()-n..].iter().zip(&blocks[..n]).enumerate().all(|(i, (a,b))| {
            let a = text(a);
            a.0 == b.0 && (a.1 == b.1 || (i == n-1 && !a.0 && (b.1.starts_with(a.1) || a.1.starts_with(&b.1))))
        })
    }).unwrap_or(0);
    let start = out.len()-overlap;
    for (i, (user, value)) in blocks.into_iter().enumerate() {
        let index = start+i;
        if let Some(old) = out.get(index) {
            // A narrower viewport must not truncate a reply already seen in full.
            if text(old).1.starts_with(&value) { continue; }
        }
        let seq = index as i64;
        let uuid = format!("side:{id}:{seq}");
        let item = if user { ChatItem::User { seq, uuid, ts: String::new(), text: value, images: 0, queued: None, shell: None } }
            else { ChatItem::Assistant { seq, msg_id: uuid.clone(), uuid, ts: String::new(), model: None, block: AssistantBlock::Text { text: value } } };
        if index < out.len() { out[index] = item; } else { out.push(item); }
    }
    (out != previous).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(body: &[&str]) -> Vec<String> {
        body.iter().map(|s| s.to_string()).chain(["› Ask a follow-up question".into(), "  00000000-0000-0000-0000-000000000001 · GPT-6-Astra high".into(), "  Side from main thread · ⌃/ to switch · ⌃c to close".into()]).collect()
    }
    #[test]
    fn streams_then_preserves_reply_and_keeps_followups() {
        let first = merge(&[], &screen(&["› Question", "", "• First"]), "side").unwrap();
        let next = merge(&first, &screen(&["› Question", "", "• First answer"]), "side").unwrap();
        assert_eq!(next.len(), 2);
        assert!(merge(&next, &screen(&["› Question", "", "• First"]), "side").is_none());
        let follow = merge(&next, &screen(&["› Question", "", "• First answer", "", "› Question", "", "• Second answer"]), "side").unwrap();
        assert_eq!(follow.len(), 4);
        assert_eq!(text(&follow[3]).1, "Second answer");
    }
    #[test]
    fn native_timing_and_shortcut_tips_are_not_assistant_text() {
        let items = merge(&[], &screen(&["› Question", "• Answer", "  Worked for 2s • 12:00", "└ Tip: Use a preset shortcut"]), "side").unwrap();
        assert_eq!(text(&items[1]).1, "Answer");
    }
}
