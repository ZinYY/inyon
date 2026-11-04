//! Local MessageDisplay hook records, not browser protocol messages.
use serde::{Deserialize, Serialize};

pub const MAX_TEXT: usize = 1024 * 1024;

pub fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().enumerate().all(|(i, b)| {
        if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() }
    })
}

#[derive(Debug, Deserialize)]
pub struct DisplayEvent {
    pub hook_event_name: String,
    pub session_id: String,
    pub turn_id: String,
    pub message_id: String,
    pub index: u64,
    pub delta: String,
    #[serde(rename = "final")]
    pub finished: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplaySnapshot {
    pub session_id: String,
    pub turn_id: String,
    pub message_id: String,
    pub index: u64,
    pub text: String,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished: bool,
    pub valid: bool,
}

impl DisplaySnapshot {
    /// A missing/out-of-order batch fails closed to the completed transcript.
    /// Replayed batches never append twice; another message must begin at index zero.
    pub fn apply(previous: Option<Self>, event: DisplayEvent, now: u64) -> Option<Self> {
        if event.hook_event_name != "MessageDisplay" || !valid_id(&event.session_id)
            || !valid_id(&event.turn_id) || !valid_id(&event.message_id) || event.delta.len() > MAX_TEXT { return previous; }
        let same = previous.as_ref().is_some_and(|p| p.session_id == event.session_id && p.turn_id == event.turn_id && p.message_id == event.message_id);
        let mut value = if same {
            let mut p = previous.unwrap();
            if event.index <= p.index || p.finished { return Some(p); }
            p.valid &= event.index == p.index + 1;
            p
        } else {
            if event.index != 0 { return previous; }
            Self { session_id: event.session_id, turn_id: event.turn_id, message_id: event.message_id,
                index: 0, text: String::new(), started_at: now, updated_at: now, finished: false, valid: true }
        };
        value.valid &= value.text.len().saturating_add(event.delta.len()) <= MAX_TEXT;
        if value.valid { value.text.push_str(&event.delta); } else { value.text.clear(); }
        value.index = event.index;
        value.updated_at = now;
        value.finished = event.finished;
        Some(value)
    }
}
