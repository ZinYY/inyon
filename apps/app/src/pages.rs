//! Pages: sites an agent made (plans, mockups) that the hub serves under a token link, shown in a
//! sandboxed frame. The rules behind the screens, as apps/web/src/lib/pages.ts and
//! components/pages/shared.tsx have them: how pages group, which ones a session works on, which a
//! reply links to, what counts as changed, when a link is renewed, and the words the screens use.
//! The list comes live from the hub's `pages` stream (v2.md); everything here is pure.

use std::collections::HashMap;

use inyon_chat_core::js::{local_time, parse_date};
use inyon_chat_core::overview::{all_panes, pane_title};
use inyon_protocol::{HostKind, HostState, HostStatus, MachineKind, PageEntry, PageOpen};

use crate::format::Clock;

/// `<host>/<slug>`
pub fn page_id(host: &str, slug: &str) -> String {
    format!("{host}/{slug}")
}

pub fn id_of(p: &PageEntry) -> String {
    page_id(&p.host, &p.slug)
}

/// The last folder of a project path, as the list names it.
pub fn project_name(project: &str) -> String {
    let trimmed = project.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(last) if !last.is_empty() => last.to_string(),
        _ => project.to_string(),
    }
}

/// When a page last changed, as far as the app can tell: its progress, or when it was added.
pub fn changed_at(p: &PageEntry) -> f64 {
    let t = p.progress.as_ref().and_then(|g| g.updated.as_deref()).map_or(f64::NAN, parse_date);
    if t.is_finite() {
        t.max(p.added as f64)
    } else {
        p.added as f64
    }
}

/// A machine behind a paired hub, as the phone app names them: `hub:<id>/<host>`.
pub fn is_hub_host(host: &str) -> bool {
    host.strip_prefix("hub:").is_some_and(|r| r.find('/').is_some_and(|i| i > 0))
}

/// A hub's own machine: "local" here, or "hub:<id>/local" for a paired hub's.
pub fn is_hub_machine(host: &str) -> bool {
    let own = if is_hub_host(host) { &host[host.find('/').map_or(0, |i| i + 1)..] } else { host };
    own == "local"
}

/// What a page's group is called: the name its session has in the app now (`running`, by
/// `<host>/<session>`), else the name Inyon last gave that session, else its project's folder.
pub fn group_name(p: &PageEntry, running: &HashMap<String, String>) -> String {
    p.session
        .as_deref()
        .and_then(|s| running.get(&page_id(&p.host, s)))
        .filter(|n| !n.is_empty())
        .cloned()
        .or_else(|| p.session_name.clone().filter(|n| !n.is_empty()))
        .unwrap_or_else(|| project_name(&p.project))
}

/// The names running agent sessions have in the app, by `<host>/<conversation id>`. An agent that
/// shows no title of its own ("Claude Code") gives no name.
pub fn running_names(hosts: &[HostState]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for r in all_panes(hosts) {
        let Some(a) = &r.pane.agent else { continue };
        let Some(sid) = a.session_id.as_deref() else { continue };
        let name = pane_title(&r);
        let own = a.custom.clone().unwrap_or_else(|| a.kind.def().name.to_string());
        if !name.is_empty() && name != own && name != r.pane.command {
            out.insert(page_id(&r.host.config.id, sid), name);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectGroup {
    pub name: String,
    /// `<host>/s:<session>` or `<host>/p:<name>`
    pub key: String,
    pub pages: Vec<PageEntry>,
}

/// A machine's projects and their pages, the newest change first.
#[derive(Debug, Clone, PartialEq)]
pub struct PageGroup {
    pub host: String,
    pub projects: Vec<ProjectGroup>,
}

fn by_change(a: &PageEntry, b: &PageEntry) -> std::cmp::Ordering {
    changed_at(b).partial_cmp(&changed_at(a)).unwrap_or(std::cmp::Ordering::Equal)
}

/// `localeCompare` closely enough for names: case folded first, then as written.
fn name_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// Machine, then session or project, then page. The hub's own machine comes first, then the others
/// by name; groups by name (see [`group_name`]); inside a group the page that changed last is on top.
pub fn group_pages(pages: &[&PageEntry], host_name: impl Fn(&str) -> String, running: &HashMap<String, String>) -> Vec<PageGroup> {
    let mut hosts: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<(String, String, Vec<PageEntry>)>> = HashMap::new();
    for p in pages {
        if !groups.contains_key(&p.host) {
            hosts.push(p.host.clone());
        }
        let list = groups.entry(p.host.clone()).or_default();
        let name = group_name(p, running);
        let key = match &p.session {
            Some(s) if name != project_name(&p.project) => format!("s:{s}"),
            _ => format!("p:{name}"),
        };
        match list.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, _, v)) => v.push((*p).clone()),
            None => list.push((key, name, vec![(*p).clone()])),
        }
    }
    hosts.sort_by(|a, b| is_hub_machine(b).cmp(&is_hub_machine(a)).then_with(|| name_cmp(&host_name(a), &host_name(b))));
    hosts
        .into_iter()
        .map(|host| {
            let mut list = groups.remove(&host).unwrap_or_default();
            list.sort_by(|a, b| name_cmp(&a.1, &b.1));
            let projects = list
                .into_iter()
                .map(|(key, name, mut v)| {
                    v.sort_by(by_change);
                    ProjectGroup { name, key: format!("{host}/{key}"), pages: v }
                })
                .collect();
            PageGroup { host, projects }
        })
        .collect()
}

