//! `inyon` with nothing after it: what this machine is to the app, where it answers, who's
//! paired, what's running. Read from the state files, so it's on screen before the engine
//! could have started.

use std::fs;
use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde::Serialize;

use super::{banner, fit, pad, Theme, VERSION};
use crate::paths::{node_dir, saved_hub};
use crate::service::{self, Role, Sys};
use crate::state;
use crate::sys::which;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Job {
    pub role: String,
    pub service: bool,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PairedPhone {
    pub key: String,
    pub name: String,
    pub paired: String,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Status {
    pub v: u32,
    pub cli: String,
    pub engine: Option<String>,
    /// hub, node (paired with the phone), joined (a hub's node) or none
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub machine: Option<String>,
    pub jobs: Vec<Job>,
    pub addresses: Vec<String>,
    pub relays: Vec<String>,
    pub phones: Vec<PairedPhone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<String>>,
    pub agents: Vec<String>,
    pub pairing_open: bool,
    pub log: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// how phones reach this computer, when it's only through the tunnel
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reach: Option<String>,
    /// what runs in the background, what runs it, and how to stop it
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

impl Status {
    /// Whether the phone link runs, as a service or started by hand.
    pub fn link_running(&self) -> bool {
        self.jobs.iter().any(|j| j.role == "link" && j.running)
    }

    /// Where the link answers: the addresses it last published, while it runs. A link that has
    /// stopped leaves them in state.json, and they don't answer any more.
    pub fn answering(&self) -> Vec<&str> {
        if !self.link_running() {
            return Vec::new();
        }
        self.addresses.iter().chain(self.relays.iter()).map(String::as_str).collect()
    }
}

/// What to say when phones reach this computer only through the tunnel: neither the LAN nor a
/// VPN is on (the default on Windows, where both need a firewall rule) and there's no address of
/// your own. Says how to let phones on the same network in.
pub fn tunnel_only(cfg: &state::LinkConfig, windows: bool) -> Option<String> {
    let tunnel = cfg.tunnel.unwrap_or(true);
    let lan = cfg.lan.unwrap_or(false);
    let vpn = cfg.vpn.unwrap_or(!windows);
    let own = cfg.urls.as_ref().is_some_and(|u| !u.is_empty()) || cfg.relays.as_ref().is_some_and(|r| !r.is_empty());
    if !tunnel || lan || vpn || own {
        return None;
    }
    let mut s = "Phones reach this computer only through the tunnel, not over the same Wi-Fi or VPN. To let them in that way, run `inyon setup` and tick LAN or VPN".to_string();
    s.push_str(if windows { "; Windows then asks to add the firewall rule." } else { "." });
    Some(s)
}

/// Reads everything the screen shows. `agents` costs a tmux call, so it's optional.
pub fn gather(sys: &Sys, with_agents: bool) -> Status {
    let link = state::link_state();
    let hub = saved_hub();
    let jobs: Vec<Job> = service::ROLES
        .iter()
        .map(|r| {
            let running = crate::update::read_running(*r);
            let alive = running.as_ref().and_then(|s| s.pid).is_some_and(|p| sys.proc.alive(p));
            // a link whose pid record is stale still answers on its port
            let answers = || *r == Role::Link && service::link_answers(state::link_config().port.unwrap_or(4780));
            Job { role: r.name().to_string(), service: service::has_service(sys, *r), running: alive || answers(), version: running.and_then(|s| s.version) }
        })
        .collect();
    let has = |r: Role| jobs.iter().any(|j| j.role == r.name() && (j.service || j.running));
    let role = if has(Role::Hub) {
        "hub"
    } else if hub.is_some() || has(Role::Node) {
        "joined"
    } else if has(Role::Link) || state::identity().is_some() {
        "node"
    } else {
        "none"
    };
    // the engine and the hub are built into this program
    let engine = jobs.iter().find_map(|j| if j.running { j.version.clone() } else { None }).or_else(|| Some(VERSION.to_string()));
    let phones = state::phones()
        .into_iter()
        .map(|(key, p)| PairedPhone { key: key.chars().take(12).collect(), name: p.name, paired: state::format_date(p.paired_at), last_seen: p.last_seen.map(state::format_time) })
        .collect();
    let nodes = (role == "hub").then(hub_nodes).flatten();
    let agents = if with_agents { running_agents() } else { Vec::new() };
    // why the hub connection can't connect, or the plain http it warns about
    let running = jobs.iter().any(|j| j.role == Role::Node.name() && j.running);
    let note = (role == "joined" && running).then(crate::commands::connection_note).flatten().map(|(_, s)| s);
    let reach = (role == "node" || has(Role::Link)).then(|| tunnel_only(&state::link_config(), cfg!(windows))).flatten();
    let background = background_line(sys.platform, crate::app::running(sys.proc).as_ref(), &jobs);
    Status {
        v: 1,
        cli: VERSION.to_string(),
        engine,
        role: role.to_string(),
        hub,
        machine: state::identity().and_then(|i| i.id),
        jobs,
        addresses: link.as_ref().map(|s| s.urls.clone()).unwrap_or_default(),
        relays: link.as_ref().map(|s| s.relays.clone()).unwrap_or_default(),
        phones,
        nodes,
        agents,
        pairing_open: state::pairing_open(),
        log: last_log_lines(&node_dir(), 6),
        note,
        reach,
        background,
    }
}

/// What `inyon status` says runs in the background: the Inyon app and what it runs on Windows,
/// the services elsewhere, with how to stop it. None when nothing runs.
pub fn background_line(platform: crate::sys::Platform, app: Option<&crate::app::AppState>, jobs: &[Job]) -> Option<String> {
    if let Some(line) = app.and_then(crate::app::status_line) {
        return Some(line);
    }
    let running: Vec<&str> = jobs
        .iter()
        .filter(|j| j.running)
        .map(|j| match j.role.as_str() {
            "link" => "the phone link",
            "node" => "the hub connection",
            _ => "the hub",
        })
        .collect();
    if running.is_empty() {
        return None;
    }
    // elsewhere it may be a user service, a login item or a copy started with nohup: not said
    let by = if platform == crate::sys::Platform::Windows { ", started outside the Inyon app (an older version, or a terminal)" } else { "" };
    // `inyon stop` leaves a hub running: phones and other machines depend on it
    let stop = match (running.contains(&"the hub"), running.len()) {
        (false, _) => "`inyon stop` stops it",
        (true, 1) => "`inyon hub stop` stops it",
        (true, _) => "`inyon stop` stops the rest, `inyon hub stop` the hub",
    };
    Some(format!("Running in the background: {}{by}. {stop}.", running.join(", ")))
}

/// The request for the hub's machines: the local token in a header, not in the address, where a
/// proxy's or the hub's log would keep it (every hub since 0.13 takes either).
fn hub_nodes_request(hub: &str, token: &str) -> (String, Vec<(String, String)>) {
    (format!("{hub}/api/local/nodes"), vec![("authorization".to_owned(), format!("Bearer {token}"))])
}

/// The hub's machines, through its loopback API (hub 0.13 and on; older hubs have none).
fn hub_nodes() -> Option<Vec<String>> {
    let token = fs::read_to_string(crate::paths::data_dir().join("local-token")).ok()?.trim().to_string();
    let env = crate::paths::read_env_file(&crate::paths::data_dir().join("hub.env"));
    let (url, headers) = hub_nodes_request(&crate::commands::hub_local_url(&env), &token);
    let res = crate::http::get_local(&url, &headers).ok()?;
    if !res.ok() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&res.body).ok()?;
    Some(v.get("nodes")?.as_array()?.iter().filter_map(|n| n.get("name").or(n.get("hostname")).and_then(|s| s.as_str()).map(String::from)).collect())
}

const AGENTS: [&str; 5] = ["claude", "codex", "gemini", "opencode", "aider"];

/// The coding agents in tmux panes right now (one tmux call; nothing when tmux isn't running).
pub fn running_agents() -> Vec<String> {
    if which("tmux").is_none() {
        return Vec::new();
    }
    let r = crate::sys::run(&["tmux", "list-panes", "-a", "-F", "#{session_name}:#{window_index}.#{pane_index} #{pane_current_command}"]);
    if !r.ok {
        return Vec::new();
    }
    parse_agents(&r.out)
}

pub fn parse_agents(list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|l| {
            let (pane, cmd) = l.split_once(' ')?;
            let name = cmd.trim().rsplit('/').next()?.to_lowercase();
            AGENTS.iter().any(|a| name == *a).then(|| format!("{} in {pane}", cmd.trim()))
        })
        .collect()
}

/// The last `n` lines of whichever log changed most recently.
pub fn last_log_lines(dir: &Path, n: usize) -> Vec<String> {
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for name in ["link.log", "node.log", "hub.log", "update.log"] {
        let p = dir.join(name);
        if let Ok(m) = fs::metadata(&p).and_then(|m| m.modified()) {
            if best.as_ref().is_none_or(|(t, _)| m > *t) {
                best = Some((m, p));
            }
        }
    }
    let Some((_, p)) = best else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&p) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    lines.iter().rev().take(n).rev().map(|l| format!("{name}: {}", state::local_log_line(l))).collect()
}

