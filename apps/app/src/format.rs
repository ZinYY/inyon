//! Words and numbers as the overview shows them. The ones apps/web/src/lib/format.ts has come from
//! the shared view model (crates/inyon-chat-core); the rest are components/ui/status.tsx's. Time
//! comes in as arguments, so every function here is pure.

use inyon_chat_core::format as shared;
use inyon_chat_core::js::local_time;
use inyon_protocol::{RateWindow, TurnFailure};

pub use inyon_chat_core::format::{tokens, LiveWindow};

/// The local clock: now in unix ms, and the minutes local time is ahead of UTC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub now: f64,
    pub tz_minutes: i32,
}

/// "~/code/app" for a home folder, and long paths keep their first and last parts. Windows too.
pub fn short_path(p: &str, max: usize) -> String {
    shared::short_path(Some(p), max)
}

/// "38 ms", "1.4 s": how long a machine took to report its sessions.
pub fn response_time(ms: u64) -> String {
    shared::response_time(ms as f64)
}

/// "just now", "12m ago", "3h ago", then the date ("Oct 29").
pub fn time_ago(ms: f64, clock: Clock) -> String {
    shared::time_ago(ms, clock.now, clock.tz_minutes)
}

/// "$3.42", or whole dollars from $100.
pub fn money(n: f64) -> String {
    shared::money(Some(n))
}

/// "resets in 2h 10m" until a unix-seconds time.
pub fn resets_in(unix: Option<f64>, clock: Clock) -> String {
    shared::resets_in(unix, clock.now)
}

/// A plan-limit window as of now: once its reset time has passed, it shows as fresh.
pub fn live_window(w: Option<&RateWindow>, clock: Clock) -> Option<LiveWindow> {
    shared::live_window(w, clock.now)
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// "1 background task" reads "1 task running in the background"; "1 shell, 1 agent" reads
/// "1 shell and 1 agent running in the background".
pub fn background_text(bg: &str) -> String {
    let parts: Vec<String> = bg.split(", ").map(|p| p.replacen("background ", "", 1)).collect();
    let list = match parts.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.clone(),
        None => String::new(),
    };
    format!("{list} running in the background")
}

/// "Stopped: rate limit · resets 14:05", with the day when the reset is more than 20 hours away.
pub fn failure_text(f: &TurnFailure, clock: Clock) -> String {
    let what = match f.error.as_str() {
        "rate_limit" => "rate limit",
        "billing_error" => "billing problem",
        "authentication_failed" => "signed out",
        "overloaded" => "servers overloaded",
        "server_error" => "server error",
        "invalid_request" => "request refused",
        "max_output_tokens" => "reply too long",
        _ => "an error",
    };
    let words = format!("Stopped: {what}");
    let at = f.resets_at.map_or(0.0, |s| s as f64 * 1000.0);
    if at <= clock.now {
        return words;
    }
    let t = local_time(at, clock.tz_minutes);
    let day = if at - clock.now > 20.0 * 3_600_000.0 { format!("{} ", ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][t.weekday as usize]) } else { String::new() };
    format!("{words} · resets {day}{:02}:{:02}", t.hour, t.minute)
}