/// Whether a page matches what's typed in the filter: its name, group, project or machine.
pub fn matches_page(p: &PageEntry, machine: &str, q: &str, running: &HashMap<String, String>) -> bool {
    let t = q.trim().to_lowercase();
    t.is_empty() || [p.name.clone(), group_name(p, running), project_name(&p.project), machine.to_string()].iter().any(|s| s.to_lowercase().contains(&t))
}

// ---------- a session's pages ----------

fn drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A folder as the app compares it: forward slashes, no slash at the end, and a Windows path in any
/// case.
pub fn folder_key(p: &str) -> String {
    let s = p.replace('\\', "/");
    let trimmed = s.trim_end_matches('/');
    // the root keeps its slash
    let s = if trimmed.is_empty() && !s.is_empty() { &s[..1] } else { trimmed };
    if drive(s) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

fn within(child: &str, parent: &str) -> bool {
    child == parent || child.starts_with(&if parent.ends_with('/') { parent.to_string() } else { format!("{parent}/") })
}

/// The pages a session running in `cwd` on `host` works on, the newest change first: on the same
/// machine, the ones its conversation (`session`) added, and those with the session inside the
/// page's project or the page's folder under the session's. A session in a home folder or higher
/// (two folders deep or less) gets no pages by folder below it, or every page on the machine would
/// be its own.
pub fn session_pages(pages: &[PageEntry], host: &str, cwd: Option<&str>, session: Option<&str>) -> Vec<PageEntry> {
    let at = cwd.map(folder_key);
    let deep = at.as_deref().is_some_and(|a| {
        let rest = if drive(a) { &a[2..] } else { a };
        rest.split('/').filter(|s| !s.is_empty()).count() > 2
    });
    let mut v: Vec<PageEntry> = pages
        .iter()
        .filter(|p| {
            p.host == host
                && (session.is_some_and(|s| p.session.as_deref() == Some(s))
                    || at.as_deref().is_some_and(|a| within(a, &folder_key(&p.project)) || (deep && within(&folder_key(&p.folder), a))))
        })
        .cloned()
        .collect();
    v.sort_by(by_change);
    v
}

/// `decodeURIComponent`: None where JavaScript would throw.
fn decode_strict(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16);
            let (h, l) = (hex(*b.get(i + 1)?)?, hex(*b.get(i + 2)?)?);
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `\w` as JavaScript reads it.
fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The pages a reply links to, as `/pages/<host>/<slug>` (what `inyon pages add` prints), each once
/// and in order, with the host as this app names it: a session that came through a paired hub names
/// that hub's machines. The rule of lib/pages.ts's pattern, read by hand:
/// `(?:^|[^\w/.-])(?:https?://[^\s/]+)?/pages/([\w.%-]+)/([a-z0-9][a-z0-9-]{0,62})(?![\w-])`.
pub fn linked_pages(text: &str, session_host: &str) -> Vec<String> {
    const MARK: &str = "/pages/";
    let hub = if is_hub_host(session_host) { &session_host[..session_host.find('/').map_or(0, |i| i + 1)] } else { "" };
    let b = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    // where the last match ended: a match never starts inside the one before it
    let mut pos = 0usize;
    let mut from = 0usize;
    while let Some(off) = text[from..].find(MARK) {
        let i = from + off;
        from = i + 1;
        // what may stand in front: the start, a character outside [\w/.-], or a full address
        let boundary = |at: usize| at == 0 || (at > pos && !(word(b[at - 1]) || b"/.-".contains(&b[at - 1])));
        let direct = boundary(i);
        let address = || {
            let mut j = i;
            while j > 0 && !b[j - 1].is_ascii_whitespace() && b[j - 1] != b'/' {
                j -= 1;
            }
            if j == i {
                return false;
            }
            let scheme = ["https://", "http://"].into_iter().find(|s| text[..j].ends_with(s));
            scheme.is_some_and(|s| boundary(j - s.len()))
        };
        if !(direct || address()) {
            continue;
        }
        let mut k = i + MARK.len();
        let host_start = k;
        while k < b.len() && (word(b[k]) || b".%-".contains(&b[k])) {
            k += 1;
        }
        if k == host_start || b.get(k) != Some(&b'/') {
            continue;
        }
        let host = &text[host_start..k];
        k += 1;
        let slug_start = k;
        while k < b.len() && (b[k].is_ascii_lowercase() || b[k].is_ascii_digit() || b[k] == b'-') {
            k += 1;
        }
        let slug = &text[slug_start..k];
        let ok = !slug.is_empty() && slug.len() <= 63 && slug.as_bytes()[0] != b'-' && !b.get(k).is_some_and(|&c| word(c) || c == b'-');
        if !ok {
            continue;
        }
        pos = k;
        from = k;
        let Some(host) = decode_strict(host) else { continue };
        let id = page_id(&format!("{hub}{host}"), slug);
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

// ---------- changed since you last opened it ----------

pub const SEEN_KEY: &str = "inyon.pages.seen.v1";

/// When each page was last looked at, by id.
pub type Seen = HashMap<String, f64>;

/// The marks as saved: anything but an object of numbers is nothing.
pub fn parse_seen(raw: Option<&str>) -> Seen {
    let v: Option<serde_json::Map<String, serde_json::Value>> = raw.and_then(|s| serde_json::from_str(s).ok());
    v.map(|m| m.into_iter().filter_map(|(k, v)| v.as_f64().map(|n| (k, n))).collect()).unwrap_or_default()
}

pub fn seen_json(seen: &Seen) -> String {
    serde_json::to_string(seen).unwrap_or_else(|_| "{}".into())
}

/// A page counts as changed until it is opened, and again whenever it changes after that.
pub fn is_changed(p: &PageEntry, seen: &Seen) -> bool {
    seen.get(&id_of(p)).is_none_or(|&t| changed_at(p) > t)
}

/// The mark for a page looked at `now`.
pub fn seen_mark(p: &PageEntry, now: f64) -> f64 {
    now.max(changed_at(p))
}

// ---------- links ----------

/// A link is minted again when less than this is left of it: by a timer while the page is on
/// screen, and when the app comes back into view.
pub const RENEW_BEFORE_MS: f64 = 3_600_000.0;

pub fn should_renew(until: f64, now: f64) -> bool {
    until - now < RENEW_BEFORE_MS
}

/// A link with when it runs out by this device's clock.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLink {
    pub url: String,
    pub until: f64,
}

/// The hub says how long a link lasts (`ttl`), counted here from when it was asked for, since the
/// phone's clock and the hub's may not agree; a hub that doesn't say (no `ttl`, read as 0) gives
/// only its own `expires`.
pub fn local_deadline(l: &PageOpen, asked_at: f64) -> PageLink {
    PageLink { url: l.url.clone(), until: if l.ttl > 0 { asked_at + l.ttl as f64 } else { l.expires as f64 } }
}

/// How long to wait before renewing a link that is on screen: until an hour is left, at least 5 s.
pub fn renew_in(until: f64, now: f64) -> f64 {
    (until - RENEW_BEFORE_MS - now).max(5_000.0)
}

/// A timed renewal that failed is tried again after a minute, then twice as long each time, up to
/// 10 minutes.
pub const RENEW_RETRY_MS: f64 = 60_000.0;
pub const RENEW_RETRY_MAX_MS: f64 = 600_000.0;

pub fn next_retry(wait: f64) -> f64 {
    (wait * 2.0).min(RENEW_RETRY_MAX_MS)
}

/// Requests of one kind that can overlap (a link for a page, the check for its parts): each takes a
/// ticket, and only the answer to the newest one counts, so a slow early answer can't replace a
/// later one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Latest(u64);

impl Latest {
    pub fn ask(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    pub fn is_newest(&self, ticket: u64) -> bool {
        self.0 == ticket
    }
}

/// The parts of a Blueprint site the viewer can jump to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Plan,
    Mockups,
    Matrix,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Plan, Part::Mockups, Part::Matrix];

    pub fn path(self) -> &'static str {
        match self {
            Part::Plan => "",
            Part::Mockups => "mockups/",
            Part::Matrix => "matrix.html",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Part::Plan => "Plan",
            Part::Mockups => "Mockups",
            Part::Matrix => "Matrix",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Part::Plan => "plan",
            Part::Mockups => "mockups",
            Part::Matrix => "matrix",
        }
    }

    pub fn of(k: &str) -> Part {
        Part::ALL.into_iter().find(|p| p.key() == k).unwrap_or(Part::Plan)
    }
}

