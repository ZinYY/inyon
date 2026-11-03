//! What Settings, Temp files, the companion and the update prompt decide, apart from drawing it
//! (pages/Settings.tsx, pages/Temp.tsx, components/CompanionAgent.tsx, CompanionSheet.tsx,
//! UpdatePrompt.tsx, lib/updates.ts and lib/end.ts), so it can be tested without a browser.

use inyon_chat_core::format::bytes;
use inyon_protocol::{AgentKind, CompanionScope, HostKind, HostState, IntegrationState, SleepRule};

use crate::api::{CustomAgent, HubUpdate, TempItem, TempListing};

// ---------- Settings › Live numbers: idle agents go to sleep ----------

const SLEEP_CHOICES: [(u32, &str); 4] = [(0, "Off"), (30, "30 min"), (60, "1 hour"), (120, "2 hours")];

/// "45 min", "1 hour", "3 hours".
pub fn sleep_label(m: u32) -> String {
    if m.is_multiple_of(60) {
        format!("{} hour{}", m / 60, if m == 60 { "" } else { "s" })
    } else {
        format!("{m} min")
    }
}

/// The minutes the "all" rule sets, 0 when there's none.
pub fn sleep_minutes(rules: &[SleepRule]) -> u32 {
    rules.iter().find(|r| r.scope == "all").map_or(0, |r| r.minutes)
}

/// The choices, with a time set another way (the API takes any minutes) as one more, in order.
pub fn sleep_choices(current: u32) -> Vec<(String, String)> {
    let mut v: Vec<(u32, String)> = SLEEP_CHOICES.iter().map(|(m, l)| (*m, l.to_string())).collect();
    if !v.iter().any(|(m, _)| *m == current) {
        v.push((current, sleep_label(current)));
        v.sort_by_key(|(m, _)| *m);
    }
    v.into_iter().map(|(m, l)| (m.to_string(), l)).collect()
}

// ---------- Settings › Updates ----------

/// What the hub's update row shows.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateRow {
    /// the update runs; the hub restarts when it's done
    Updating,
    /// updated, but nothing could restart it
    RestartNeeded,
    /// a newer release or newer commits; `button` when it can update from here
    Available {
        label: String,
        hint: String,
        button: Option<&'static str>,
    },
    UpToDate {
        hint: String,
    },
}

pub fn update_row(info: &HubUpdate, updating: bool) -> UpdateRow {
    if updating || info.running {
        return UpdateRow::Updating;
    }
    if info.restart_needed {
        return UpdateRow::RestartNeeded;
    }
    let running = format!("This hub runs {}.", info.current);
    let reason = info.reason.clone().unwrap_or_default();
    if info.latest.is_some() || info.behind.unwrap_or(0) > 0 {
        let commits = match info.behind.unwrap_or(0) {
            0 => String::new(),
            1 => " 1 new commit to pull.".into(),
            n => format!(" {n} new commits to pull."),
        };
        let label = info.latest.as_ref().map_or_else(|| "Update available".to_string(), |l| format!("Update available: {l}"));
        if info.can_update {
            let button = if info.error.is_some() { "Try again" } else { "Update" };
            return UpdateRow::Available { label, hint: format!("{running}{commits} It restarts when the update is done."), button: Some(button) };
        }
        return UpdateRow::Available { label, hint: format!("{running} {reason}"), button: None };
    }
    UpdateRow::UpToDate { hint: if reason.is_empty() { running } else { format!("{running} {reason}") } }
}

/// What the hub or this page runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub version: String,
    pub commit: Option<String>,
}

/// The same version, and the same commit when both know theirs (short hashes may differ in length).
pub fn same_build(a: &Build, b: &Build) -> bool {
    if a.version != b.version {
        return false;
    }
    match (&a.commit, &b.commit) {
        (Some(x), Some(y)) if !x.is_empty() && !y.is_empty() => x.starts_with(y.as_str()) || y.starts_with(x.as_str()),
        _ => true,
    }
}

/// Where "Inyon was updated" remembers the build it was offered for (the web app's key).
pub const RELOAD_SEEN: &str = "inyon.reload-seen";

pub fn build_key(b: &Build) -> String {
    format!("{}+{}", b.version, b.commit.as_deref().unwrap_or(""))
}

