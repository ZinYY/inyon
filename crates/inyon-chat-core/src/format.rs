//! `format.ts`: paths, durations, times, token counts and money as the app prints them.
//!
//! The TS reads the clock and the locale itself. Here the time is passed in (`now`, unix ms) and
//! the local clock is a UTC offset in minutes east (the opposite sign of `getTimezoneOffset()`);
//! dates print the way an en-US browser prints them.

use inyon_protocol::RateWindow;

use crate::js::{js_round, len16, local_time, parse_date, to_fixed};

/// A time as the TS functions take it: an ISO string or unix milliseconds.
pub trait Moment {
    /// unix ms, NaN when it isn't a time
    fn ms(&self) -> f64;
}

impl Moment for &str {
    fn ms(&self) -> f64 {
        parse_date(self)
    }
}

impl Moment for String {
    fn ms(&self) -> f64 {
        parse_date(self)
    }
}

impl Moment for f64 {
    fn ms(&self) -> f64 {
        *self
    }
}

impl Moment for u64 {
    fn ms(&self) -> f64 {
        *self as f64
    }
}

impl<T: Moment> Moment for Option<T> {
    fn ms(&self) -> f64 {
        self.as_ref().map_or(f64::NAN, Moment::ms)
    }
}

/// JavaScript's `!t` for a time: 0 and NaN are no time.
fn blank(t: f64) -> bool {
    t == 0.0 || t.is_nan()
}

/// `String(n)` for the whole and simple numbers these print.
fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The separator a path uses: a Windows path (C:\Users\me) uses backslashes.
fn sep_of(p: &str) -> char {
    if p.contains('\\') && !p.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// `p` with its home folder (/home/me, /Users/me, /root, C:\Users\me) as ~.
fn tilde(p: &str, sep: char) -> String {
    let seg_end = |rest: &str, sep: char| rest.find(sep).unwrap_or(rest.len());
    if sep == '/' {
        for prefix in ["/home/", "/Users/"] {
            if let Some(rest) = p.strip_prefix(prefix) {
                let end = seg_end(rest, '/');
                if end > 0 {
                    return format!("~{}", &rest[end..]);
                }
            }
        }
        if let Some(rest) = p.strip_prefix("/root") {
            if rest.is_empty() || rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
        return p.to_string();
    }
    let b = p.as_bytes();
    if b.len() > 9 && b[0].is_ascii_alphabetic() && &b[1..3] == b":\\" && b[3..8].eq_ignore_ascii_case(b"users") && b[8] == b'\\' {
        let rest = &p[9..];
        let end = seg_end(rest, '\\');
        if end > 0 {
            return format!("~{}", &rest[end..]);
        }
    }
    p.to_string()
}

/// Shortens an absolute path: home → ~, long paths keep the last segments. Windows paths too.
pub fn short_path(p: Option<&str>, max: usize) -> String {
    let Some(p) = p.filter(|p| !p.is_empty()) else { return String::new() };
    let sep = sep_of(p);
    let s = tilde(p, sep);
    if len16(&s) <= max {
        return s;
    }
    let mut parts: Vec<&str> = s.split(sep).collect();
    let n = parts.len();
    let joined_len = |parts: &[&str]| len16(&parts.join(&sep.to_string())) as i64;
    while parts.len() > 2 && joined_len(&parts) > max as i64 - 2 {
        parts.remove(1);
    }
    // nothing to drop (one long name): show it whole rather than a "…" that hides nothing
    if parts.len() < n {
        format!("{}{sep}…{sep}{}", parts[0], parts[1..].join(&sep.to_string()))
    } else {
        s
    }
}

/// A long folder as its last two parts ("…/qa/oc1", "…\demo\api"), for a small chip.
pub fn tail_path(p: &str, max: usize) -> String {
    if len16(p) <= max {
        return p.to_string();
    }
    let sep = sep_of(p);
    let parts: Vec<&str> = p.split(sep).filter(|x| !x.is_empty()).collect();
    if parts.len() > 2 {
        format!("…{sep}{}", parts[parts.len() - 2..].join(&sep.to_string()))
    } else {
        p.to_string()
    }
}

/// How long a machine took to answer its last status check: "38 ms", "1.4 s". It's the time the
/// check takes to run there and come back, not the network's round trip.
pub const RESPONSE_TITLE: &str = "How long the machine took to report its sessions at the last check";

pub fn response_time(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{} ms", num(js_round(ms)))
    } else {
        format!("{} s", to_fixed(ms / 1000.0, 1))
    }
}

pub fn basename(p: Option<&str>) -> String {
    let p = p.unwrap_or("");
    let last = p.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if !last.is_empty() {
        last.to_string()
    } else {
        p.to_string()
    }
}

pub fn duration(ms: f64) -> String {
    let s = js_round(ms / 1000.0) as i64;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m {}s", s % 60);
    }
    format!("{}h {}m", m / 60, m % 60)
}