/// The frame's address: the token path (it ends in a slash, so the query survives without a
/// redirect), the part, and the app's theme for the page to start in.
pub fn frame_src(base: &str, url: &str, part: Part, theme: &str) -> String {
    format!("{base}{url}{}?theme={theme}", part.path())
}

/// A note in the site's `research/` folder, which the hub shows as a page to read (`?as=note`)
/// inside the same sandbox as the site.
pub fn note_src(base: &str, url: &str, note: &str, theme: &str) -> String {
    format!("{base}{url}research/{}?as=note&theme={theme}", inyon_chat_core::js::encode_uri_component(note))
}

/// A note's name as the app shows it: the file name without `.md`.
pub fn note_title(name: &str) -> String {
    if name.len() >= 3 && name[name.len() - 3..].eq_ignore_ascii_case(".md") {
        name[..name.len() - 3].to_string()
    } else {
        name.to_string()
    }
}

// ---------- words ----------

/// "just now", "3 min ago", "2 h ago", "4 d ago", or a date.
pub fn ago_text(t: f64, clock: Clock) -> String {
    let s = ((clock.now - t) / 1000.0).max(0.0);
    if s < 45.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{} min ago", (s / 60.0).round().max(1.0))
    } else if s < 86_400.0 {
        format!("{} h ago", (s / 3600.0).round())
    } else if s < 14.0 * 86_400.0 {
        format!("{} d ago", (s / 86_400.0).round())
    } else {
        local_time(t, clock.tz_minutes).month_day()
    }
}