fn role_line(s: &Status) -> String {
    match s.role.as_str() {
        "hub" => "Hub: the server phones sign in to".to_string(),
        "joined" => format!("Machine on the hub {}", s.hub.as_deref().map(crate::http::host_of).unwrap_or_default()),
        "node" => "Paired with the phone".to_string(),
        _ => "Not set up yet: run `inyon setup`".to_string(),
    }
}

fn dot(theme: Theme, on: bool, attention: bool) -> Span<'static> {
    if on {
        Span::styled("●", theme.strong())
    } else if attention {
        Span::styled("●", theme.attention())
    } else {
        Span::styled("○", theme.faint())
    }
}

/// How many rows each part of the screen keeps, whatever it shows: a refresh changes the text
/// in a row, never where the rows are. A part with less to say leaves blank rows; one with more
/// ends in "and N more".
pub const JOB_ROWS: usize = 2;
pub const ADDRESS_ROWS: usize = 2;
pub const REACH_ROWS: usize = 2;
pub const PHONE_ROWS: usize = 3;
pub const NODE_ROWS: usize = 3;
pub const AGENT_ROWS: usize = 3;
pub const LOG_ROWS: usize = 5;

/// `lines` in exactly `n` rows: blank rows after them, or the last row saying how many didn't fit.
fn rows(mut lines: Vec<Line<'static>>, n: usize, theme: Theme) -> Vec<Line<'static>> {
    if lines.len() > n && n > 0 {
        let more = lines.len() - (n - 1);
        lines.truncate(n - 1);
        lines.push(Line::from(Span::styled(format!("  and {more} more"), theme.faint())));
    }
    lines.resize(n, Line::raw(""));
    lines
}

fn job_lines(s: &Status, theme: Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for j in &s.jobs {
        if !j.service && !j.running {
            continue;
        }
        let what = match j.role.as_str() {
            "link" => "phone link",
            "node" => "hub connection",
            _ => "hub",
        };
        let how = match (j.running, j.service) {
            (true, true) => "running",
            (true, false) => "running, started by hand",
            _ => "set up, not running",
        };
        let version = j.version.as_deref().map(|v| format!(" · {v}")).unwrap_or_default();
        out.push(Line::from(vec![
            dot(theme, j.running, j.service && !j.running),
            Span::raw(" "),
            Span::styled(what.to_string(), theme.body()),
            Span::styled(format!("  {how}{version}"), theme.muted()),
        ]));
    }
    if out.is_empty() {
        out.push(Line::from(vec![dot(theme, false, false), Span::styled(" nothing runs in the background", theme.muted())]));
    }
    out
}

/// The left column: what this machine is, what runs, where it answers and how phones reach it.
fn left_lines(s: &Status, theme: Theme, width: usize) -> Vec<Line<'static>> {
    let mut left: Vec<Line> = vec![Line::from(Span::styled(fit(&role_line(s), width), theme.body()))];
    left.push(match &s.machine {
        Some(m) => Line::from(Span::styled(format!("machine {}", fit(m, 28)), theme.faint())),
        None => Line::raw(""),
    });
    left.push(Line::raw(""));
    left.extend(rows(job_lines(s, theme), JOB_ROWS, theme));
    left.push(Line::raw(""));
    let answering = s.answering();
    let head = if !answering.is_empty() {
        "Answers on"
    } else if s.link_running() {
        "No address yet: the tunnel is starting"
    } else if s.jobs.iter().any(|j| j.role == "link" && j.service) {
        "Doesn't answer: the phone link isn't running"
    } else {
        ""
    };
    left.push(Line::from(Span::styled(fit(head, width), theme.muted())));
    let addresses: Vec<Line> = answering.iter().map(|a| Line::from(Span::raw(format!("  {}", fit(a, width.saturating_sub(2)))))).collect();
    left.extend(rows(addresses, ADDRESS_ROWS, theme));
    left.push(Line::raw(""));
    let reach: Vec<String> = s.reach.as_deref().map(|r| super::wrap(r, width)).unwrap_or_default();
    // the advice is cut to its rows rather than counted: the last row ends in an ellipsis
    let mut reach: Vec<Line> = reach.into_iter().map(|l| Line::from(Span::styled(l, theme.muted()))).collect();
    if reach.len() > REACH_ROWS {
        reach.truncate(REACH_ROWS);
        if let Some(last) = reach.last_mut() {
            let text: String = last.spans.iter().map(|sp| sp.content.as_ref()).collect();
            *last = Line::from(Span::styled(fit(&format!("{text} …"), width), theme.muted()));
        }
    }
    reach.resize(REACH_ROWS, Line::raw(""));
    left.extend(reach);
    left
}

