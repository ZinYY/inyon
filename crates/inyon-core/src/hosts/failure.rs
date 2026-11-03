//! A turn that ended on an error instead of finishing: Claude Code's StopFailure hook. The pane
//! shows why, and the alert says "Claude stopped: rate limit, resets in 2 h 10 min" rather than
//! "Claude finished".

use inyon_protocol::{RateWindow, TurnFailure};

use crate::js::json::Js;
use crate::tmux::tmux::HookPaneState;

/// Why the turn stopped, when the pane's last hook event is a StopFailure. A rate limit carries
/// when it resets: the latest reset among the limits that are full (`windows`: the session's own
/// numbers and the account's), since the agent can't go on until all of them have.
pub fn turn_failure(hook: &HookPaneState, windows: &[&RateWindow], now_s: f64) -> Option<TurnFailure> {
    let data = Some(&hook.data);
    if data.at("hook_event_name").str() != Some("StopFailure") {
        return None;
    }
    let error = data.at("error").str().filter(|e| !e.is_empty()).unwrap_or("unknown").to_owned();
    let resets_at = if error == "rate_limit" {
        windows.iter().filter(|w| *w.pct >= 100.0).filter_map(|w| w.resets_at.map(|r| *r)).filter(|r| r.is_finite() && *r > now_s).map(|r| r as u64).max()
    } else {
        None
    };
    Some(TurnFailure { error, resets_at })
}

/// The error in a few words, as the app says it.
pub fn failure_words(error: &str) -> &'static str {
    match error {
        "rate_limit" => "rate limit",
        "billing_error" => "billing problem",
        "authentication_failed" => "signed out",
        "overloaded" => "servers overloaded",
        "server_error" => "server error",
        "invalid_request" => "request refused",
        "max_output_tokens" => "reply too long",
        _ => "an error",
    }
}

/// "rate limit, resets in 2 h 10 min", for an alert, which is read wherever the phone is: a
/// time from now, since the hub doesn't know the phone's clock zone.
pub fn failure_text(f: &TurnFailure, now_s: f64) -> String {
    let words = failure_words(&f.error);
    match f.resets_at.map(|r| r as f64 - now_s).filter(|s| *s > 0.0) {
        Some(s) => format!("{words}, resets in {}", from_now(s)),
        None => words.to_owned(),
    }
}

fn from_now(secs: f64) -> String {
    let min = (secs / 60.0).ceil() as u64;
    let (d, h, m) = (min / 1440, min % 1440 / 60, min % 60);
    match (d, h, m) {
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (1, 0, _) => "1 day".to_owned(),
        (1, h, _) => format!("1 day {h} h"),
        (d, _, _) => format!("{d} days"),
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