/// "Offline · last seen 5 min ago", for a machine that isn't answering.
pub fn last_seen(ms: u64, clock: Clock) -> String {
    let s = ((clock.now - ms as f64) / 1000.0).max(0.0);
    let ago = if s < 60.0 {
        "just now".to_string()
    } else if s < 3600.0 {
        format!("{} min ago", (s / 60.0).round())
    } else if s < 86_400.0 {
        format!("{} h ago", (s / 3600.0).round())
    } else {
        format!("{} d ago", (s / 86_400.0).round())
    };
    format!("Offline · last seen {ago}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sat 31 Oct 2026, 12:00 UTC
    const NOW: f64 = 1_793_448_000_000.0;
    const UTC: Clock = Clock { now: NOW, tz_minutes: 0 };

    #[test]
    fn paths_start_at_home_and_keep_their_ends() {
        assert_eq!(short_path("/home/sam/code/storefront", 34), "~/code/storefront");
        assert_eq!(short_path("/Users/sam", 34), "~");
        assert_eq!(short_path("/root/x", 34), "~/x");
        assert_eq!(short_path("/srv/app", 34), "/srv/app");
        assert_eq!(short_path("/home/sam/code/clients/acme/storefront/packages/checkout", 34), "~/…/storefront/packages/checkout");
        assert_eq!(short_path("C:\\Users\\sam\\demo\\api", 34), "~\\demo\\api");
        assert_eq!(short_path("/a-very-long-folder-name-that-does-not-fit-anywhere", 20), "/a-very-long-folder-name-that-does-not-fit-anywhere");
        assert_eq!(short_path("", 34), "");
    }

    #[test]
    fn times_read_like_the_web_app() {
        assert_eq!(time_ago(NOW - 10_000.0, UTC), "just now");
        assert_eq!(time_ago(NOW - 12.0 * 60_000.0, UTC), "12m ago");
        assert_eq!(time_ago(NOW - 3.0 * 3_600_000.0, UTC), "3h ago");
        assert_eq!(time_ago(NOW - 2.0 * 86_400_000.0, UTC), "Oct 29");
        assert_eq!(time_ago(0.0, UTC), "");
        assert_eq!(resets_in(Some(NOW / 1000.0 + 2.0 * 3600.0 + 900.0), UTC), "resets in 2h 15m");
        assert_eq!(resets_in(Some(NOW / 1000.0 + 3.0 * 86_400.0), UTC), "resets in 3d 0h");
        assert_eq!(resets_in(Some(NOW / 1000.0 - 1.0), UTC), "resetting");
        assert_eq!(last_seen(NOW as u64 - 5 * 60_000, UTC), "Offline · last seen 5 min ago");
    }

    #[test]
    fn local_time_follows_the_offset() {
        let berlin = Clock { now: NOW, tz_minutes: 60 };
        let f = TurnFailure { error: "rate_limit".into(), resets_at: Some((NOW / 1000.0) as u64 + 2 * 3600) };
        assert_eq!(failure_text(&f, UTC), "Stopped: rate limit · resets 14:00");
        assert_eq!(failure_text(&f, berlin), "Stopped: rate limit · resets 15:00");
        let far = TurnFailure { error: "overloaded".into(), resets_at: Some((NOW / 1000.0) as u64 + 30 * 3600) };
        assert_eq!(failure_text(&far, UTC), "Stopped: servers overloaded · resets Sun 18:00");
        assert_eq!(failure_text(&TurnFailure { error: "new_kind".into(), resets_at: None }, UTC), "Stopped: an error");
    }

    #[test]
    fn numbers() {
        assert_eq!(tokens(Some(61_200.0)), "61k");
        assert_eq!(tokens(Some(999_600.0)), "1M");
        assert_eq!(tokens(Some(1_250_000.0)), "1.3M");
        assert_eq!(tokens(Some(12_000_000.0)), "12M");
        assert_eq!(tokens(Some(512.0)), "512");
        assert_eq!(tokens(None), "—");
        assert_eq!(money(3.42), "$3.42");
        assert_eq!(money(123.4), "$123");
        assert_eq!(money(100.5), "$101");
        assert_eq!(response_time(38), "38 ms");
        assert_eq!(response_time(1400), "1.4 s");
        assert_eq!(plural(1, "pane"), "1 pane");
        assert_eq!(plural(2, "pane"), "2 panes");
    }

    #[test]
    fn background_work_in_words() {
        assert_eq!(background_text("1 shell"), "1 shell running in the background");
        assert_eq!(background_text("1 background task"), "1 task running in the background");
        assert_eq!(background_text("1 shell, 2 agents"), "1 shell and 2 agents running in the background");
    }

    #[test]
    fn a_window_past_its_reset_starts_fresh() {
        let w = inyon_protocol::RateWindow { pct: 41.0.into(), resets_at: Some((NOW / 1000.0 - 5.0).into()) };
        assert_eq!(live_window(Some(&w), UTC), Some(LiveWindow { pct: 0.0, resets_at: None, reset: true }));
        let w = inyon_protocol::RateWindow { pct: 22.0.into(), resets_at: Some((NOW / 1000.0 + 60.0).into()) };
        assert_eq!(live_window(Some(&w), UTC).unwrap().pct, 22.0);
        assert_eq!(live_window(None, UTC), None);
    }
}