/// Whether to offer a reload: the hub runs other code than this page, offered once per build.
pub fn reload_due(hub: &Build, page: &Build, seen: Option<&str>) -> bool {
    !same_build(hub, page) && seen != Some(build_key(hub).as_str())
}

// ---------- Settings › Paired devices ----------

/// What a companion's scope is called, and what it allows.
pub fn scope_label(s: CompanionScope) -> (&'static str, &'static str) {
    match s {
        CompanionScope::Overview => ("Overview", "Every session, its state and machine, and live changes"),
        CompanionScope::Chat => ("Chats", "The screen, details and alerts of a chat it opens"),
        CompanionScope::Answer => ("Answer", "Answer a prompt, once, with its prompt key"),
        CompanionScope::Send => ("Send", "Type text into a session; the hub answers once it has landed"),
    }
}

/// "now", "5 min ago", "3 h ago", else the date.
pub fn since(at: f64, now: f64, date: impl Fn(f64) -> String) -> String {
    let m = ((now - at) / 60_000.0).round();
    if m < 1.0 {
        "now".into()
    } else if m < 60.0 {
        format!("{m} min ago")
    } else if m < 24.0 * 60.0 {
        format!("{} h ago", (m / 60.0).round())
    } else {
        date(at)
    }
}

/// What a companion may do under contract 1, as the paired summary says it.
pub const COMPANION_CAN: [&str; 2] = ["Reads the session overview and the chats you open in it", "Answers prompts and sends text to a session"];
pub const COMPANION_CANT: &str = "Can't open terminals, files or settings";

/// 5:00, 0:42
pub fn mmss(ms: f64) -> String {
    let s = (ms / 1000.0).ceil().max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// URLSearchParams' encoding of one value.
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The code, the hub's address and its certificate's SHA-256 in one link, for an app that takes a
/// pasted link (contract 1.1).
pub fn companion_link(origin: &str, code: &str, fingerprint: Option<&str>) -> String {
    let mut s = format!("inyon://companion?u={}&c={}", form_encode(origin), form_encode(code));
    if let Some(f) = fingerprint.filter(|f| !f.is_empty()) {
        s.push_str(&format!("&f={}", form_encode(f)));
    }
    s
}

// ---------- Settings › Notifications ----------

/// The browser's permission for notifications, or why there's none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Default,
    Granted,
    // only the browser says so
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Denied,
    Unsupported,
}

/// The System notifications row's hint. `push` when the browser can be reached while Inyon is
/// closed; `home_screen` for Safari on an iPhone or iPad, which has notifications only for an app
/// added to the Home Screen.
pub fn notices_hint(secure: bool, perm: Permission, push: bool, home_screen: bool) -> &'static str {
    if !secure {
        return "Browsers only allow these over HTTPS or in the installed app. In-app notices still work.";
    }
    match perm {
        Permission::Unsupported if home_screen => "On iPhone, add Inyon to the Home Screen to get these",
        Permission::Granted if push => "On for this device, also while Inyon is closed",
        Permission::Granted => "On for this device",
        Permission::Denied => "Turned off in your phone's settings for Inyon",
        _ if push => "Alerts when an agent needs you, even with Inyon closed",
        _ => "Alerts when an agent needs you, while Inyon is open",
    }
}

// ---------- ending a session ----------

/// What the confirm sheet says ending a pane does.
pub fn end_text(agent: Option<&str>, panes_in_window: usize, windows_in_session: usize, session: &str, window: u32) -> String {
    let also = if panes_in_window > 1 {
        String::new()
    } else if windows_in_session > 1 {
        format!(", and window {session}:{window} with it, since nothing else is in it")
    } else {
        format!(", and the tmux session “{session}” with it, since nothing else is in it")
    };
    match agent {
        Some(a) => format!("{a} exits, then its tmux pane closes{also}. Your other windows stay as they are."),
        None => format!("Its tmux pane closes{also}. Your other windows stay as they are."),
    }
}

// ---------- Temp files ----------

/// A week, in ms: what "Older than a week" picks.
const WEEK: f64 = 7.0 * 86_400_000.0;
/// Thumbnails are small; a bigger image shows a placeholder until it's opened.
pub const THUMB_MAX: u64 = 1_500_000;

/// One folder an agent keeps temp files in, with what's in it.
#[derive(Debug, Clone, PartialEq)]
pub struct TempGroup {
    pub id: String,
    pub label: String,
    pub size: f64,
    pub items: Vec<TempItem>,
}

