//! Sends that can't be read back off an input box, and the record of sends that failed.
//!
//! An agent whose screen Inyon has no parser for still gets no Enter until what was typed shows
//! on screen: its last letters and digits appear once more than before, or a paste preview does.
//! A failed send is kept with the bottom of the screen it failed on, so "it didn't arrive" can be
//! looked at afterwards instead of guessed at. The message itself isn't kept, as with receipts.

use serde_json::{json, Value};

use crate::clock::now_ms;
use crate::platform::store::kv;
use crate::re;

/// How many letters and digits from the end of a message identify it on screen.
const TAIL: usize = 16;
/// Below this many, a message ("ok", "?") can't be told apart from the rest of the screen.
const SHORTEST: usize = 4;
/// Failed sends kept, newest last.
const KEEP: usize = 20;
/// Lines of screen kept with each.
const SCREEN_LINES: usize = 30;
pub const LOG_KEY: &str = "send-failures";

/// Letters and digits only: what survives the TUI's box borders, wrapping and markers.
fn plain(s: &str) -> Vec<char> {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn count(hay: &[char], needle: &[char]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    let mut n = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            n += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    n
}

/// How often `text` shows on the screen `lines`: its tail, plus paste previews ("[Pasted text
/// #1]", "[Pasted Content 120 chars]", Gemini's "[Pasted Text: 5 lines]"). None when the message
/// has too few letters and digits to be recognised.
pub fn marks(lines: &[String], text: &str) -> Option<usize> {
    let mine = plain(text);
    if mine.len() < SHORTEST {
        return None;
    }
    let tail = &mine[mine.len().saturating_sub(TAIL)..];
    let screen = lines.join("\n");
    let previews = re!(r"\[Pasted\b", "gi").find_iter(&screen).count();
    Some(count(&plain(&screen), tail) + previews)
}

/// Whether what was typed has arrived: it shows once more than before typing. A message too short
/// to recognise has arrived once the screen changed at all.
pub fn arrived(before: &[String], now: &[String], text: &str) -> bool {
    match (marks(before, text), marks(now, text)) {
        (Some(b), Some(n)) => n > b,
        _ => before != now,
    }
}

/// Keeps a failed send: when, where, which agent, why, how long the message was, and the bottom
/// of the screen.
pub fn record_failure(host_id: &str, pane_id: &str, agent: Option<&str>, error: &str, text: &str, screen: &[String]) {
    let mut list = match kv::get_value(LOG_KEY) {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    let bottom: Vec<&str> = {
        let mut end = screen.len();
        while end > 0 && screen[end - 1].trim().is_empty() {
            end -= 1;
        }
        screen[end.saturating_sub(SCREEN_LINES)..end].iter().map(|l| l.trim_end()).collect()
    };
    list.push(json!({
        "at": now_ms(),
        "host": host_id,
        "pane": pane_id,
        "agent": agent,
        "error": error,
        "chars": text.chars().count(),
        "screen": bottom,
    }));
    let drop = list.len().saturating_sub(KEEP);
    list.drain(..drop);
    if let Err(e) = kv::set_value(LOG_KEY, &Value::Array(list)) {
        crate::platform::console::error(&format!("couldn't keep the failed send: {e}"));
    }
}

/// The failed sends kept, newest first.
pub fn failures() -> Vec<Value> {
    match kv::get_value(LOG_KEY) {
        Some(Value::Array(mut a)) => {
            a.reverse();
            a
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn a_message_has_arrived_when_its_end_shows_once_more_than_before() {
        let before = lines(&["│ > Type your message                     │", "╰──────────────────────────────────────╯"]);
        let text = "Rename the helper to parse_reply and update its callers";
        let typed = lines(&["│ > Rename the helper to parse_reply and   │", "│ update its callers                       │", "╰──────────────────────────────────────╯"]);
        assert!(arrived(&before, &typed, text));
        // half typed: the end isn't there yet, so no Enter
        let half = lines(&["│ > Rename the helper to parse_reply and   │"]);
        assert!(!arrived(&before, &half, text));
        // the same message sent earlier is still in the scrollback: one more copy is needed
        let earlier = lines(&["> Rename the helper to parse_reply and update its callers", "✦ Done.", "│ >                                  │"]);
        assert!(!arrived(&earlier, &earlier, text));
        let again = lines(&["> Rename the helper to parse_reply and update its callers", "✦ Done.", "│ > Rename the helper to parse_reply and update its callers │"]);
        assert!(arrived(&earlier, &again, text));
    }

    #[test]
    fn a_paste_preview_counts_and_a_short_message_needs_only_a_change() {
        let empty = lines(&["│ >                │"]);
        assert!(arrived(&empty, &lines(&["│ > [Pasted Text: 12 lines] │"]), &"line\n".repeat(12)));
        assert!(arrived(&empty, &lines(&["│ > ok │"]), "ok"));
        assert!(!arrived(&empty, &empty, "ok"));
        assert_eq!(marks(&empty, "?"), None);
    }

    #[test]
    fn failed_sends_keep_the_bottom_of_the_screen_and_not_the_message() {
        crate::test_support::rt::run(|| async {
            crate::platform::store::use_store(std::rc::Rc::new(crate::platform::store::MemoryStore::new()));
            let screen: Vec<String> = (0..50).map(|i| format!("line {i}   ")).chain(["".to_string(), "  ".to_string()]).collect();
            for i in 0..25 {
                record_failure("h1", "%3", Some("gemini"), &format!("failure {i}"), "a secret plan", &screen);
            }
            let kept = failures();
            assert_eq!(kept.len(), KEEP);
            assert_eq!(kept[0]["error"], "failure 24");
            assert_eq!(kept[0]["chars"], 13);
            let shown = kept[0]["screen"].as_array().unwrap();
            assert_eq!(shown.len(), SCREEN_LINES);
            assert_eq!(shown.last().unwrap(), "line 49");
            assert!(!serde_json::to_string(&kept).unwrap().contains("secret"));
        });
    }
}