/// The right column: who's paired, the hub's machines, and the agents in tmux.
fn right_lines(s: &Status, theme: Theme, width: usize) -> Vec<Line<'static>> {
    let mut right: Vec<Line> = Vec::new();
    right.push(Line::from(Span::styled(if s.phones.is_empty() { "No phones paired yet".to_string() } else { format!("Phones ({})", s.phones.len()) }, theme.muted())));
    // the names padded to the longest shown, so "seen" lines up; times short, as the window has them
    let name_w = s.phones.iter().take(PHONE_ROWS).map(|p| unicode_width::UnicodeWidthStr::width(p.name.as_str())).max().unwrap_or(0).min(18).min(width.saturating_sub(4));
    let today = state::format_date(state::now_ms());
    let phones: Vec<Line> = s
        .phones
        .iter()
        .map(|p| {
            let seen = p.last_seen.as_deref().map(|t| format!("seen {}", crate::desktop::pairing::seen_short(t, &today))).unwrap_or_else(|| "never seen".to_string());
            Line::from(vec![Span::raw(format!("  {}", pad(&p.name, name_w))), Span::styled(fit(&format!("  {seen}"), width.saturating_sub(name_w + 2)), theme.faint())])
        })
        .collect();
    right.extend(rows(phones, PHONE_ROWS, theme));
    right.push(if s.pairing_open { Line::from(vec![Span::styled("  ● ", theme.attention()), Span::styled("a pairing code is open", theme.muted())]) } else { Line::raw("") });
    // a hub's machines: the role doesn't change while the screen is open, so neither does this
    if s.role == "hub" {
        right.push(Line::raw(""));
        let nodes = s.nodes.clone().unwrap_or_default();
        right.push(Line::from(Span::styled(format!("Machines on this hub ({})", nodes.len()), theme.muted())));
        right.extend(rows(nodes.iter().map(|n| Line::from(Span::raw(format!("  {}", fit(n, width.saturating_sub(2)))))).collect(), NODE_ROWS, theme));
    }
    right.push(Line::raw(""));
    right.push(Line::from(Span::styled(if s.agents.is_empty() { "No agents running in tmux" } else { "Agents running" }, theme.muted())));
    right.extend(rows(s.agents.iter().map(|a| Line::from(Span::raw(format!("  {}", fit(a, width.saturating_sub(2)))))).collect(), AGENT_ROWS, theme));
    right
}