/// The listing as the screen shows it: the folders that exist and hold something, in order.
pub fn temp_groups(l: &TempListing) -> Vec<TempGroup> {
    let items = l.items.as_deref().unwrap_or_default();
    l.roots
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|r| r.exists && items.iter().any(|i| i.root == r.id))
        .map(|r| TempGroup { id: r.id.clone(), label: r.label.clone(), size: r.size, items: items.iter().filter(|i| i.root == r.id).cloned().collect() })
        .collect()
}

/// A tick on `at` (in screen order): with shift, everything from the last tick to here goes on;
/// without, this one flips.
pub fn toggle_pick(picked: &[String], order: &[String], at: usize, last: Option<usize>, range: bool) -> Vec<String> {
    let mut next: Vec<String> = picked.to_vec();
    let Some(path) = order.get(at) else { return next };
    match last.filter(|l| range && *l < order.len()) {
        Some(from) => {
            let (a, b) = (from.min(at), from.max(at));
            for p in &order[a..=b] {
                if !next.contains(p) {
                    next.push(p.clone());
                }
            }
        }
        None => {
            if let Some(i) = next.iter().position(|p| p == path) {
                next.remove(i);
            } else {
                next.push(path.clone());
            }
        }
    }
    next
}

/// What "Older than a week" picks: not changed lately and older than seven days.
pub fn older_than_a_week(items: &[TempItem], now: f64) -> Vec<String> {
    items.iter().filter(|i| !i.recent && now - i.modified > WEEK).map(|i| i.path.clone()).collect()
}

/// The question before deleting: how much goes, that it's for good, and whether an agent may still
/// use it. `(title, body)`.
pub fn delete_question(items: &[(String, f64, bool)], machine: &str) -> (String, String) {
    let size: f64 = items.iter().map(|i| i.1).sum();
    let recent = items.iter().filter(|i| i.2).count();
    let one = items.len() == 1;
    let mut body = format!("Frees {} on {machine}. This can't be undone.", bytes(size));
    if recent > 0 {
        if one {
            body.push_str(" It changed in the last 30 minutes, so an agent may still be using it.");
        } else {
            body.push_str(&format!(" {recent} of them changed in the last 30 minutes, so an agent may still be using them."));
        }
    }
    let title = if one { format!("Delete {}?", items[0].0) } else { format!("Delete {} items?", items.len()) };
    (title, body)
}

/// "1 item", "3 items".
pub fn items_text(n: usize) -> String {
    format!("{n} item{}", if n == 1 { "" } else { "s" })
}

/// The line under an item's name: its size, how many files in a folder, and when it changed.
pub fn item_line(i: &TempItem, ago: &str) -> String {
    if i.kind == "dir" {
        format!("{} · {} file{} · {ago}", bytes(i.size), i.files, if i.files == 1 { "" } else { "s" })
    } else {
        format!("{} · {ago}", bytes(i.size))
    }
}

/// An image preview as a `data:` address the page can show.
pub fn data_url(mime: &str, base64: Option<&str>) -> Option<String> {
    base64.filter(|b| !b.is_empty()).map(|b| format!("data:{mime};base64,{b}"))
}

// ---------- Settings: the rest of what the rows say ----------

/// The docs on the website: setup, pairing, every screen and command.
pub fn guide_url() -> String {
    format!("{}/docs/", inyon_protocol::INSTALL_URL)
}

/// Where people can pay for Inyon, if they want to (the repo's FUNDING.yml names the same).
pub const SPONSOR: &str = "https://github.com/sponsors/Abhash-Chakraborty";

/// "Inyon 0.25.0 (abc1234)", for a bug report.
pub fn version_summary(version: &str, commit: Option<&str>) -> String {
    match commit.filter(|c| !c.is_empty()) {
        Some(c) => format!("Inyon {version} ({c})"),
        None => format!("Inyon {version}"),
    }
}

/// A machine in Settings › Machines: where it is, and whether the integration is on.
pub fn machine_line(h: &HostState, hosts: &[HostState]) -> String {
    let place = match h.config.kind {
        HostKind::Local => "This hub".to_string(),
        HostKind::Node => h.hostname.clone().unwrap_or_else(|| "Paired machine".into()),
        HostKind::Link => h.hostname.clone().unwrap_or_else(|| "Laptop".into()),
        HostKind::Ssh => crate::machines::ssh_address(&h.config, hosts),
    };
    let on = if h.integration == IntegrationState::Installed { "on" } else { "off" };
    format!("{place} · integration {on}")
}

