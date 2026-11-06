//! Port of claude/delivery.ts: whether text reached the agent (in its input box, its queue or its
//! record of the conversation), and the ledger of what the hub sent to each pane.

use std::collections::HashSet;

use inyon_protocol::{ChatItem, ScreenState, SendState, SendStatus};

use crate::clock::now_ms;
use crate::js::text::squash;
use crate::js::JsStr;
use crate::re;

/// `text.replace(/\s+/g, ' ').trim()`
fn collapse(text: &str) -> String {
    re!(r"\s+", "g").replace_all(text, " ").jtrim().to_owned()
}

/// A terminal paste preview is not the underlying editable text.
pub fn has_paste_preview(text: &str) -> bool {
    re!(r"\[Pasted (?:text #\d+|Content \d+ chars\]|~\d+ lines?\])", "i").is_match(text)
}

/// Whether the input box holds `text`. `known` says the box was just given this text (typed into
/// an emptied box, or what the draft mirror last put there). Only then do previews count: Claude
/// collapses a long paste to "[Pasted text #1]", Codex to a character count and OpenCode to a line
/// count, and an older draft can show the same preview. And only then is whitespace ignored, which
/// the terminal's wrapping changes: otherwise a draft differing only in spaces or line breaks
/// (inside a string, say) would be taken for the message, and Enter would send the old one.
pub fn input_contains(draft: Option<&str>, text: &str, known: bool) -> bool {
    let trimmed = draft.unwrap_or("").jtrim();
    if re!(r"\[Pasted\s+Content\s+\d+\s+chars\]", "i").is_match(trimmed) {
        return known && codex_paste_matches(trimmed, text, true);
    }
    // OpenCode collapses a multi-line paste to its line count
    if let Some(lines) = re!(r"^\[Pasted ~(\d+) lines?\]$", "i").captures(trimmed) {
        return known && crate::js::to_number(&lines[1]) == re!(r"\r\n?", "g").replace_all(text, "\n").split('\n').count() as f64;
    }
    // Codex loads image paths while typing and renders numbered attachment tokens instead.
    // Compare the entire body and ordered attachment list, not a substring or similar length.
    let mut image = 0;
    let rendered = re!(r"(Attached files:\s*)([\s\S]*)$").replacen(text, 1, |c: &regex::Captures| {
        let files = c[2]
            .split('\n')
            .map(|path| {
                if re!(r"^\/[^\n]+\.(png|jpe?g|gif|webp)$", "i").is_match(path.jtrim()) {
                    image += 1;
                    format!("[Image #{image}]")
                } else {
                    path.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("{}{files}", &c[1])
    });
    let same = |s: &str| if known { squash(s) } else { collapse(s) };
    let value = same(draft.unwrap_or(""));
    value == same(text) || image > 0 && value == same(&rendered) || known && re!(r"^(\[Pastedtext#\d+(\+\d+lines?)?\])+$").is_match(&value)
}

/// Windows can split one paste into several collapsed spans with literal text between them.
/// Check every visible character and every span's length against the text we just pasted.
/// Whitespace around spans is ambiguous after terminal wrapping, so retain those few positions.
fn codex_paste_matches(draft: &str, text: &str, whole: bool) -> bool {
    let spans = re!(r"\[Pasted\s+Content\s+(\d+)\s+chars\]", "i");
    for utf16 in [false, true] {
        let chars: Vec<char> = text.chars().collect();
        let mut positions = vec![0usize];
        let mut end = 0;
        for span in spans.captures_iter(draft) {
            let m = span.get(0).unwrap();
            positions = match_literal(&draft[end..m.start()], &chars, positions);
            let n = span[1].parse::<usize>().unwrap_or(usize::MAX);
            let mut next = Vec::new();
            for mut start in positions {
                loop {
                    let mut at = start;
                    let mut count = 0usize;
                    while at < chars.len() && count < n {
                        count += if utf16 { chars[at].len_utf16() } else { 1 };
                        at += 1;
                    }
                    if count == n { next.push(at); }
                    if start == chars.len() || !chars[start].is_whitespace() { break; }
                    start += 1;
                }
            }
            next.sort_unstable(); next.dedup();
            positions = next;
            end = m.end();
        }
        let positions = match_literal(&draft[end..], &chars, positions);
        if positions.iter().any(|&at| !whole || chars[at..].iter().all(|c| c.is_whitespace())) { return true; }
    }
    false
}

fn match_literal(literal: &str, text: &[char], positions: Vec<usize>) -> Vec<usize> {
    positions.into_iter().filter_map(|mut at| {
        for c in literal.chars().filter(|c| !c.is_whitespace()) {
            while at < text.len() && text[at].is_whitespace() { at += 1; }
            if text.get(at) != Some(&c) { return None; }
            at += 1;
        }
        Some(at)
    }).collect()
}

/// A scrolled composer can confirm a paste this operation just placed in an emptied box,
/// but never an unknown older draft. A paste leaves the cursor at the end, so only a visible
/// suffix with no rows hidden below it is evidence; an arbitrary middle fragment is not.
pub fn input_shown(screen: &ScreenState, text: &str, known: bool) -> bool {
    if screen.draft_clipped.is_none() {
        return input_contains(screen.draft.as_deref(), text, known);
    }
    if !known || screen.draft_clipped.as_deref() != Some("start") { return false; }
    let draft = screen.draft.as_deref().unwrap_or("");
    let spans = re!(r"\[Pasted\s+Content\s+\d+\s+chars\]", "i");
    if let Some(last) = spans.find_iter(draft).last() {
        // A final folded block could still be an intermediate chunk. Require a literal ending
        // before trusting a clipped preview; its hidden span lengths alone do not prove arrival.
        if draft[last.end()..].chars().filter(|c| c.is_alphanumeric()).count() < 16 { return false; }
        let mut pieces = Vec::new();
        let mut end = 0;
        for span in spans.find_iter(draft) {
            pieces.push(draft[end..span.start()].chars().rev().collect::<String>());
            pieces.push(span.as_str().to_owned());
            end = span.end();
        }
        pieces.push(draft[end..].chars().rev().collect());
        pieces.reverse();
        return codex_paste_matches(&pieces.concat(), &text.chars().rev().collect::<String>(), false);
    }
    let tail = squash(draft);
    tail.chars().filter(|c| c.is_alphanumeric()).count() >= 16 && squash(text).ends_with(&tail)
}

/// A missing composer is unknown, rather than evidence that input was submitted.
pub fn codex_accepted(screen: &ScreenState, text: &str) -> bool {
    (screen.model.as_deref().is_some_and(|m| !m.is_empty()) || screen.input_visible == Some(true))
        && screen.prompt.is_none()
        && screen.draft_clipped.is_none()
        && screen.draft.as_deref().is_none_or(|d| d.jtrim().is_empty())
        || screen.queue.as_ref().is_some_and(|q| q.iter().any(|queued| input_contains(Some(queued), text, true)))
}

/// Words and their order only: agents rewrap pastes, drop markup and swap quotes.
fn loose_text(s: &str) -> String {
    let s = codex_images(s);
    let s = re!(r"<\/?pasted_content\b[^>]*>", "g").replace_all(&s, "");
    re!(r"[^\p{L}\p{N}!/]+", "gu").replace_all(&s.to_lowercase(), "").into_owned()
}

/// Codex records attached images as envelopes and numbered labels; put the paths back.
fn codex_images(s: &str) -> String {
    let mut images: Vec<(String, String)> = vec![];
    let s = re!(r#"<image\s+name=(\[Image #\d+\])\s+path="([^"]+)">[\s\S]*?<\/image>"#, "g")
        .replace_all(s, |c: &regex::Captures| {
            let name = c[1].to_owned();
            match images.iter_mut().find(|(n, _)| *n == name) {
                Some(e) => e.1 = c[2].to_owned(),
                None => images.push((name, c[2].to_owned())),
            }
            String::new()
        })
        .into_owned();
    if images.is_empty() {
        return s;
    }
    re!(r"(Attached files:\s*)([\s\S]*)$")
        .replacen(&s, 1, |c: &regex::Captures| {
            let files =
                re!(r"\[Image #\d+\]", "g").replace_all(&c[2], |m: &regex::Captures| images.iter().find(|(n, _)| *n == m[0]).map_or_else(|| m[0].to_owned(), |e| e.1.clone()));
            format!("{}{files}", &c[1])
        })
        .into_owned()
}

/// Whether the agent's own record of a message is the one that was sent (long ones get cut short).
pub fn same_message(recorded: &str, sent: &str) -> bool {
    let a = loose_text(recorded);
    let b = loose_text(sent);
    // a message with no words ("?", "...") would match anything else without words, including the
    // stand-in for items that aren't messages: then the text itself has to agree
    if a.is_empty() || b.is_empty() {
        return !squash(recorded).is_empty() && squash(recorded) == squash(sent);
    }
    a == b || (a.len16() >= 2000 && b.starts_with(&a))
}

/// What the agent recorded for a conversation item, in the form it was typed.
fn typed_as(i: &ChatItem) -> Option<String> {
    match i {
        ChatItem::User { text, shell, .. } => Some(if *shell == Some(true) { format!("!{text}") } else { text.clone() }),
        ChatItem::Command { name, args, .. } if !name.is_empty() => Some(format!("/{} {args}", re!(r"^\/+").replacen(name, 1, ""))),
        _ => None,
    }
}

const KEEP_MS: u64 = 10 * 60_000;
const SCAN: usize = 400;

fn recent(items: &[ChatItem]) -> &[ChatItem] {
    &items[items.len().saturating_sub(SCAN)..]
}

/// The hub's record of what it sent to one pane. Each send is matched to the first conversation
/// item that wasn't there when it started and says the same thing, so clocks and look-alike older
/// messages don't matter, and two identical messages claim two items.
#[derive(Debug, Clone, Default)]
pub struct SendLedger {
    sends: Vec<(SendState, HashSet<String>)>,
}

impl SendLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track(&mut self, id: &str, text: &str, items: &[ChatItem], setting: bool) {
        self.sends.retain(|(s, _)| s.id != id);
        let known = recent(items).iter().filter(|i| typed_as(i).is_some()).map(|i| i.uuid().to_owned()).collect();
        let state = SendState { id: id.into(), text: text.into(), at: now_ms(), state: SendStatus::Sending, uuid: None, error: None, setting: setting.then_some(true) };
        self.sends.push((state, known));
    }

    /// `settle(id, { state, error })`: a field left as None isn't in the patch.
    pub fn settle(&mut self, id: &str, state: Option<SendStatus>, error: Option<String>) {
        let Some((s, _)) = self.sends.iter_mut().find(|(x, _)| x.id == id) else { return };
        // a late record already proved delivery; a timeout can't take that back
        if s.state == SendStatus::Delivered && state != Some(SendStatus::Delivered) {
            return;
        }
        if let Some(state) = state {
            s.state = state;
        }
        if error.is_some() {
            s.error = error;
        }
        if state != Some(SendStatus::Failed) {
            s.error = None;
        }
    }

    pub fn get(&self, id: &str) -> Option<SendState> {
        self.sends.iter().find(|(x, _)| x.id == id).map(|(s, _)| s.clone())
    }

    /// Matches open sends against the conversation and the agent's queue; true if anything moved.
    pub fn match_items(&mut self, items: &[ChatItem], queue: &[String]) -> bool {
        let mut changed = false;
        let mut claimed: HashSet<String> = self.sends.iter().filter_map(|(s, _)| s.uuid.clone()).collect();
        let recent = recent(items);
        // An earlier attempt that failed before Enter must not steal a retry's receipt.
        // Keep FIFO among live sends, and still allow late receipts to recover old failures.
        let mut order: Vec<usize> = (0..self.sends.len()).collect();
        order.sort_by_key(|&i| self.sends[i].0.state == SendStatus::Failed);
        for i in order {
            let (s, known) = &mut self.sends[i];
            if s.state == SendStatus::Delivered {
                continue;
            }
            let hit = recent.iter().find(|i| !known.contains(i.uuid()) && !claimed.contains(i.uuid()) && same_message(&typed_as(i).unwrap_or_else(|| "\0".into()), &s.text));
            if let Some(hit) = hit {
                s.state = SendStatus::Delivered;
                s.uuid = Some(hit.uuid().to_owned());
                s.error = None;
                claimed.insert(hit.uuid().to_owned());
                changed = true;
            } else if matches!(s.state, SendStatus::Sending | SendStatus::Sent) && queue.iter().any(|q| same_message(q, &s.text)) {
                s.state = SendStatus::Queued;
                changed = true;
            }
        }
        let old = now_ms().saturating_sub(KEEP_MS);
        let before = self.sends.len();
        self.sends.retain(|(s, _)| s.at >= old || s.state == SendStatus::Sending);
        let excess = self.sends.len().saturating_sub(20);
        self.sends.drain(..excess);
        changed || self.sends.len() != before
    }

    pub fn list(&self) -> Vec<SendState> {
        self.sends.iter().map(|(s, _)| s.clone()).collect()
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