/// The screen, for a terminal or a TestBackend. Every row has a fixed place for a given role and
/// terminal size, so the refresh every 2 s changes text, never where it is.
pub fn render(f: &mut Frame, s: &Status, theme: Theme) {
    let area = f.area();
    let narrow = area.width < 72;
    let head = banner(area.width);
    let rows_ = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(head.len() as u16 + 1), Constraint::Min(4), Constraint::Length(1)]).split(area);
    let mut top: Vec<Line> = head.iter().map(|l| Line::from(Span::styled(l.clone(), theme.strong()))).collect();
    top.push(Line::from(Span::styled("─".repeat(area.width as usize), theme.line())));
    f.render_widget(Paragraph::new(top), rows_[0]);

    let left_w = if narrow { area.width as usize } else { (area.width as usize) * 58 / 100 };
    let right_w = if narrow { area.width as usize } else { (area.width as usize) - left_w };
    let left = left_lines(s, theme, left_w.saturating_sub(1));
    let right = right_lines(s, theme, right_w.saturating_sub(1));
    // the parts first, then the log in what's left (up to its rows), keeping three rows for it so a
    // note always shows: both depend on the terminal's size and the role, never on a refresh
    let body_h = rows_[1].height as usize;
    let content_h = (if narrow { left.len() + 1 + right.len() } else { left.len().max(right.len()) }).min(body_h.saturating_sub(3));
    let log_h = (body_h - content_h).min(2 + LOG_ROWS);
    let body =
        Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(content_h as u16), Constraint::Length(log_h as u16), Constraint::Min(0)]).split(rows_[1]);
    if narrow {
        let mut all = left;
        all.push(Line::raw(""));
        all.extend(right);
        f.render_widget(Paragraph::new(all), body[0]);
    } else {
        let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(left_w as u16), Constraint::Min(0)]).split(body[0]);
        f.render_widget(Paragraph::new(left), cols[0]);
        f.render_widget(Paragraph::new(right), cols[1]);
    }
    // the log section: a note (the engine isn't installed, the hub can't be reached) heads it in
    // place of the oldest lines, so it shows on any terminal and nothing outside the log moves
    let mut below: Vec<Line> = vec![Line::raw(""), Line::from(Span::styled(if s.log.is_empty() { "Log: nothing yet" } else { "Log" }, theme.muted()))];
    let room = log_h.saturating_sub(2);
    let note: Vec<Line> = s
        .note
        .as_deref()
        .map(|n| super::wrap(n, (area.width as usize).saturating_sub(2)))
        .unwrap_or_default()
        .into_iter()
        .take(2.min(room))
        .enumerate()
        .map(|(i, l)| Line::from(vec![Span::styled(if i == 0 { "! " } else { "  " }, theme.attention()), Span::raw(l)]))
        .collect();
    // then what runs in the background and how to stop it
    let bg: Vec<Line> = s
        .background
        .as_deref()
        .map(|b| super::wrap(b, (area.width as usize).saturating_sub(2)))
        .unwrap_or_default()
        .into_iter()
        .take(2.min(room - note.len()))
        .map(|l| Line::from(vec![Span::styled("  ", theme.muted()), Span::styled(l, theme.muted())]))
        .collect();
    let lines = room - note.len() - bg.len();
    below.extend(note);
    below.extend(bg);
    let log: Vec<Line> = s.log.iter().rev().take(lines).rev().map(|l| Line::from(Span::styled(fit(l, area.width as usize), theme.faint()))).collect();
    below.extend(rows(log, lines, theme));
    below.truncate(log_h);
    f.render_widget(Paragraph::new(below), body[1]);
    let foot = Rect { x: area.x, y: area.y + area.height - 1, width: area.width, height: 1 };
    f.render_widget(Paragraph::new(Line::from(Span::styled(fit("q quit · r refresh · s setup · p pair", area.width as usize), theme.faint()))), foot);
}