/// What's wrong with a new password before it's sent, if anything.
pub fn password_error(next: &str, again: &str) -> Option<&'static str> {
    if next.chars().count() < 8 {
        Some("Use at least 8 characters.")
    } else if next != again {
        Some("The two new passwords don't match.")
    } else {
        None
    }
}

/// What the hub address says when it names the hub machine itself.
pub const LOCAL_HINT: &str = "This is the hub machine itself. Other devices use its Tailscale or NetBird name or IP with the same port.";

/// The hub address on this page's own machine, which other devices can't use as it is.
pub fn local_address(origin: &str) -> bool {
    let rest = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")).unwrap_or("");
    rest.starts_with("localhost") || rest.starts_with("127.") || rest.starts_with("[::1]")
}

/// The agents a custom agent can work like: the ones whose conversation Inyon reads.
pub fn chat_kinds() -> Vec<AgentKind> {
    inyon_protocol::agents().values().filter(|d| d.chat).map(|d| d.kind).collect()
}

/// A custom agent's list with `next` in place of the one with `old`'s id, or added at the end.
pub fn with_custom(list: &[CustomAgent], old: Option<&str>, next: CustomAgent) -> Option<Vec<CustomAgent>> {
    match old {
        // removed meanwhile (on another device): there's nothing to edit
        Some(id) if !list.iter().any(|a| a.id == id) => None,
        Some(id) => Some(list.iter().map(|a| if a.id == id { next.clone() } else { a.clone() }).collect()),
        None => Some(list.iter().cloned().chain([next]).collect()),
    }
}