/// "11 h 52 m" until a link runs out.
pub fn left_text(until: f64, now: f64) -> String {
    let m = ((until - now) / 60_000.0).floor().max(0.0) as u64;
    if m >= 60 {
        format!("{} h {} m", m / 60, m % 60)
    } else {
        format!("{m} m")
    }
}

/// "updated 3 min ago" by the board's own date, or when it was added when it has no board (or no
/// readable date).
pub fn updated_line(p: &PageEntry, clock: Clock) -> String {
    let t = p.progress.as_ref().and_then(|g| g.updated.as_deref()).map_or(f64::NAN, parse_date);
    if t.is_finite() {
        format!("updated {}", ago_text(t, clock))
    } else {
        format!("added {}", ago_text(p.added as f64, clock))
    }
}

/// A page's name for a screen reader, with what the card shows of it.
pub fn page_label(p: &PageEntry, changed: bool, online: bool, clock: Clock) -> String {
    let mut parts = vec![p.name.clone()];
    if changed {
        parts.push("changed".into());
    }
    if let Some(g) = &p.progress {
        parts.push(format!("{} of {} done", g.done, g.total));
        if let Some(n) = g.now.as_deref().filter(|n| !n.is_empty()) {
            parts.push(n.into());
        }
    }
    parts.push(if online { updated_line(p, clock) } else { "machine offline".into() });
    parts.join(", ")
}

/// Who added a page, as the Folder sheet says it.
pub fn added_by(by: &str) -> &str {
    match by {
        "cli" => "inyon pages add",
        "app" => "the app",
        other => other,
    }
}

/// The device sizes the desktop viewer offers; Fit fills the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Fit,
    Phone,
    Tablet,
    Desktop,
}

impl Device {
    /// name, width, height
    pub fn size(self) -> Option<(&'static str, f64, f64)> {
        match self {
            Device::Fit => None,
            Device::Phone => Some(("Phone", 390.0, 844.0)),
            Device::Tablet => Some(("Tablet", 820.0, 1180.0)),
            Device::Desktop => Some(("Desktop", 1440.0, 900.0)),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Device::Fit => "fit",
            Device::Phone => "phone",
            Device::Tablet => "tablet",
            Device::Desktop => "desktop",
        }
    }
}

/// How much a device frame shrinks to fit the stage, never grown past its real size.
pub fn fit_scale(w: f64, h: f64, stage_w: f64, stage_h: f64) -> f64 {
    const PAD: f64 = 24.0;
    ((stage_w - PAD) / w).min((stage_h - PAD) / h).clamp(0.1, 1.0)
}

/// The footer's size note: "Phone · 390 × 844 · 50%", or "Fit · 1100 × 760".
pub fn size_note(device: Device, stage_w: f64, stage_h: f64) -> String {
    match device.size() {
        Some((name, w, h)) => {
            let s = fit_scale(w, h, stage_w, stage_h);
            let pct = if s < 1.0 { format!(" · {}%", (s * 100.0).round()) } else { String::new() };
            format!("{name} · {w} × {h}{pct}")
        }
        None if stage_w > 0.0 => format!("Fit · {} × {}", (stage_w - 24.0).max(0.0), (stage_h - 24.0).max(0.0)),
        None => String::new(),
    }
}

// ---------- machines ----------