/// The same, as text for a pipe or a file.
pub fn plain(s: &Status) -> String {
    let mut out = vec![format!("inyon {}", s.cli)];
    out.push(role_line(s));
    if let Some(m) = &s.machine {
        out.push(format!("machine {m}"));
    }
    for j in &s.jobs {
        if j.service || j.running {
            out.push(format!("{}: {}{}", j.role, if j.running { "running" } else { "set up, not running" }, j.version.as_deref().map(|v| format!(" ({v})")).unwrap_or_default()));
        }
    }
    if let Some(b) = &s.background {
        out.push(b.clone());
    }
    for a in s.answering() {
        out.push(format!("answers on {a}"));
    }
    if let Some(r) = &s.reach {
        out.push(r.clone());
    }
    if s.phones.is_empty() {
        out.push("no phones paired yet".into());
    }
    for p in &s.phones {
        out.push(format!("phone {} ({}), paired {}, last seen {}", p.name, p.key, p.paired, p.last_seen.as_deref().unwrap_or("never")));
    }
    if let Some(nodes) = &s.nodes {
        for n in nodes {
            out.push(format!("machine on this hub: {n}"));
        }
    }
    for a in &s.agents {
        out.push(format!("agent {a}"));
    }
    if let Some(n) = &s.note {
        out.push(n.clone());
    }
    for l in &s.log {
        out.push(l.clone());
    }
    out.extend(hints(s));
    out.join("\n")
}

