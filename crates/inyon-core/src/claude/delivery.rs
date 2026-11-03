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
    if let Some(preview) = re!(r"^\[Pasted Content (\d+) chars\]$", "i").captures(trimmed) {
        let n = crate::js::to_number(&preview[1]);
        return known && (n == text.chars().count() as f64 || n == text.len16() as f64);
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

/// A missing composer is unknown, rather than evidence that input was submitted.
pub fn codex_accepted(screen: &ScreenState, text: &str) -> bool {
    (screen.model.as_deref().is_some_and(|m| !m.is_empty()) || screen.input_visible == Some(true))
        && screen.prompt.is_none()
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
        for (s, known) in &mut self.sends {
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