pub fn time_ago(ts: impl Moment, now: f64, offset_min: i32) -> String {
    let t = ts.ms();
    if blank(t) {
        return String::new();
    }
    let s = ((now - t) / 1000.0).max(0.0);
    if s < 45.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{}m ago", num(js_round(s / 60.0)))
    } else if s < 86400.0 {
        format!("{}h ago", num(js_round(s / 3600.0)))
    } else {
        local_time(t, offset_min).month_day()
    }
}

pub fn clock(ts: &str, offset_min: i32) -> String {
    let t = parse_date(ts);
    if blank(t) {
        String::new()
    } else {
        local_time(t, offset_min).clock()
    }
}

/// The family, major and minor of the first `claude-<family>-<major>[-<minor>]` in `id`, as
/// `claude-([a-z]+)-([0-9]+)(?:-([0-9]{1,2}))?(?:[-\[]|$)` finds it (by hand, to keep regex out
/// of the app's wasm).
fn claude_model(id: &str) -> Option<(&str, &str, Option<&str>)> {
    let run = |at: usize, ok: fn(&u8) -> bool| at + id.as_bytes()[at..].iter().take_while(|b| ok(b)).count();
    // a model name ends here: at the end, or at '-' or '['
    let ends = |at: usize| matches!(id.as_bytes().get(at), None | Some(b'-' | b'['));
    let mut from = 0;
    while let Some(n) = id[from..].find("claude-") {
        let start = from + n + "claude-".len();
        from = from + n + 1;
        let family_end = run(start, u8::is_ascii_lowercase);
        if family_end == start || id.as_bytes().get(family_end) != Some(&b'-') {
            continue;
        }
        let major_end = run(family_end + 1, u8::is_ascii_digit);
        if major_end == family_end + 1 {
            continue;
        }
        let (family, major) = (&id[start..family_end], &id[family_end + 1..major_end]);
        // a minor of one or two digits, when a model name ends after it
        if id.as_bytes().get(major_end) == Some(&b'-') {
            let minor_end = run(major_end + 1, u8::is_ascii_digit);
            for end in (major_end + 2..=minor_end.min(major_end + 3)).rev() {
                if ends(end) {
                    return Some((family, major, Some(&id[major_end + 1..end])));
                }
            }
        }
        if ends(major_end) {
            return Some((family, major, None));
        }
    }
    None
}

pub fn model_name(id: Option<&str>) -> String {
    let Some(id) = id.filter(|id| !id.is_empty()) else { return String::new() };
    // "claude-opus-4-6[1m]" is the 1M-context variant of the same model
    let Some((family, major, minor)) = claude_model(id) else { return id.to_string() };
    let minor = minor.map(|m| format!(".{m}")).unwrap_or_default();
    format!("{}{} {major}{minor}", family[..1].to_uppercase(), &family[1..])
}