/// A removed custom agent back where it was (or last, when the list got shorter), unless it's
/// there again already.
pub fn put_back(list: &mut Vec<CustomAgent>, agent: CustomAgent, at: usize) {
    if !list.iter().any(|a| a.id == agent.id) {
        list.insert(at.min(list.len()), agent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(scope: &str, minutes: u32) -> SleepRule {
        SleepRule { scope: scope.into(), minutes }
    }

    #[test]
    fn idle_agents_sleep_after_one_of_a_few_times_or_the_one_set_elsewhere() {
        assert_eq!(sleep_minutes(&[rule("p1", 10), rule("all", 60)]), 60);
        assert_eq!(sleep_minutes(&[]), 0);
        let labels = |m| sleep_choices(m).into_iter().map(|(_, l)| l).collect::<Vec<_>>();
        assert_eq!(labels(60), ["Off", "30 min", "1 hour", "2 hours"]);
        assert_eq!(labels(45), ["Off", "30 min", "45 min", "1 hour", "2 hours"]);
        assert_eq!(labels(180), ["Off", "30 min", "1 hour", "2 hours", "3 hours"]);
    }

    fn info() -> HubUpdate {
        HubUpdate { current: "0.24.0".into(), can_update: true, ..Default::default() }
    }

    #[test]
    fn the_update_row_says_what_the_hub_can_do() {
        assert_eq!(update_row(&info(), false), UpdateRow::UpToDate { hint: "This hub runs 0.24.0.".into() });
        let newer = HubUpdate { latest: Some("0.25.0".into()), ..info() };
        assert_eq!(
            update_row(&newer, false),
            UpdateRow::Available { label: "Update available: 0.25.0".into(), hint: "This hub runs 0.24.0. It restarts when the update is done.".into(), button: Some("Update") }
        );
        let source = HubUpdate { behind: Some(3), error: Some("pull failed".into()), ..info() };
        assert_eq!(
            update_row(&source, false),
            UpdateRow::Available {
                label: "Update available".into(),
                hint: "This hub runs 0.24.0. 3 new commits to pull. It restarts when the update is done.".into(),
                button: Some("Try again")
            }
        );
        let npm = HubUpdate { latest: Some("0.25.0".into()), can_update: false, reason: Some("Installed with npm: run npm i -g.".into()), ..info() };
        assert!(matches!(update_row(&npm, false), UpdateRow::Available { button: None, hint, .. } if hint == "This hub runs 0.24.0. Installed with npm: run npm i -g."));
        assert_eq!(update_row(&newer, true), UpdateRow::Updating);
        assert_eq!(update_row(&HubUpdate { restart_needed: true, ..info() }, false), UpdateRow::RestartNeeded);
    }

    fn b(v: &str, c: Option<&str>) -> Build {
        Build { version: v.into(), commit: c.map(str::to_string) }
    }

    #[test]
    fn a_reload_is_offered_once_for_each_new_build() {
        assert!(same_build(&b("0.24.0", Some("abc1234")), &b("0.24.0", Some("abc1234ffee"))));
        assert!(same_build(&b("0.24.0", None), &b("0.24.0", Some("abc"))));
        assert!(!same_build(&b("0.24.0", Some("abc")), &b("0.24.0", Some("def"))));
        let page = b("0.24.0", Some("abc"));
        let hub = b("0.25.0", Some("def"));
        assert!(reload_due(&hub, &page, None));
        assert!(!reload_due(&hub, &page, Some("0.25.0+def")), "offered already");
        assert!(!reload_due(&page, &page, None));
        assert_eq!(build_key(&b("0.25.0", None)), "0.25.0+");
    }

    #[test]
    fn a_companion_pairs_with_a_code_or_a_link() {
        assert_eq!(mmss(300_000.0), "5:00");
        assert_eq!(mmss(41_200.0), "0:42");
        assert_eq!(mmss(-5.0), "0:00");
        assert_eq!(companion_link("https://hub.example:8443", "ABC234", None), "inyon://companion?u=https%3A%2F%2Fhub.example%3A8443&c=ABC234");
        assert_eq!(companion_link("http://100.64.0.1:7878", "XYZ789", Some("ab12")), "inyon://companion?u=http%3A%2F%2F100.64.0.1%3A7878&c=XYZ789&f=ab12");
        assert_eq!(scope_label(CompanionScope::Send).0, "Send");
    }

    #[test]
    fn seen_reads_as_minutes_hours_then_the_date() {
        let date = |_| "2/11/2026".to_string();
        assert_eq!(since(1_000.0, 20_000.0, date), "now");
        assert_eq!(since(0.0, 5.0 * 60_000.0, date), "5 min ago");
        assert_eq!(since(0.0, 3.0 * 3_600_000.0, date), "3 h ago");
        assert_eq!(since(0.0, 3.0 * 86_400_000.0, date), "2/11/2026");
    }

    #[test]
    fn the_notifications_row_says_why_they_are_off() {
        assert!(notices_hint(false, Permission::Granted, true, false).starts_with("Browsers only allow"));
        assert_eq!(notices_hint(true, Permission::Granted, true, false), "On for this device, also while Inyon is closed");
        // Safari in a tab: only the Home Screen app gets notifications
        assert_eq!(notices_hint(true, Permission::Unsupported, false, true), "On iPhone, add Inyon to the Home Screen to get these");
        assert_eq!(notices_hint(true, Permission::Default, false, true), "Alerts when an agent needs you, while Inyon is open");
        // without Web Push nothing reaches a closed tab
        assert_eq!(notices_hint(true, Permission::Default, false, false), "Alerts when an agent needs you, while Inyon is open");
        assert_eq!(notices_hint(true, Permission::Default, true, false), "Alerts when an agent needs you, even with Inyon closed");
    }

    #[test]
    fn ending_a_session_says_what_closes_with_it() {
        assert_eq!(end_text(Some("Claude Code"), 2, 1, "api", 1), "Claude Code exits, then its tmux pane closes. Your other windows stay as they are.");
        assert_eq!(end_text(None, 1, 2, "api", 3), "Its tmux pane closes, and window api:3 with it, since nothing else is in it. Your other windows stay as they are.");
        assert!(end_text(Some("Codex"), 1, 1, "docs", 1).contains("the tmux session “docs” with it"));
    }

    fn item(path: &str, root: &str, size: f64, modified: f64, recent: bool) -> TempItem {
        TempItem { root: root.into(), path: path.into(), name: path.rsplit('/').next().unwrap().into(), kind: "dir".into(), size, files: 2, modified, recent, images: vec![] }
    }

    #[test]
    fn temp_files_group_pick_and_ask_before_deleting() {
        let l: TempListing = serde_json::from_value(serde_json::json!({
            "roots": [
                { "id": "claude", "label": "Claude Code", "size": 30, "exists": true },
                { "id": "empty", "label": "Nothing", "size": 0, "exists": true },
                { "id": "gone", "label": "Gone", "size": 0, "exists": false },
            ],
            "items": [
                { "root": "claude", "path": "/t/a", "name": "a", "kind": "dir", "size": 10, "files": 1, "modified": 0, "recent": false, "images": [] },
                { "root": "claude", "path": "/t/b", "name": "b", "kind": "file", "size": 20, "modified": 0, "recent": true },
            ],
        }))
        .unwrap();
        let g = temp_groups(&l);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].items.len(), 2);
        let order: Vec<String> = ["/t/a", "/t/b", "/t/c", "/t/d"].map(String::from).to_vec();
        let one = toggle_pick(&[], &order, 0, None, false);
        assert_eq!(one, ["/t/a"]);
        assert_eq!(toggle_pick(&one, &order, 3, Some(0), true), ["/t/a", "/t/b", "/t/c", "/t/d"]);
        assert!(toggle_pick(&one, &order, 0, Some(0), false).is_empty());
        let now = 20.0 * 86_400_000.0;
        let items = [item("/t/old", "x", 1.0, 0.0, false), item("/t/new", "x", 1.0, now - 1000.0, false), item("/t/busy", "x", 1.0, 0.0, true)];
        assert_eq!(older_than_a_week(&items, now), ["/t/old"]);
        let (title, body) = delete_question(&[("a".into(), 2048.0, true)], "studio");
        assert_eq!(title, "Delete a?");
        assert!(body.starts_with("Frees 2 KB on studio. This can't be undone. It changed in the last 30 minutes"), "{body}");
        let (title, body) = delete_question(&[("a".into(), 1.0, false), ("b".into(), 1.0, true)], "studio");
        assert_eq!(title, "Delete 2 items?");
        assert!(body.ends_with("1 of them changed in the last 30 minutes, so an agent may still be using them."));
        assert_eq!(items_text(1), "1 item");
        assert_eq!(data_url("image/png", Some("AAA")).as_deref(), Some("data:image/png;base64,AAA"));
        assert_eq!(data_url("image/png", None), None);
    }
    #[test]
    fn a_new_password_needs_eight_characters_typed_twice_the_same() {
        assert_eq!(password_error("short", "short"), Some("Use at least 8 characters."));
        assert_eq!(password_error("long enough", "long enougj"), Some("The two new passwords don't match."));
        assert_eq!(password_error("long enough", "long enough"), None);
    }

    #[test]
    fn the_about_rows_name_the_build_and_the_links() {
        assert_eq!(version_summary("0.25.0", Some("abc1234")), "Inyon 0.25.0 (abc1234)");
        assert_eq!(version_summary("0.25.0", Some("")), "Inyon 0.25.0");
        assert_eq!(guide_url(), "https://inyon.tech/docs/");
        assert!(local_address("http://localhost:7681"));
        assert!(local_address("http://127.0.0.1:7681"));
        assert!(local_address("http://[::1]:7681"));
        assert!(!local_address("https://box.netbird.cloud"));
    }

    #[test]
    fn custom_agents_work_like_a_chat_agent_and_edit_in_place() {
        let kinds = chat_kinds();
        assert!(kinds.contains(&AgentKind::Claude) && kinds.contains(&AgentKind::Codex));
        assert!(kinds.iter().all(|k| k.def().chat));
        let a = |id: &str, name: &str| CustomAgent { id: id.into(), name: name.into(), command: "claude".into(), like: AgentKind::Claude };
        let list = vec![a("1", "One"), a("2", "Two")];
        let edited = with_custom(&list, Some("2"), a("2", "Deux")).unwrap();
        assert_eq!(edited.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["One", "Deux"]);
        let added = with_custom(&list, None, a("", "Three")).unwrap();
        assert_eq!(added.len(), 3);
        assert_eq!(with_custom(&list, Some("9"), a("9", "Gone")), None);
    }

    #[test]
    fn undo_puts_a_removed_agent_back_into_the_list_as_it_is_now() {
        let a = |id: &str| CustomAgent { id: id.into(), name: id.into(), command: "claude".into(), like: AgentKind::Claude };
        // "2" was removed from 1, 2, 3; "4" was added since
        let mut list = vec![a("1"), a("3"), a("4")];
        put_back(&mut list, a("2"), 1);
        assert_eq!(list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4"]);
        put_back(&mut list, a("2"), 1);
        assert_eq!(list.len(), 4);
        let mut short = vec![];
        put_back(&mut short, a("9"), 5);
        assert_eq!(short.len(), 1);
    }
}
