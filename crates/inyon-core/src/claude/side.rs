//! Codex's ephemeral /btw threads have no rollout and reject history/resume APIs.
//! Keep the visible native conversation separately; never mix it with the parent.
use inyon_protocol::{AssistantBlock, ChatItem};
use super::screen::{codex_composer_footer, unstyle};
use super::screen_reply::bare;
use crate::re;

fn text(item: &ChatItem) -> (bool, &str) {
    match item {
        ChatItem::User { text, .. } => (true, text),
        ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } => (false, text),
        _ => (false, ""),
    }
}

fn same_reply(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() { return false; }
    a.starts_with(b) || b.starts_with(a)
        // A terminal repaint can briefly omit rows in the middle of a reply.
        // Its substantial unchanged opening still identifies the same block.
        || (a.chars().take(64).count() == 64 && b.chars().take(64).count() == 64 && a.chars().take(64).eq(b.chars().take(64)))
}

/// The top of a viewport can start halfway through an already captured reply.
/// Only extend a known suffix; an older or unrelated viewport must never replace it.
fn extend_clipped(before: &str, now: &str) -> Option<String> {
    let old = bare(before);
    let new = bare(now);
    if new.is_empty() || old.contains(&new) { return None; }
    let head: String = new.chars().take(20).collect();
    if head.chars().count() < 20 { return None; }
    let at = old.rmatch_indices(&head).map(|(at, _)| at).find(|&at| new.starts_with(&old[at..]))?;
    let matched = old[at..].chars().count();
    let mut count = 0;
    for (i, c) in now.char_indices() {
        if c.is_alphanumeric() { count += c.to_lowercase().count(); }
        if count == matched {
            let rest = &now[i+c.len_utf8()..];
            let tail = before.char_indices().rev().find(|(_, c)| c.is_alphanumeric()).map_or("", |(i,c)| &before[i+c.len_utf8()..]);
            return Some(format!("{before}{}", rest.strip_prefix(tail).unwrap_or(rest)));
        }
    }
    None
}