/// "%12" → "12" for display
pub fn pane_label(id: &str) -> &str {
    id.strip_prefix('%').unwrap_or(id)
}

pub fn tokens(n: Option<f64>) -> String {
    let Some(n) = n.filter(|n| !n.is_nan()) else { return "—".into() };
    // 999.5k and up would otherwise read "1000k"
    if n >= 999_500.0 {
        let s = to_fixed(n / 1_000_000.0, if n >= 10_000_000.0 { 0 } else { 1 });
        return format!("{}M", s.strip_suffix(".0").unwrap_or(&s));
    }
    if n >= 1000.0 {
        return format!("{}k", num(js_round(n / 1000.0)));
    }
    num(n)
}

pub fn bytes(n: f64) -> String {
    const K: f64 = 1024.0;
    if n >= K * K * K {
        format!("{} GB", to_fixed(n / (K * K * K), 1))
    } else if n >= K * K {
        format!("{} MB", to_fixed(n / (K * K), if n >= 100.0 * K * K { 0 } else { 1 }))
    } else if n >= K {
        format!("{} KB", num(js_round(n / K)))
    } else {
        format!("{} B", num(n))
    }
}

pub fn money(n: Option<f64>) -> String {
    match n {
        None => "—".into(),
        Some(n) if n >= 100.0 => format!("${}", to_fixed(n, 0)),
        Some(n) => format!("${}", to_fixed(n, 2)),
    }
}

/// A plan-limit window as of now. Once its reset time passes, the old percentage is history: show
/// it as fresh until the agent reports the new window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveWindow {
    pub pct: f64,
    /// unix seconds
    pub resets_at: Option<f64>,
    /// the window reset since it was reported
    pub reset: bool,
}

pub fn live_window(w: Option<&RateWindow>, now: f64) -> Option<LiveWindow> {
    let w = w?;
    let resets_at = w.resets_at.map(|r| r.0);
    if let Some(r) = resets_at.filter(|r| *r != 0.0 && !r.is_nan()) {
        if r * 1000.0 <= now {
            return Some(LiveWindow { pct: 0.0, resets_at: None, reset: true });
        }
    }
    Some(LiveWindow { pct: w.pct.0, resets_at, reset: false })
}

/// "resets in 2h 10m" until a unix-seconds time.
pub fn resets_in(unix: Option<f64>, now: f64) -> String {
    let Some(unix) = unix.filter(|u| !blank(*u)) else { return String::new() };
    let s = unix - now / 1000.0;
    if s <= 0.0 {
        return "resetting".into();
    }
    let h = (s / 3600.0).floor() as i64;
    let m = ((s % 3600.0) / 60.0).floor() as i64;
    let d = h / 24;
    let span = if d >= 1 {
        format!("{d}d {}h", h % 24)
    } else if h != 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    };
    format!("resets in {span}")
}

/// The clock time of a reset: the time alone on the same day, with the weekday otherwise.
pub fn reset_clock(unix: Option<f64>, now: f64, offset_min: i32) -> String {
    let Some(unix) = unix.filter(|u| !blank(*u)) else { return String::new() };
    let d = local_time(unix * 1000.0, offset_min);
    if d.same_day(&local_time(now, offset_min)) {
        d.clock()
    } else {
        d.weekday_clock()
    }
}

pub fn since(iso: impl Moment, now: f64) -> String {
    let t = iso.ms();
    if blank(t) {
        return String::new();
    }
    duration((now - t).max(0.0))
}

