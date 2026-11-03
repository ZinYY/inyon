//! Port of hosts/turns.ts.

use crate::re;
use std::collections::HashMap;

use crate::js::json::Js;
use crate::tmux::tmux::HookPaneState;

/// What a pane shows: waiting, working or idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Waiting,
    Working,
    Idle,
}

/// The longest a turn is shown as working on the hooks' and an answer's word alone.
const MAX_MS: u64 = 15 * 60_000;
/// A working Claude redraws every second (the spinner, the elapsed time); a screen that hasn't
/// changed this long is an agent that stopped, whatever the last signal said.
const STILL_MS: u64 = 8_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Said {
    Busy,
    Done,
}

/// Hook events that start or continue a turn; Stop (and a new session) end it. Another agent's
/// events count in Claude Code's words (see `HookPaneState::event_name`).
fn hook_says(hook: &HookPaneState) -> Option<Said> {
    let h = Some(&hook.data);
    match hook.event_name() {
        "UserPromptSubmit" | "PermissionRequest" => Some(Said::Busy),
        "Stop" | "StopFailure" | "SessionStart" | "SessionEnd" => Some(Said::Done),
        // "Claude is waiting for your input" comes after a minute idle; the rest ask for something mid-turn
        "Notification" => Some(if h.at("notification_type").str() == Some("idle_prompt") || re!(r"waiting for your input", "i").is_match(h.at("message").str().unwrap_or("")) {
            Said::Done
        } else {
            Said::Busy
        }),
        _ => None,
    }
}

struct Turn {
    since: u64,
    /// the screen showed Claude working during this turn, so its idle can be believed
    seen_working: bool,
    screen: Option<String>,
    same_since: u64,
}

/// Claude's turns as the hub knows them besides the screen. Right after a prompt is answered, and
/// while Claude runs a long command on some terminals (psmux), its screen can read as idle although
/// the turn goes on. An answer from Inyon, or a hook event that starts a turn, holds the session
/// at working until a signal says the turn ended: the Stop hook, a working screen that went idle,
/// a screen that stopped changing, or a long time.
#[derive(Default)]
pub struct Turns {
    turns: HashMap<String, Turn>,
    /// when each pane's last turn ended, so the hook event that started it doesn't start it again
    ended: HashMap<String, u64>,
}

impl Turns {
    pub fn new() -> Self {
        Self::default()
    }

    /// A prompt was answered (or a message sent) from Inyon: the turn goes on.
    pub fn start(&mut self, key: &str, now: u64) {
        self.turns.insert(key.to_owned(), Turn { since: now, seen_working: false, screen: None, same_since: now });
    }

    pub fn end(&mut self, key: &str, now: u64) {
        self.turns.remove(key);
        self.ended.insert(key.to_owned(), now);
    }

    pub fn forget(&mut self, key: &str) {
        self.turns.remove(key);
        self.ended.remove(key);
    }

    /// What to show for a pane whose screen reads `shown`, given its last hook event and screen text.
    pub fn state(&mut self, key: &str, shown: Shown, screen: &str, hook: Option<&HookPaneState>, now: u64) -> Shown {
        let said = hook.and_then(hook_says);
        let at = hook.map_or(0.0, |h| h.ts) * 1000.0;
        let ended = self.ended.get(key).copied().unwrap_or(0) as f64;
        if said == Some(Said::Busy) && !self.turns.contains_key(key) && at > ended && (now as f64) - at < MAX_MS as f64 {
            self.start(key, at as u64);
        }
        let Some(turn) = self.turns.get_mut(key) else { return shown };
        // (hook times are whole seconds)
        if (said == Some(Said::Done) && at >= ((turn.since / 1000) * 1000) as f64) || now.saturating_sub(turn.since) > MAX_MS {
            self.end(key, now);
            return shown;
        }
        // a question mid-turn doesn't end it: once it's answered the turn carries on
        match shown {
            Shown::Waiting => {
                turn.screen = None;
                return shown;
            }
            Shown::Working => {
                turn.seen_working = true;
                turn.screen = None;
                return shown;
            }
            Shown::Idle => {}
        }
        if turn.seen_working {
            self.end(key, now);
            return shown;
        }
        if turn.screen.as_deref() != Some(screen) {
            turn.screen = Some(screen.to_owned());
            turn.same_since = now;
        } else if now.saturating_sub(turn.same_since) >= STILL_MS {
            self.end(key, now);
            return shown;
        }
        Shown::Working
    }
}

#[cfg(test)]
#[path = "turns_tests.rs"]
mod tests;