pub fn merge(previous: &[ChatItem], captured: &[String], id: &str) -> Option<Vec<ChatItem>> {
    let lines: Vec<_> = captured.iter().map(|l| unstyle(l).text.trim_end().to_owned()).collect();
    let (end, _) = codex_composer_footer(&lines)?;
    let mut blocks: Vec<(bool, String)> = vec![];
    let mut continuation = String::new();
    let mut user_complete = false;
    let known_users: std::collections::HashSet<_> = previous.iter().filter_map(|it| { let (user, value) = text(it); user.then(|| bare(value)) }).collect();
    let known_user = |s: &str| known_users.contains(&bare(s));
    for (row, line) in lines[..end].iter().enumerate() {
        if re!(r"^\s*Worked for |^\s*└ Tip: ").is_match(line) { continue; }
        if re!(r"^[•◦●○◉∙⋅\u{2800}-\u{28ff}] .*esc to interrupt|^\s*\d+ background terminals? running").is_match(line) { break; }
        if let Some(t) = line.strip_prefix("› ").or_else(|| line.strip_prefix("» ")) {
            blocks.push((true, t.to_owned()));
            // A sticky prompt may remain at row zero above a clipped assistant
            // reply, with no bullet between them. It is not a multiline prompt.
            user_complete = known_user(t) || (row == 0 && captured.get(1).is_some_and(|l| l.starts_with("\x1b[49m")));
        } else if let Some(t) = line.strip_prefix("• ") {
            blocks.push((false, t.to_owned()));
        } else if let Some((user, t)) = blocks.last_mut() {
            if *user && user_complete {
                continuation.push_str(line.strip_prefix("  ").unwrap_or(line)); continuation.push('\n');
            } else {
                t.push('\n'); t.push_str(line.strip_prefix("  ").unwrap_or(line));
                if *user { user_complete = known_user(t); }
            }
        } else {
            continuation.push_str(line.strip_prefix("  ").unwrap_or(line)); continuation.push('\n');
        }
    }
    for (_, t) in &mut blocks { *t = t.trim().to_owned(); }
    blocks.retain(|(_, t)| !t.is_empty());
    let mut out = previous.to_vec();
    if !continuation.trim().is_empty() {
        if let Some(ChatItem::Assistant { block: AssistantBlock::Text { text }, .. }) = out.last_mut() {
            if let Some(next) = extend_clipped(text, continuation.trim()) { *text = next; }
        }
    }
    // Codex redraws raw Markdown as styled text and rewraps it on completion.
    // Compare content rather than terminal formatting. Match historical windows
    // too: a repeated/older capture must update its original messages, not append.
    let old: Vec<_> = out.iter().map(|item| { let (role, s) = text(item); (role, bare(s)) }).collect();
    let new: Vec<_> = blocks.iter().map(|(role, s)| (*role, bare(s))).collect();
    let compatible = |a: &(bool, String), b: &(bool, String)| {
        a.0 == b.0 && (a.1 == b.1 || (!a.0 && same_reply(&a.1, &b.1)))
    };
    let indices = (0..out.len()).filter_map(|start| {
        if new.first().is_none_or(|b| !compatible(&old[start], b)) { return None; }
        let mut cursor = start;
        let mut indices = vec![];
        let mut matched = 0;
        for b in &new {
            // Once another turn starts, Codex can hide the previous turn's tool
            // and commentary rows. Skip those rows, never a different user turn.
            while cursor < old.len() && !compatible(&old[cursor], b) {
                if old[cursor].0 { return None; }
                cursor += 1;
            }
            if cursor < old.len() { matched += 1; }
            indices.push(cursor);
            cursor += 1;
        }
        Some((matched, start, indices))
    }).max_by_key(|(matched, start, _)| (*matched, *start)).map(|(_, _, indices)| indices)
        .unwrap_or_else(|| (out.len()..out.len()+blocks.len()).collect());
    for (i, (user, value)) in blocks.into_iter().enumerate() {
        let index = indices[i];
        if let Some(old) = out.get(index) {
            // A narrower viewport must not truncate a reply already seen in full.
            let before = bare(text(old).1);
            let now = bare(&value);
            if before.starts_with(&now) || (!user && same_reply(&before, &now) && before.len() >= now.len()) { continue; }
        }
        let seq = out.get(index).map(ChatItem::seq).unwrap_or_else(|| out.last().map_or(0, |it| it.seq()+1));
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

    #[test]
    fn markdown_redraw_and_rewrapping_do_not_duplicate_a_turn() {
        let frames = [
            screen(&["› Any ideas for a quiet afternoon?", "• Try **the beach** and [the museum]"]),
            screen(&["› Any ideas for a quiet afternoon?", "• Try the beach and the museum (https://example.org).", "  Bring a light jacket."]),
            screen(&["› Any ideas for a quiet", "  afternoon?", "• Try the beach and the museum", "  (https://example.org). Bring a light jacket."]),
        ];
        let mut items = vec![];
        for frame in frames.iter().cycle().take(30) {
            if let Some(next) = merge(&items, frame, "side") { items = next; }
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].seq(), 0);
            assert_eq!(items[1].seq(), 1);
        }
        assert!(text(&items[1]).1.contains("light jacket"));
    }

    #[test]
    fn an_incomplete_repaint_cannot_append_or_shorten_the_same_reply() {
        let opening = "• Spend the afternoon by the coast: you can fit a beach, coffee and some local history into one easy outing.";
        let full = screen(&["› Any afternoon ideas?", opening, "  1. Walk along the beach.", "  2. Stop for coffee.", "  3. Visit the museum.", "  Finish with another stroll."]);
        let partial = screen(&["› Any afternoon ideas?", opening, "  1. Walk along the beach.", "  2. Stop for coffee. Finish with another"]);
        let mut items = merge(&[], &partial, "side").unwrap();
        items = merge(&items, &full, "side").unwrap();
        for frame in [&partial, &full].into_iter().cycle().take(30) { assert!(merge(&items, frame, "side").is_none()); }
        assert_eq!(items.len(), 2);
        assert!(text(&items[1]).1.contains("3. Visit the museum."));
        assert!(text(&items[1]).1.ends_with("Finish with another stroll."));
    }

    #[test]
    fn older_viewports_and_three_turns_keep_their_original_positions() {
        let first = screen(&["› Where should I walk?", "• Follow the coast."]);
        let second = screen(&["› Where should I walk?", "• Follow the coast.", "› What about rain?", "• Visit the museum."]);
        let third = screen(&["› What about rain?", "• Visit the museum.", "› And coffee?", "• Try the cafe nearby."]);
        let mut items = merge(&[], &first, "side").unwrap();
        items = merge(&items, &second, "side").unwrap();
        items = merge(&items, &third, "side").unwrap();
        for frame in [&first, &second, &third].into_iter().cycle().take(30) {
            assert!(merge(&items, frame, "side").is_none());
        }
        assert_eq!(items.len(), 6);
        assert_eq!(text(&items[4]).1, "And coffee?");
    }

    #[test]
    fn a_new_turn_hiding_old_tool_rows_keeps_each_question_once() {
        let first = screen(&["› Where should I walk?", "• I will check the routes.", "• Browsing the web", "• Searched the web for coastal paths", "• Follow the coast."]);
        let second = screen(&["› Where should I walk?", "• Follow the coast.", "› What about rain?", "• Visit the museum."]);
        let items = merge(&[], &first, "side").unwrap();
        let next = merge(&items, &second, "side").unwrap();
        assert_eq!(next.iter().filter(|i| text(i).0).count(), 2);
        assert_eq!(next.len(), 7);
        assert!(merge(&next, &second, "side").is_none());
        assert!(merge(&next, &first, "side").is_none());
    }

    #[test]
    fn sticky_prompt_above_clipped_output_is_not_a_new_multiline_question() {
        let items = merge(&[], &screen(&["› Where should I walk?", "• Follow the coast and stop for coffee."]), "side").unwrap();
        let clipped = screen(&["› Where should I walk?", "  stop for coffee.", "", "› What about rain?", "• Visit the museum."]);
        let next = merge(&items, &clipped, "side").unwrap();
        assert_eq!(next.len(), 4);
        assert_eq!(text(&next[0]).1, "Where should I walk?");
        assert!(merge(&next, &clipped, "side").is_none());
        let styled = screen(&["\x1b[48;2;68;68;68m› Where should I walk?", "\x1b[49m  stop for coffee.", "› What about rain?", "• Visit the museum."]);
        let fresh = merge(&[], &styled, "side").unwrap();
        assert_eq!(text(&fresh[0]).1, "Where should I walk?");
    }

    #[test]
    fn clipped_reply_only_extends_a_matching_tail_and_never_truncates() {
        let first = screen(&["› Tell me about the coast.", "• Start at the quiet beach and follow the path toward the harbor."]);
        let items = merge(&[], &first, "side").unwrap();
        let clipped = screen(&["  follow the path toward the harbor. Stop for coffee afterward."]);
        let next = merge(&items, &clipped, "side").unwrap();
        assert_eq!(next.len(), 2);
        assert_eq!(text(&next[1]).1, "Start at the quiet beach and follow the path toward the harbor. Stop for coffee afterward.");
        assert!(merge(&next, &clipped, "side").is_none());
        assert!(merge(&next, &screen(&["  unrelated old text from another viewport"]), "side").is_none());
    }
}