/// What to read next, for a person or an agent working on this computer: where its pages show
/// when it's joined to a hub, and where the docs are.
pub fn hints(s: &Status) -> Vec<String> {
    let mut out = Vec::new();
    if s.role == "joined" {
        out.push("Sessions and pages on this machine show in the hub's app. Share a folder of pages with `inyon pages add <folder>` (hub 0.22 or later).".to_string());
    }
    out.push(format!("Docs: {} or `inyon docs <topic>`", crate::docs::DOCS_URL));
    out
}

/// `inyon status --json`: the status with the docs address and the hints.
pub fn json(s: &Status) -> serde_json::Value {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.insert("docs".into(), crate::docs::DOCS_URL.into());
        o.insert("hints".into(), hints(s).into());
    }
    v
}

/// A rendered buffer as lines, trailing spaces trimmed, for the snapshot tests.
pub fn buffer_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut lines = Vec::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// What a key on the screen asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Quit,
    Refresh,
    /// leave the screen and run this command (`setup` goes through the wizard)
    Run(&'static str),
}

/// The key's meaning. Only a press counts: Windows also reports each key's release, which used
/// to run a command twice or quit straight after it.
pub fn key_action(k: &crossterm::event::KeyEvent) -> Option<Key> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    if k.kind != KeyEventKind::Press {
        return None;
    }
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => Some(Key::Quit),
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Some(Key::Quit),
        KeyCode::Char('r') => Some(Key::Refresh),
        KeyCode::Char('s') => Some(Key::Run("setup")),
        KeyCode::Char('p') => Some(Key::Run("pair")),
        _ => None,
    }
}