/// The check `openLink` makes before opening a link in a new tab: only http(s) goes.
pub fn is_web_link(url: Option<&str>) -> bool {
    let starts = |u: &str, p: &str| u.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
    url.is_some_and(|u| starts(u, "http://") || starts(u, "https://"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_protocol::Num;

    const NOW: f64 = 1_791_720_000_000.0; // 2026-10-11T12:00:00Z
    const UTC: i32 = 0;
    fn iso(ms: f64) -> String {
        // only whole seconds are needed here
        let t = local_time(ms, 0);
        let secs = (ms / 1000.0) as i64 % 60;
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z", t.year, t.month, t.day, t.hour, t.minute, secs)
    }
    fn sp(p: &str) -> String {
        short_path(Some(p), 42)
    }

    #[test]
    fn home_folders_become_tilde() {
        assert_eq!(sp("/home/me/code"), "~/code");
        assert_eq!(sp("/Users/me"), "~");
        assert_eq!(sp("/root/x"), "~/x");
        assert_eq!(sp("/homework/x"), "/homework/x");
        assert_eq!(sp("/rootfs/x"), "/rootfs/x");
    }

    #[test]
    fn long_paths_keep_their_last_segments_within_the_limit() {
        let p = "/home/me/projects/inyon/apps/web/src/components/chat/ChatView.tsx";
        let s = sp(p);
        assert_eq!(s, "~/…/web/src/components/chat/ChatView.tsx");
        assert!(len16(&s) <= 42);
        assert_eq!(short_path(Some(p), 20), "~/…/ChatView.tsx");
    }

    #[test]
    fn one_long_name_is_shown_whole() {
        let long = "a".repeat(50);
        assert_eq!(sp(&format!("~/{long}")), format!("~/{long}"));
        assert_eq!(sp(&format!("/{long}")), format!("/{long}"));
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert_eq!(short_path(None, 42), "");
        assert_eq!(sp(""), "");
    }

    #[test]
    fn windows_paths_keep_their_backslashes_and_their_home_becomes_tilde() {
        assert_eq!(sp(r"C:\Users\abhash\inyon-demo\checkout-service"), r"~\inyon-demo\checkout-service");
        assert_eq!(short_path(Some(r"D:\work\a\b\c\d\e\f\checkout-service\src"), 30), r"D:\…\e\f\checkout-service\src");
        assert_eq!(sp(r"C:\demo"), r"C:\demo");
    }

    #[test]
    fn tail_path_keeps_the_last_two_parts_with_their_separator() {
        assert_eq!(tail_path(r"C:\Users\abhash\inyon-demo\checkout-service", 32), r"…\inyon-demo\checkout-service");
        assert_eq!(tail_path("/home/me/projects/qa/oc1/very-long-name-here", 32), "…/oc1/very-long-name-here");
        assert_eq!(tail_path("/srv/app", 32), "/srv/app");
    }

    #[test]
    fn response_time_reads_ms_then_seconds() {
        assert_eq!(response_time(38.0), "38 ms");
        assert_eq!(response_time(1373.0), "1.4 s");
    }

    #[test]
    fn basenames() {
        assert_eq!(basename(Some("/a/b/c.ts")), "c.ts");
        assert_eq!(basename(Some("/a/b/")), "b");
        assert_eq!(basename(Some("/")), "/");
        assert_eq!(basename(None), "");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0.0), "0s");
        assert_eq!(duration(59_400.0), "59s");
        assert_eq!(duration(59_600.0), "1m 0s");
        assert_eq!(duration(125_000.0), "2m 5s");
        assert_eq!(duration(3_600_000.0), "1h 0m");
        assert_eq!(duration(26.0 * 3_600_000.0 + 5.0 * 60_000.0), "26h 5m");
    }

    #[test]
    fn recent_times_read_in_minutes_and_hours() {
        assert_eq!(time_ago(NOW - 10_000.0, NOW, UTC), "just now");
        assert_eq!(time_ago(NOW + 60_000.0, NOW, UTC), "just now"); // a clock slightly ahead
        assert_eq!(time_ago(iso(NOW - 10.0 * 60_000.0).as_str(), NOW, UTC), "10m ago");
        assert_eq!(time_ago(NOW - 3.0 * 3_600_000.0, NOW, UTC), "3h ago");
    }

    #[test]
    fn older_times_show_the_date() {
        let s = time_ago(NOW - 3.0 * 86_400_000.0, NOW, UTC);
        assert!(!s.contains("ago"));
        assert_eq!(s, "Oct 8");
    }

    #[test]
    fn missing_or_invalid_times_are_blank() {
        assert_eq!(time_ago("", NOW, UTC), "");
        assert_eq!(time_ago("not a date", NOW, UTC), "");
        assert_eq!(time_ago(0.0, NOW, UTC), "");
        assert_eq!(since(None::<&str>, NOW), "");
        assert_eq!(since("garbage", NOW), "");
    }

    #[test]
    fn since_counts_up_from_a_time() {
        assert_eq!(since(NOW - 90_000.0, NOW), "1m 30s");
        assert_eq!(since(iso(NOW - 5_000.0), NOW), "5s");
        assert_eq!(since(NOW + 5_000.0, NOW), "0s");
    }

    #[test]
    fn clocks() {
        assert_eq!(clock("", UTC), "");
        assert_eq!(clock(&iso(NOW), UTC), "12:00 PM");
        assert_eq!(clock(&iso(NOW), -300), "7:00 AM");
    }

    #[test]
    fn claude_ids_read_as_family_and_version() {
        assert_eq!(model_name(Some("claude-opus-4-1-20250805")), "Opus 4.1");
        assert_eq!(model_name(Some("claude-sonnet-4-20250514")), "Sonnet 4");
        assert_eq!(model_name(Some("claude-opus-5-5")), "Opus 5.5");
        assert_eq!(model_name(Some("claude-haiku-4-5")), "Haiku 4.5");
    }

    #[test]
    fn the_1m_context_suffix_is_the_same_model() {
        assert_eq!(model_name(Some("claude-opus-4-6[1m]")), "Opus 4.6");
        assert_eq!(model_name(Some("claude-fable-5[1m]")), "Fable 5");
    }

    #[test]
    fn other_model_ids_are_shown_as_is() {
        assert_eq!(model_name(Some("gpt-5-codex")), "gpt-5-codex");
        assert_eq!(model_name(Some("claude-3-5-sonnet-20241022")), "claude-3-5-sonnet-20241022");
        assert_eq!(model_name(None), "");
    }

    #[test]
    fn pane_labels() {
        assert_eq!(pane_label("%12"), "12");
        assert_eq!(pane_label("12"), "12");
    }

    #[test]
    fn token_counts() {
        assert_eq!(tokens(None), "—");
        assert_eq!(tokens(Some(f64::NAN)), "—");
        assert_eq!(tokens(Some(0.0)), "0");
        assert_eq!(tokens(Some(999.0)), "999");
        assert_eq!(tokens(Some(1499.0)), "1k");
        assert_eq!(tokens(Some(1500.0)), "2k");
        assert_eq!(tokens(Some(999_400.0)), "999k");
        // never "1000k"
        assert_eq!(tokens(Some(999_999.0)), "1M");
        assert_eq!(tokens(Some(1_000_000.0)), "1M");
        assert_eq!(tokens(Some(1_250_000.0)), "1.3M");
        assert_eq!(tokens(Some(12_345_678.0)), "12M");
    }

    #[test]
    fn byte_sizes() {
        let k = 1024.0;
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(2048.0), "2 KB");
        assert_eq!(bytes(5.5 * k * k), "5.5 MB");
        assert_eq!(bytes(250.0 * k * k), "250 MB");
        assert_eq!(bytes(3.0 * k * k * k), "3.0 GB");
    }

    #[test]
    fn money_amounts() {
        assert_eq!(money(None), "—");
        assert_eq!(money(Some(0.0)), "$0.00");
        assert_eq!(money(Some(3.456)), "$3.46");
        assert_eq!(money(Some(150.4)), "$150");
    }

    fn window(pct: f64, resets_at: Option<f64>) -> RateWindow {
        RateWindow { pct: Num(pct), resets_at: resets_at.map(Num) }
    }

    #[test]
    fn a_window_past_its_reset_shows_as_fresh() {
        let w = window(80.0, Some(NOW / 1000.0 - 1.0));
        assert_eq!(live_window(Some(&w), NOW), Some(LiveWindow { pct: 0.0, resets_at: None, reset: true }));
        assert_eq!(w.pct.0, 80.0); // the original is left alone
    }

    #[test]
    fn a_current_window_is_returned_as_is() {
        let w = window(40.0, Some(NOW / 1000.0 + 60.0));
        assert_eq!(live_window(Some(&w), NOW), Some(LiveWindow { pct: 40.0, resets_at: Some(NOW / 1000.0 + 60.0), reset: false }));
        assert_eq!(live_window(Some(&window(40.0, None)), NOW), Some(LiveWindow { pct: 40.0, resets_at: None, reset: false }));
        assert_eq!(live_window(None, NOW), None);
    }

    #[test]
    fn resets_in_counts_down() {
        let at = |s: f64| Some(NOW / 1000.0 + s);
        assert_eq!(resets_in(None, NOW), "");
        assert_eq!(resets_in(at(-5.0), NOW), "resetting");
        assert_eq!(resets_in(at(30.0 * 60.0), NOW), "resets in 30m");
        assert_eq!(resets_in(at(90.0 * 60.0), NOW), "resets in 1h 30m");
        assert_eq!(resets_in(at(50.0 * 3600.0), NOW), "resets in 2d 2h");
    }

    #[test]
    fn reset_clock_shows_only_the_time_on_the_same_day_the_weekday_otherwise() {
        assert_eq!(reset_clock(None, NOW, UTC), "");
        assert_eq!(reset_clock(Some((NOW + 60_000.0) / 1000.0), NOW, UTC), "12:01 PM");
        assert_eq!(reset_clock(Some((NOW + 3.0 * 86_400_000.0) / 1000.0), NOW, UTC), "Wed 12:00 PM");
    }

    #[test]
    fn only_web_links_open() {
        assert!(is_web_link(Some("https://example.com")));
        assert!(is_web_link(Some("HTTP://example.com/a")));
        assert!(!is_web_link(Some("javascript:alert(1)")));
        assert!(!is_web_link(Some("file:///etc/passwd")));
        assert!(!is_web_link(None));
    }

    #[test]
    fn model_names_by_hand_match_the_pattern() {
        let model = regex::Regex::new(r"claude-([a-z]+)-([0-9]+)(?:-([0-9]{1,2}))?(?:[-\[]|$)").unwrap();
        let by_regex = |id: &str| model.captures(id).map(|m| (m[1].to_string(), m[2].to_string(), m.get(3).map(|x| x.as_str().to_string())));
        let pieces = ["claude-", "opus", "sonnet", "-", "4", "5", "123", "[1m]", "[", "x", "us.anthropic.", "Claude-", "2025", "-v1", ""];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut ids: Vec<String> = [
            "claude-opus-4-6[1m]",
            "claude-sonnet-4-5-20250929",
            "claude-3-5-sonnet",
            "claude-opus-4-123",
            "claude-haiku-4",
            "us.anthropic.claude-opus-4-1-v1:0",
            "claude-opus-4x",
            "claude-opus-",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for _ in 0..4000 {
            let len = 1 + (seed % 8) as usize;
            ids.push(
                (0..len)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        pieces[(seed % pieces.len() as u64) as usize]
                    })
                    .collect(),
            );
        }
        for id in ids {
            let by_hand = claude_model(&id).map(|(f, a, b)| (f.to_string(), a.to_string(), b.map(str::to_string)));
            assert_eq!(by_hand, by_regex(&id), "{id:?}");
        }
    }
}