/// What the lists need to know about a page's machine.
#[derive(Debug, Clone, PartialEq)]
pub struct PageMachine {
    pub name: String,
    pub online: bool,
    /// the hub's own machine
    pub hub: bool,
    /// the machine's id and kind, when the hub lists it
    pub known: Option<(String, MachineKind)>,
}

/// The machine a page is on, by the hosts the app knows; the hub's own machine is "This hub" until
/// its name arrives.
pub fn machine_of(hosts: &[HostState], host: &str) -> PageMachine {
    let hub = is_hub_machine(host);
    match hosts.iter().find(|h| h.config.id == host) {
        None => PageMachine { name: if hub { "This hub".into() } else { host.to_string() }, online: true, hub, known: None },
        Some(h) => PageMachine {
            name: h.config.name.clone(),
            online: h.status == HostStatus::Online,
            hub: h.config.kind == HostKind::Local || hub,
            known: Some((h.config.id.clone(), crate::model::machine_kind(h))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_protocol::PageProgress;

    #[test]
    fn only_the_newest_request_counts() {
        let mut l = Latest::default();
        let open = l.ask();
        let revoke = l.ask();
        // the open answers last, with a link the revoke already ended
        assert!(l.is_newest(revoke));
        assert!(!l.is_newest(open));
    }

    fn entry(slug: &str) -> PageEntry {
        PageEntry {
            host: "local".into(),
            slug: slug.into(),
            name: slug.into(),
            project: "/home/me/code/app".into(),
            by: "claude".into(),
            added: 1_000,
            folder: format!("/home/me/code/app/private/plans/{slug}"),
            progress: None,
            notes: None,
            session: None,
            session_name: None,
        }
    }

    fn with(slug: &str, f: impl FnOnce(&mut PageEntry)) -> PageEntry {
        let mut e = entry(slug);
        f(&mut e);
        e
    }

    fn iso(ms: u64) -> String {
        let t = local_time(ms as f64, 0);
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", t.year, t.month, t.day, t.hour, t.minute, (ms / 1000) % 60, ms % 1000)
    }

    fn progress(done: u32, total: u32, updated: &str) -> Option<PageProgress> {
        Some(PageProgress { done, total, now: None, updated: Some(updated.into()) })
    }

    fn name_of(h: &str) -> String {
        match h {
            "local" => "studio",
            "hub:h1/local" => "arm",
            "laptop" => "air",
            other => other,
        }
        .to_string()
    }

    fn slugs(v: &[PageEntry]) -> Vec<&str> {
        v.iter().map(|p| p.slug.as_str()).collect()
    }

    fn none() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn machine_then_project_then_page_with_the_hub_first_and_the_newest_change_on_top() {
        let pages = [
            with("old", |e| e.added = 10),
            with("win", |e| {
                e.host = "laptop".into();
                e.project = "C:\\demo".into();
                e.added = 5;
            }),
            with("busy", |e| {
                e.added = 1;
                e.progress = progress(1, 2, &iso(50));
            }),
            with("site", |e| {
                e.project = "/home/me/code/site".into();
                e.added = 20;
            }),
        ];
        let refs: Vec<&PageEntry> = pages.iter().collect();
        let groups = group_pages(&refs, name_of, &none());
        assert_eq!(groups.iter().map(|g| g.host.as_str()).collect::<Vec<_>>(), ["local", "laptop"]);
        let first: Vec<(&str, Vec<&str>)> = groups[0].projects.iter().map(|p| (p.name.as_str(), slugs(&p.pages))).collect();
        assert_eq!(first, [("app", vec!["busy", "old"]), ("site", vec!["site"])]);
        // a Windows path names its project by its last folder too
        assert_eq!(groups[1].projects[0].name, "demo");
    }

    #[test]
    fn other_machines_follow_by_name_and_a_paired_hubs_own_machine_counts_as_a_hub_machine() {
        let pages = [with("a", |e| e.host = "zeta".into()), with("b", |e| e.host = "hub:h1/local".into()), with("c", |e| e.host = "alpha".into())];
        let refs: Vec<&PageEntry> = pages.iter().collect();
        let groups = group_pages(&refs, name_of, &none());
        assert_eq!(groups.iter().map(|g| g.host.as_str()).collect::<Vec<_>>(), ["hub:h1/local", "alpha", "zeta"]);
    }

    #[test]
    fn the_filter_looks_at_the_name_the_project_and_the_machine() {
        let p = with("plan", |e| {
            e.name = "Inyon plan".into();
            e.project = "/srv/custom-tmux".into();
        });
        assert!(matches_page(&p, "studio", "inyon", &none()));
        assert!(matches_page(&p, "studio", "CUSTOM", &none()));
        assert!(matches_page(&p, "studio", "stud", &none()));
        assert!(!matches_page(&p, "studio", "handy", &none()));
        assert!(matches_page(&p, "studio", "  ", &none()));
        assert_eq!(project_name("/a/b/c/"), "c");
    }

    #[test]
    fn a_sessions_pages_are_grouped_under_the_name_the_session_has_now() {
        let pages = [
            with("plan", |e| {
                e.session = Some("chat-1".into());
                e.session_name = Some("Handy redesign".into());
            }),
            with("mock", |e| e.session = Some("chat-1".into())),
            entry("other"),
            with("gone", |e| e.session = Some("chat-9".into())),
        ];
        // renamed while it runs: the group follows the running session's name
        let running = HashMap::from([(page_id("local", "chat-1"), "Legato development".to_string())]);
        let refs: Vec<&PageEntry> = pages.iter().collect();
        let groups = group_pages(&refs, name_of, &running);
        let got: Vec<(&str, Vec<&str>)> = groups[0]
            .projects
            .iter()
            .map(|p| {
                let mut s = slugs(&p.pages);
                s.sort();
                (p.name.as_str(), s)
            })
            .collect();
        assert_eq!(got, [("app", vec!["gone", "other"]), ("Legato development", vec!["mock", "plan"])]);
        // not running: the name Inyon last gave it, else the project folder
        assert_eq!(group_name(&pages[0], &none()), "Handy redesign");
        assert_eq!(group_name(&pages[1], &none()), "app");
        assert!(matches_page(&pages[1], "studio", "legato", &running));
        assert!(!matches_page(&pages[2], "studio", "legato", &running));
    }

    #[test]
    fn running_sessions_name_their_pages_by_the_title_they_show_not_the_agents_own_name() {
        let pane = |id: &str, title: &str, sid: Option<&str>| {
            let mut agent = serde_json::json!({ "kind": "claude", "pid": 1, "state": "idle" });
            if let Some(s) = sid {
                agent["sessionId"] = s.into();
            }
            serde_json::json!({ "id": id, "index": 0, "active": true, "command": "claude", "cwd": "/x", "title": title, "width": 80, "height": 24, "dead": false, "agent": agent })
        };
        let host: HostState = serde_json::from_value(serde_json::json!({
            "config": { "id": "local", "name": "studio", "kind": "local" }, "status": "online", "integration": "installed", "tmux": "ok", "agents": [], "updatedAt": 1,
            "sessions": [{ "id": "$1", "name": "s", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "claude", "active": true, "activity": 0,
                "panes": [pane("%1", "✳ Legato development", Some("chat-1")), pane("%2", "Claude Code", Some("chat-2")), pane("%3", "✳ No id", None)] }] }]
        }))
        .unwrap();
        let names = running_names(&[host]);
        assert_eq!(names.into_iter().collect::<Vec<_>>(), [("local/chat-1".to_string(), "Legato development".to_string())]);
    }

    fn session_fixture() -> Vec<PageEntry> {
        vec![
            with("plan", |e| e.added = 5),
            with("site", |e| {
                e.project = "/home/me/code/site".into();
                e.folder = "/home/me/code/site/docs".into();
                e.added = 9;
            }),
            with("scratch", |e| {
                e.project = "/home/me/scratch/x".into();
                e.folder = "/home/me/scratch/x".into();
            }),
            with("win", |e| {
                e.host = "laptop".into();
                e.project = "C:\\Dev\\Robot".into();
                e.folder = "C:\\Dev\\Robot\\plans".into();
            }),
        ]
    }

    #[test]
    fn a_session_gets_its_projects_pages_from_anywhere_inside_it() {
        let all = session_fixture();
        assert_eq!(slugs(&session_pages(&all, "local", Some("/home/me/code/app"), None)), ["plan"]);
        assert_eq!(slugs(&session_pages(&all, "local", Some("/home/me/code/app/apps/web/"), None)), ["plan"]);
        // not a folder that only starts the same
        assert!(session_pages(&all, "local", Some("/home/me/code/application"), None).is_empty());
    }

    #[test]
    fn a_session_gets_pages_in_folders_under_it_but_not_every_page_under_a_home_folder() {
        let all = session_fixture();
        assert_eq!(slugs(&session_pages(&all, "local", Some("/home/me/code"), None)), ["site", "plan"]);
        assert!(session_pages(&all, "local", Some("/home/me"), None).is_empty());
        assert!(session_pages(&all, "local", Some("/"), None).is_empty());
    }

    #[test]
    fn a_session_gets_the_same_machines_pages_only_windows_paths_in_any_case_and_slash() {
        let all = session_fixture();
        assert_eq!(slugs(&session_pages(&all, "laptop", Some("c:/dev/robot"), None)), ["win"]);
        assert!(session_pages(&all, "laptop", Some("/home/me/code/app"), None).is_empty());
        assert!(session_pages(&all, "local", None, None).is_empty());
        assert_eq!(folder_key("C:\\Dev\\"), "c:/dev");
        assert_eq!(folder_key("/"), "/");
    }

    #[test]
    fn a_session_gets_the_pages_its_conversation_added_wherever_they_are() {
        let mut all = session_fixture();
        all.push(with("mine", |e| {
            e.project = "/srv/elsewhere".into();
            e.folder = "/srv/elsewhere/plan".into();
            e.session = Some("chat-1".into());
        }));
        assert_eq!(slugs(&session_pages(&all, "local", Some("/home/me/code/app"), Some("chat-1"))), ["mine", "plan"]);
        assert_eq!(slugs(&session_pages(&all, "local", None, Some("chat-1"))), ["mine"]);
        assert_eq!(slugs(&session_pages(&all, "laptop", Some("c:/dev/robot"), Some("chat-1"))), ["win"]);
    }

    #[test]
    fn a_reply_links_to_what_inyon_pages_add_prints_each_once_and_the_full_address() {
        let text = "Added it. Open it in the app at /pages/local/my-plan\nor `/pages/local/my-plan`, and https://hub.example:8080/pages/laptop/robot.";
        assert_eq!(linked_pages(text, "local"), ["local/my-plan", "laptop/robot"]);
        // a session through a paired hub names that hub's machines
        assert_eq!(linked_pages("at /pages/local/my-plan", "hub:h1/laptop"), ["hub:h1/local/my-plan"]);
        // not a source path that has pages in it
        assert!(linked_pages("see apps/web/src/pages/Pages.tsx and /home/me/pages/local/x", "local").is_empty());
        // escaped machine names, and ones JavaScript can't decode
        assert_eq!(linked_pages("(/pages/box%20one/plan)", "local"), ["box one/plan"]);
        assert!(linked_pages("/pages/bad%zz/plan", "local").is_empty());
        // a slug that runs on into capitals or past 63 characters isn't one
        assert!(linked_pages("/pages/local/planB", "local").is_empty());
        assert!(linked_pages(&format!("/pages/local/{}", "a".repeat(64)), "local").is_empty());
        assert_eq!(linked_pages("/pages/local/a-b-, then", "local"), ["local/a-b-"]);
    }

    #[test]
    fn a_page_never_opened_counts_as_changed_and_opening_it_clears_that_until_its_progress_moves() {
        let p = with("plan", |e| e.added = 100);
        assert!(is_changed(&p, &Seen::new()));
        let seen = Seen::from([(id_of(&p), seen_mark(&p, 500.0))]);
        assert!(!is_changed(&p, &seen));
        assert_eq!(seen_json(&seen), r#"{"local/plan":500.0}"#);
        // the web app's marks read the same
        assert_eq!(parse_seen(Some(r#"{"local/plan":500,"x":"no"}"#)), Seen::from([("local/plan".to_string(), 500.0)]));
        assert!(parse_seen(Some("[1]")).is_empty() && parse_seen(Some("not json")).is_empty() && parse_seen(None).is_empty());
        let mut later = p.clone();
        later.progress = progress(2, 9, &iso(900));
        assert_eq!(changed_at(&later), 900.0);
        assert!(is_changed(&later, &seen));
        // a progress time that isn't a date falls back to when the page was added
        let odd = with("x", |e| {
            e.added = 42;
            e.progress = progress(0, 1, "soon");
        });
        assert_eq!(changed_at(&odd), 42.0);
    }

    const NOW: f64 = 1_000_000_000.0;

    #[test]
    fn a_link_is_renewed_when_less_than_an_hour_is_left_by_a_timer_never_sooner_than_5_s() {
        assert_eq!(RENEW_BEFORE_MS, 3_600_000.0);
        assert!(!should_renew(NOW + 12.0 * 3_600_000.0, NOW));
        assert!(!should_renew(NOW + 3_600_001.0, NOW));
        assert!(should_renew(NOW + 3_599_999.0, NOW));
        assert!(should_renew(NOW - 1.0, NOW));
        assert_eq!(renew_in(NOW + 12.0 * 3_600_000.0, NOW), 11.0 * 3_600_000.0);
        assert_eq!(renew_in(NOW + 3_600_001.0, NOW), 5_000.0);
        assert_eq!(renew_in(NOW - 1.0, NOW), 5_000.0);
        assert_eq!(next_retry(RENEW_RETRY_MS), 120_000.0);
        assert_eq!(next_retry(400_000.0), RENEW_RETRY_MAX_MS);
    }

    #[test]
    fn a_links_end_is_counted_on_this_devices_clock_from_the_hubs_ttl() {
        // the hub's clock is two hours ahead of this one
        let hub_expires = (NOW + 14.0 * 3_600_000.0) as u64;
        let l = PageOpen { url: "/v/t/".into(), expires: hub_expires, ttl: 12 * 3_600_000 };
        assert_eq!(local_deadline(&l, NOW).until, NOW + 12.0 * 3_600_000.0);
        // a hub that doesn't say has only its own expiry
        let old = PageOpen { ttl: 0, ..l };
        assert_eq!(local_deadline(&old, NOW).until, hub_expires as f64);
    }

    #[test]
    fn the_frames_address_keeps_the_token_paths_slash_so_the_theme_survives_without_a_redirect() {
        assert_eq!(frame_src("", "/v/v1.abc.def/", Part::Plan, "dark"), "/v/v1.abc.def/?theme=dark");
        assert_eq!(frame_src("http://box:4777", "/v/t/", Part::Mockups, "light"), "http://box:4777/v/t/mockups/?theme=light");
        assert_eq!(frame_src("", "/v/t/", Part::Matrix, "light"), "/v/t/matrix.html?theme=light");
        assert_eq!(note_src("", "/v/t/", "Field notes.md", "dark"), "/v/t/research/Field%20notes.md?as=note&theme=dark");
        assert_eq!(note_title("Field notes.MD"), "Field notes");
        assert_eq!(note_title("raw.txt"), "raw.txt");
    }

    #[test]
    fn words_for_how_long_ago_and_how_long_is_left() {
        let c = Clock { now: NOW, tz_minutes: 0 };
        assert_eq!(ago_text(NOW - 10_000.0, c), "just now");
        assert_eq!(ago_text(NOW - 3.0 * 60_000.0, c), "3 min ago");
        assert_eq!(ago_text(NOW - 2.0 * 3_600_000.0, c), "2 h ago");
        assert_eq!(ago_text(NOW - 4.0 * 86_400_000.0, c), "4 d ago");
        // 1_000_000_000 ms is 12 Jan 1970; 20 days before is 23 Dec 1969
        assert_eq!(ago_text(NOW - 20.0 * 86_400_000.0, c), "Dec 23");
        assert_eq!(left_text(NOW + (11.0 * 60.0 + 52.0) * 60_000.0, NOW), "11 h 52 m");
        assert_eq!(left_text(NOW + 30.0 * 60_000.0, NOW), "30 m");
        assert_eq!(left_text(NOW - 5.0, NOW), "0 m");
        let p = with("plan", |e| e.added = (NOW - 120_000.0) as u64);
        assert_eq!(updated_line(&p, c), "added 2 min ago");
        let mut busy = p.clone();
        busy.progress = Some(PageProgress { done: 3, total: 8, now: Some("Wiring the viewer".into()), updated: Some(iso((NOW - 10_000.0) as u64)) });
        assert_eq!(updated_line(&busy, c), "updated just now");
        assert_eq!(page_label(&busy, true, true, c), "plan, changed, 3 of 8 done, Wiring the viewer, updated just now");
        assert_eq!(page_label(&p, false, false, c), "plan, machine offline");
        assert_eq!(added_by("cli"), "inyon pages add");
    }

    #[test]
    fn a_device_frame_shrinks_to_fit_the_stage_and_never_grows() {
        assert_eq!(fit_scale(390.0, 844.0, 2000.0, 2000.0), 1.0);
        assert!((fit_scale(1440.0, 900.0, 744.0, 1000.0) - 0.5).abs() < 1e-9);
        assert!((fit_scale(390.0, 844.0, 800.0, 446.0) - 0.5).abs() < 1e-9);
        assert_eq!(size_note(Device::Desktop, 744.0, 1000.0), "Desktop · 1440 × 900 · 50%");
        assert_eq!(size_note(Device::Phone, 2000.0, 2000.0), "Phone · 390 × 844");
        assert_eq!(size_note(Device::Fit, 1124.0, 784.0), "Fit · 1100 × 760");
        assert_eq!(Part::of("matrix"), Part::Matrix);
    }

    #[test]
    fn a_pages_machine_is_the_hubs_until_its_name_arrives() {
        let m = machine_of(&[], "local");
        assert_eq!((m.name.as_str(), m.online, m.hub), ("This hub", true, true));
        let h: HostState = serde_json::from_value(serde_json::json!({ "config": { "id": "laptop", "name": "air", "kind": "node" }, "status": "error",
            "integration": "installed", "tmux": "ok", "agents": [], "sessions": [], "updatedAt": 1 }))
        .unwrap();
        let m = machine_of(&[h], "laptop");
        assert_eq!((m.name.as_str(), m.online, m.hub), ("air", false, false));
        assert!(is_hub_machine("hub:h1/local") && !is_hub_machine("hub:h1/box") && !is_hub_host("hub:/x"));
    }
}