/// Shows the screen until q or Esc; r reads everything again.
pub fn run_screen(sys: &Sys, theme: Theme) -> std::io::Result<()> {
    use crossterm::event::{self, Event};
    let mut terminal = ratatui::init();
    let mut status = gather(sys, true);
    let result = (|| -> std::io::Result<Option<&'static str>> {
        loop {
            terminal.draw(|f| render(f, &status, theme))?;
            if event::poll(std::time::Duration::from_secs(2))? {
                if let Event::Key(k) = event::read()? {
                    match key_action(&k) {
                        Some(Key::Quit) => return Ok(None),
                        Some(Key::Run(next)) => return Ok(Some(next)),
                        Some(Key::Refresh) => {}
                        // a key that does nothing (or a release) doesn't read everything again
                        None => continue,
                    }
                }
            }
            status = gather(sys, true);
        }
    })();
    ratatui::restore();
    match &result {
        Ok(Some(next)) => crate::commands::run_named(next),
        // the screen ran on the terminal's second buffer, so what was there before is back;
        // what it showed stays in the scrollback as plain lines, like any command's output
        Ok(None) => println!("{}", plain(&status)),
        Err(_) => {}
    }
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_local_token_goes_in_a_header_not_the_address() {
        let (url, headers) = super::hub_nodes_request("http://127.0.0.1:4777", "s3cret-local-token");
        assert_eq!(url, "http://127.0.0.1:4777/api/local/nodes");
        assert!(!url.contains("s3cret"));
        assert_eq!(headers, [("authorization".to_owned(), "Bearer s3cret-local-token".to_owned())]);
    }

    use super::*;

    fn status() -> Status {
        Status {
            v: 1,
            cli: "0.19.0".into(),
            engine: Some("0.19.0".into()),
            role: "node".into(),
            hub: None,
            machine: Some("k7m2p9x4".into()),
            jobs: vec![Job { role: "link".into(), service: true, running: true, version: Some("0.19.0".into()) }],
            addresses: vec!["wss://quiet-otter.trycloudflare.com/link".into()],
            relays: vec![],
            phones: vec![],
            nodes: None,
            agents: vec![],
            pairing_open: false,
            log: vec![],
            note: None,
            reach: None,
            background: None,
        }
    }

    fn screen(s: &Status) -> String {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        t.draw(|f| render(f, s, Theme::plain())).unwrap();
        buffer_text(t.backend().buffer())
    }

    #[test]
    fn status_says_where_the_docs_are_and_where_a_joined_machines_pages_show() {
        let s = status();
        assert!(plain(&s).ends_with("Docs: https://inyon.tech/docs/ or `inyon docs <topic>`"), "{}", plain(&s));
        assert!(!plain(&s).contains("pages add"));
        let joined = Status { role: "joined".into(), hub: Some("http://100.64.0.1:4747".into()), ..status() };
        assert!(plain(&joined).contains("`inyon pages add <folder>`"), "{}", plain(&joined));
        let v = json(&joined);
        assert_eq!(v["docs"], "https://inyon.tech/docs/");
        assert_eq!(v["hints"].as_array().unwrap().len(), 2);
        assert_eq!(v["role"], "joined");
    }

    #[test]
    fn a_link_that_has_stopped_answers_nowhere() {
        let running = status();
        assert!(plain(&running).contains("answers on wss://quiet-otter.trycloudflare.com/link"));
        assert!(screen(&running).contains("Answers on"));
        // state.json still names the tunnel it had, which is gone with it
        let mut stopped = status();
        stopped.jobs[0].running = false;
        assert!(!plain(&stopped).contains("answers on"), "{}", plain(&stopped));
        assert!(!screen(&stopped).contains("Answers on"));
        assert!(!screen(&stopped).contains("trycloudflare"));
    }

    #[test]
    fn tunnel_only_is_said_with_how_to_let_the_same_network_in() {
        let cfg = |lan: Option<bool>, vpn: Option<bool>| state::LinkConfig { lan, vpn, ..Default::default() };
        // the defaults: on Windows the tunnel only, elsewhere the VPN too
        let windows = tunnel_only(&cfg(None, None), true).unwrap();
        assert!(windows.contains("only through the tunnel") && windows.contains("`inyon setup`") && windows.contains("firewall rule"), "{windows}");
        assert_eq!(tunnel_only(&cfg(None, None), false), None);
        let elsewhere = tunnel_only(&cfg(None, Some(false)), false).unwrap();
        assert!(!elsewhere.contains("firewall"));
        assert_eq!(tunnel_only(&cfg(Some(true), None), true), None);
        assert_eq!(tunnel_only(&cfg(None, Some(true)), true), None);
        assert_eq!(tunnel_only(&state::LinkConfig { tunnel: Some(false), ..Default::default() }, true), None);
        assert_eq!(tunnel_only(&state::LinkConfig { urls: Some(vec!["wss://a.example/link".into()]), ..Default::default() }, true), None);
        let s = Status { reach: Some(windows.clone()), ..status() };
        assert!(plain(&s).contains(&windows));
        assert!(screen(&s).contains("only through the tunnel"));
    }

    fn row_of(screen: &str, starts: &str) -> usize {
        screen.lines().position(|l| l.contains(starts)).unwrap_or_else(|| panic!("no row with {starts:?} in\n{screen}"))
    }

    #[test]
    fn rows_keep_their_places_whatever_the_refresh_brings() {
        let quiet = status();
        // everything that changes between two refreshes, at once
        let mut busy = status();
        busy.jobs[0].running = false;
        busy.addresses = vec!["ws://192.168.1.20:4780/link".into(), "ws://100.97.40.12:4780/link".into(), "wss://a.trycloudflare.com/link".into()];
        busy.phones = (0..6).map(|i| PairedPhone { key: format!("k{i}"), name: format!("Phone {i}"), paired: "2026-10-14".into(), last_seen: None }).collect();
        busy.pairing_open = true;
        busy.agents = vec!["claude in work:1.0".into(), "codex in ops:0.0".into(), "gemini in a:0.0".into(), "aider in b:0.0".into()];
        busy.log = (0..6).map(|i| format!("link.log: line {i}")).collect();
        busy.reach = Some(tunnel_only(&state::LinkConfig::default(), true).unwrap());
        for (w, h) in [(100, 30), (60, 48)] {
            let draw = |s: &Status| {
                let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
                t.draw(|f| render(f, s, Theme::plain())).unwrap();
                buffer_text(t.backend().buffer())
            };
            let (a, b) = (draw(&quiet), draw(&busy));
            assert_eq!(row_of(&a, "No phones paired yet"), row_of(&b, "Phones (6)"), "{a}\n---\n{b}");
            assert_eq!(row_of(&a, "No agents running"), row_of(&b, "Agents running"), "{a}\n---\n{b}");
            assert_eq!(row_of(&a, "Log"), row_of(&b, "Log"), "{a}\n---\n{b}");
            assert_eq!(row_of(&a, "q quit"), row_of(&b, "q quit"));
            // more than fits says so in its last row
            assert!(b.contains("and 4 more") && b.contains("and 2 more"), "{b}");
        }
    }

    #[test]
    fn a_note_shows_on_a_short_terminal_without_moving_the_log() {
        let mut s = status();
        s.log = (0..6).map(|i| format!("link.log: line {i}")).collect();
        let draw = |s: &Status| {
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            t.draw(|f| render(f, s, Theme::plain())).unwrap();
            buffer_text(t.backend().buffer())
        };
        let before = draw(&s);
        s.note = Some("The engine isn't installed yet: run `inyon setup`.".into());
        let after = draw(&s);
        assert!(after.contains("! The engine isn't installed yet"), "{after}");
        assert_eq!(row_of(&before, "Log"), row_of(&after, "Log"), "{before}\n---\n{after}");
        // the newest log line still shows, the oldest made room
        assert!(after.contains("line 5") && before.contains("line 1") && !after.contains("line 1"), "{after}");
    }

    #[test]
    fn only_a_key_press_counts_not_its_release() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let key = |c: char, kind: KeyEventKind| KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE, kind, state: KeyEventState::NONE };
        assert_eq!(key_action(&key('s', KeyEventKind::Press)), Some(Key::Run("setup")));
        assert_eq!(key_action(&key('s', KeyEventKind::Release)), None);
        assert_eq!(key_action(&key('p', KeyEventKind::Repeat)), None);
        assert_eq!(key_action(&key('q', KeyEventKind::Press)), Some(Key::Quit));
        assert_eq!(key_action(&key('x', KeyEventKind::Press)), None);
    }

    #[test]
    fn a_phone_named_in_korean_keeps_its_last_seen_column_in_line() {
        let mut s = status();
        s.phones = vec![
            PairedPhone { key: "a".into(), name: "갤럭시 탭 S9 울트라 플러스".into(), paired: "x".into(), last_seen: Some("22:27".into()) },
            PairedPhone { key: "b".into(), name: "Pixel 8".into(), paired: "x".into(), last_seen: Some("09:01".into()) },
        ];
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        t.draw(|f| render(f, &s, Theme::plain())).unwrap();
        let buf = t.backend().buffer().clone();
        // the terminal column each "seen" starts in, counted in cells as the terminal draws them
        let seen: Vec<u16> =
            (0..buf.area.height).filter_map(|y| (0..buf.area.width.saturating_sub(4)).find(|&x| (0..4).map(|i| buf[(x + i, y)].symbol()).collect::<String>() == "seen")).collect();
        assert_eq!(seen.len(), 2, "{}", buffer_text(&buf));
        assert_eq!(seen[0], seen[1], "{}", buffer_text(&buf));
    }

    #[test]
    fn the_background_line_names_the_command_that_stops_what_runs() {
        let job = |role: &str| Job { role: role.into(), service: true, running: true, version: None };
        let line = |jobs: &[Job]| background_line(crate::sys::Platform::Linux, None, jobs).unwrap();
        assert_eq!(line(&[job("link")]), "Running in the background: the phone link. `inyon stop` stops it.");
        assert_eq!(line(&[job("hub")]), "Running in the background: the hub. `inyon hub stop` stops it.");
        assert_eq!(line(&[job("hub"), job("node")]), "Running in the background: the hub, the hub connection. `inyon stop` stops the rest, `inyon hub stop` the hub.");
        assert_eq!(background_line(crate::sys::Platform::Linux, None, &[Job { running: false, ..job("link") }]), None);
    }

    #[test]
    fn agents_are_picked_out_of_the_pane_list() {
        let list = "work:0.0 zsh\nwork:1.0 claude\nops:0.0 /usr/local/bin/codex\nops:1.0 vim\n";
        assert_eq!(parse_agents(list), vec!["claude in work:1.0", "/usr/local/bin/codex in ops:0.0"]);
    }

    #[test]
    fn the_newest_log_gives_its_last_lines() {
        let dir = std::env::temp_dir().join(format!("inyon-dash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("update.log"), "a\nb\n\nc\n").unwrap();
        assert_eq!(last_log_lines(&dir, 2), vec!["update.log: b", "update.log: c"]);
        assert!(last_log_lines(&dir.join("nope"), 2).is_empty());
        // the engine's UTC stamps are shown in local time
        fs::write(dir.join("link.log"), "2025-10-14T11:01:23.456Z tunnel: up\n").unwrap();
        assert_eq!(last_log_lines(&dir, 1), vec![format!("link.log: {}", state::local_log_line("2025-10-14T11:01:23Z tunnel: up"))]);
        assert!(!last_log_lines(&dir, 1)[0].contains('Z'));
        fs::remove_dir_all(&dir).unwrap();
    }
}
