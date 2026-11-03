//! The overview as the screen draws it, worked out from the machines the hub sends (`overview` on
//! `/ws`). What the React app keeps in lib/overview.ts and lib/groups.ts comes from the shared view
//! model (crates/inyon-chat-core); this adds what pages/Home.tsx and the overview's components
//! work out for themselves.

pub use inyon_chat_core::groups::tmux_target;
use inyon_chat_core::groups::{group_panes as group_by_session, GroupFilter};
pub use inyon_chat_core::overview::{all_panes, pane_title, PaneRef};
use inyon_protocol::{is_busy, AgentKind, AgentPaneInfo, AgentState, HostKind, HostState, HostStatus, MachineKind, RateWindow, TmuxState};

use crate::format::{self, Clock};

/// What the overview needs of a pane beyond the shared view model.
pub trait AgentPane<'a> {
    /// Only for panes running an agent; the overview shows no others.
    fn agent(&self) -> &'a AgentPaneInfo;
    /// A key that stays the same while the pane lives.
    fn key(&self) -> String;
}

impl<'a> AgentPane<'a> for PaneRef<'a> {
    fn agent(&self) -> &'a AgentPaneInfo {
        self.pane.agent.as_ref().expect("an agent pane")
    }

    fn key(&self) -> String {
        format!("{}{}", self.host.config.id, self.pane.id)
    }
}

/// An agent pane the overview shows as a card; the companion has a line of its own.
fn on_a_card(r: &PaneRef) -> bool {
    r.pane.agent.is_some() && r.pane.companion != Some(true)
}

/// Agent panes, the ones that need you first, then the most recently active.
pub fn agent_panes(hosts: &[HostState]) -> Vec<PaneRef<'_>> {
    let mut v = inyon_chat_core::overview::agent_panes(hosts);
    v.retain(on_a_card);
    v
}

/// Working, or idle with shells or tasks still running behind the prompt.
pub fn busy(a: &AgentPaneInfo) -> bool {
    is_busy(a.state, a.background.as_deref())
}

pub fn agent_name(a: &AgentPaneInfo) -> String {
    a.custom.clone().unwrap_or_else(|| a.kind.def().name.clone())
}

/// What a session's chip says. On a machine that isn't answering a session is shown as last seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Working,
    Background,
    Waiting,
    Idle,
    Unknown,
    Offline,
}

impl Shown {
    pub fn of(a: &AgentPaneInfo, host: &HostState) -> Shown {
        if host.status != HostStatus::Online {
            return Shown::Offline;
        }
        match a.state {
            AgentState::Idle if a.background.as_deref().is_some_and(|b| !b.is_empty()) => Shown::Background,
            AgentState::Idle => Shown::Idle,
            AgentState::Working => Shown::Working,
            AgentState::Waiting => Shown::Waiting,
            AgentState::Unknown => Shown::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Shown::Working => "Working",
            Shown::Background => "Background",
            Shown::Waiting => "Needs you",
            Shown::Idle => "Idle",
            Shown::Unknown => "Running",
            Shown::Offline => "Offline",
        }
    }

    /// The class the chip's dot takes (app.css): ink with a ring while working, colour only when
    /// something needs you.
    pub fn class(self) -> &'static str {
        match self {
            Shown::Working => "working",
            Shown::Background => "background",
            Shown::Waiting => "waiting",
            Shown::Idle => "idle",
            Shown::Unknown => "idle",
            Shown::Offline => "offline",
        }
    }
}

/// The card's middle line: what the agent is doing, asking, or when it last did something.
pub fn card_line(r: &PaneRef, clock: Clock) -> String {
    let a = r.agent();
    match a.state {
        AgentState::Working => a.activity.clone().unwrap_or_else(|| "Working…".into()),
        AgentState::Waiting => a.waiting_for.clone().unwrap_or_default(),
        _ => {
            if let Some(bg) = a.background.as_deref().filter(|b| !b.is_empty()) {
                format::background_text(bg)
            } else if let Some(f) = &a.failure {
                format::failure_text(f, clock)
            } else {
                format!("Active {}", format::time_ago(*r.window.activity * 1000.0, clock))
            }
        }
    }
}

/// "Sonnet 5.5 · medium", the model and effort the agent reported.
pub fn model_line(a: &AgentPaneInfo) -> String {
    let Some(st) = &a.status else { return String::new() };
    [st.model.as_deref(), st.effort.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The card's grey line: the machine (when the list isn't grouped by machine), where in tmux, and
/// the folder.
pub fn card_meta(r: &PaneRef, show_host: bool) -> String {
    let mut parts = vec![];
    if show_host {
        parts.push(r.host.config.name.clone());
    }
    parts.push(tmux_target(r));
    let path = format::short_path(&r.pane.cwd, 34);
    if !path.is_empty() {
        parts.push(path);
    }
    parts.join(" · ")
}

/// Agent panes grouped by machine, each in a place that doesn't change: machines in the order they
/// were added, sessions in the order tmux started them, panes in window and pane order. Nothing
/// moves when an agent starts working or asks something, so a card is still where you aimed.
#[derive(Debug)]
pub struct HostGroup<'a> {
    pub host: &'a HostState,
    pub panes: Vec<PaneRef<'a>>,
}

/// The overview's All / Working / Idle switch, shown once there are more than three sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Working,
    Idle,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Working, Filter::Idle];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Working => "Working",
            Filter::Idle => "Idle",
        }
    }

    /// Working counts work left running behind an idle prompt; idle is everything else.
    pub fn keeps(self, a: &AgentPaneInfo) -> bool {
        match self {
            Filter::All => true,
            Filter::Working => busy(a),
            Filter::Idle => !busy(a),
        }
    }
}

/// Past this many sessions the overview offers the filter.
pub const FILTER_FROM: usize = 4;

/// The shared grouping (machine, then tmux session) with the sessions run together: the overview
/// shows one list of cards per machine.
pub fn group_panes(hosts: &[HostState], filter: Filter) -> Vec<HostGroup<'_>> {
    let keep = |r: &PaneRef| on_a_card(r) && filter.keeps(r.agent());
    group_by_session(hosts, &GroupFilter { query: None, keep: Some(&keep) })
        .into_iter()
        .map(|g| HostGroup { host: g.host, panes: g.sessions.into_iter().flat_map(|s| s.panes).collect() })
        .collect()
}

/// The line under the title: who needs you, else what's working, else that it's quiet.
pub fn status_line(panes: &[PaneRef]) -> String {
    let waiting = panes.iter().filter(|r| r.agent().state == AgentState::Waiting).count();
    let working = panes.iter().filter(|r| busy(r.agent())).count();
    let in_front = panes.iter().filter(|r| r.agent().state == AgentState::Working).count();
    let behind = working - in_front;
    if waiting > 0 {
        format!("{waiting} session{} you.", if waiting > 1 { "s need" } else { " needs" })
    } else if in_front > 0 && behind > 0 {
        format!("{in_front} working, {behind} in the background.")
    } else if in_front > 0 {
        format!("{in_front} session{} working.", if in_front > 1 { "s are" } else { " is" })
    } else if behind > 0 {
        format!("{behind} session{} work running in the background.", if behind > 1 { "s have" } else { " has" })
    } else if !panes.is_empty() {
        "All quiet. Every session is idle.".into()
    } else {
        "No agents are running right now.".into()
    }
}

pub fn pane_count(h: &HostState) -> usize {
    h.sessions.iter().flat_map(|s| &s.windows).map(|w| w.panes.len()).sum()
}

/// "2 machines · 4 agent sessions · 6 panes", the header's count on a wide screen.
pub fn counts_line(hosts: &[HostState], agent_sessions: usize) -> String {
    let panes: usize = hosts.iter().map(pane_count).sum();
    format!("{} · {} · {}", format::plural(hosts.len(), "machine"), format::plural(agent_sessions, "agent session"), format::plural(panes, "pane"))
}

/// One line about a machine in the list: its sessions when online, when it was last seen when not.
pub fn machine_line(h: &HostState, clock: Clock) -> String {
    if h.status == HostStatus::Online {
        if h.tmux == TmuxState::Missing {
            return "tmux not installed".into();
        }
        return format!("{} · {}", format::plural(h.sessions.len(), "session"), format::plural(pane_count(h), "pane"));
    }
    if let Some(seen) = h.last_seen {
        return format::last_seen(seen, clock);
    }
    // the reason, when there is one, beats a bare "Connecting…"
    match (&h.error, h.status) {
        (Some(e), _) => e.clone(),
        (None, HostStatus::Error) => "Not reachable".into(),
        _ => "Connecting…".into(),
    }
}

/// The table's tmux column.
pub fn tmux_cell(h: &HostState) -> String {
    if h.status != HostStatus::Online {
        return if h.last_seen.is_some() { "Offline".into() } else { "—".into() };
    }
    if h.tmux == TmuxState::Missing {
        return "Not installed".into();
    }
    format!("{} · {}", format::plural(h.sessions.len(), "session"), format::plural(pane_count(h), "pane"))
}

/// The table's grey line under a machine's name: why it's down, this hub, or how it's reached.
pub fn machine_address(h: &HostState, hosts: &[HostState]) -> String {
    if h.status == HostStatus::Error {
        return h.error.clone().unwrap_or_default();
    }
    if h.config.kind == HostKind::Local {
        return match &h.os {
            Some(os) => format!("This hub · {os}"),
            None => "This hub".into(),
        };
    }
    let target = h.config.target.clone().unwrap_or_default();
    match &h.config.via {
        Some(via) => {
            let name = hosts.iter().find(|x| &x.config.id == via).map_or("a removed computer".to_string(), |x| x.config.name.clone());
            format!("{target} via {name}")
        }
        None => target,
    }
}

/// What a machine's icon draws: what it reported, or a server for this hub before its first look.
pub fn machine_kind(h: &HostState) -> MachineKind {
    h.machine_kind.unwrap_or(if h.config.kind == HostKind::Local { MachineKind::Server } else { MachineKind::Desktop })
}

/// The agents installed on a machine, with how many of each run there now.
pub fn agent_badges(h: &HostState) -> Vec<(AgentKind, usize)> {
    h.agents
        .iter()
        .map(|k| {
            let n = h.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes).filter(|p| p.agent.as_ref().is_some_and(|a| a.kind == *k)).count();
            (*k, n)
        })
        .collect()
}

/// One agent's plan limits on one machine, from the freshest reading there: a session's status or
/// the account itself.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitRow {
    pub host: String,
    pub host_id: String,
    pub kind: AgentKind,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub resets: Option<u32>,
}

pub fn limit_rows(hosts: &[HostState]) -> Vec<LimitRow> {
    let mut rows = vec![];
    for h in hosts {
        // (kind, five hour, seven day, read at), freshest per kind, in the order first seen
        let mut best: Vec<(AgentKind, Option<RateWindow>, Option<RateWindow>, u64)> = vec![];
        let mut offer = |kind: AgentKind, five: &Option<RateWindow>, seven: &Option<RateWindow>, at: u64| {
            if five.is_none() && seven.is_none() {
                return;
            }
            match best.iter_mut().find(|b| b.0 == kind) {
                Some(b) if b.3 < at => *b = (kind, five.clone(), seven.clone(), at),
                Some(_) => {}
                None => best.push((kind, five.clone(), seven.clone(), at)),
            }
        };
        for r in all_panes(std::slice::from_ref(h)) {
            if let Some(st) = r.pane.agent.as_ref().and_then(|a| a.status.as_ref().map(|s| (a.kind, s))) {
                offer(st.0, &st.1.five_hour, &st.1.seven_day, st.1.updated_at);
            }
        }
        for l in h.limits.iter().flatten() {
            offer(l.kind, &l.five_hour, &l.seven_day, l.updated_at);
        }
        for (kind, five_hour, seven_day, _) in best {
            let resets = h.limits.iter().flatten().find(|l| l.kind == kind).and_then(|l| l.resets).filter(|n| *n > 0);
            rows.push(LimitRow { host: h.config.name.clone(), host_id: h.config.id.clone(), kind, five_hour, seven_day, resets });
        }
    }
    rows
}

/// A meter's colour: ink, then amber from 60%, red from 85%.
pub fn meter_tone(pct: f64) -> &'static str {
    if pct >= 85.0 {
        "danger"
    } else if pct >= 60.0 {
        "warn"
    } else {
        "ink"
    }
}

/// Where a session opens: this app's route for it, as the web app's (`/m/<machine>/p/<pane
/// without %>`) under /next.
pub fn pane_href(host: &str, pane: &str) -> String {
    crate::route::Route::Session { host: host.into(), pane: pane.into(), view: None }.href()
}

/// A machine's page in this app.
pub fn machine_href(host: &str) -> String {
    crate::route::Route::Machine { id: host.into() }.href()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: f64 = 1_793_448_000_000.0;
    const CLOCK: Clock = Clock { now: NOW, tz_minutes: 0 };

    fn pane(id: &str, title: &str, agent: serde_json::Value) -> serde_json::Value {
        json!({ "id": id, "index": 0, "active": true, "command": "claude", "cwd": "/home/sam/code/app", "title": title,
            "width": 80, "height": 40, "dead": false, "agent": agent })
    }

    fn agent(kind: &str, state: &str) -> serde_json::Value {
        json!({ "kind": kind, "pid": 1, "state": state })
    }

    fn host(id: &str, sessions: serde_json::Value) -> HostState {
        serde_json::from_value(json!({ "config": { "id": id, "name": id, "kind": "node" }, "status": "online", "integration": "installed",
            "tmux": "ok", "agents": ["claude", "codex"], "sessions": sessions, "updatedAt": 0 }))
        .unwrap()
    }

    fn session(id: &str, name: &str, activity: u64, panes: serde_json::Value) -> serde_json::Value {
        json!({ "id": id, "name": name, "attached": 0, "windows": [{ "id": "@1", "index": 1, "name": "", "active": true, "activity": activity, "panes": panes }] })
    }

    #[test]
    fn sessions_that_need_you_come_first_then_the_latest() {
        let hosts = vec![host(
            "box",
            json!([
                session("$1", "a", 100, json!([pane("%1", "✳ Idle one", agent("claude", "idle"))])),
                session("$2", "b", 300, json!([pane("%2", "✳ Busy one", agent("claude", "working"))])),
                session("$3", "c", 200, json!([pane("%3", "Asks", agent("codex", "waiting"))])),
                session("$4", "d", 400, json!([pane("%4", "Newer idle", agent("claude", "idle"))])),
            ]),
        )];
        let order: Vec<_> = agent_panes(&hosts).iter().map(|r| r.pane.id.clone()).collect();
        assert_eq!(order, ["%3", "%2", "%4", "%1"]);
    }

    #[test]
    fn groups_keep_tmux_order_and_skip_terminals_and_the_companion() {
        let mut companion = pane("%9", "Companion", agent("claude", "idle"));
        companion["companion"] = json!(true);
        let mut shell = pane("%8", "bash", serde_json::Value::Null);
        shell.as_object_mut().unwrap().remove("agent");
        let hosts = vec![
            host(
                "one",
                json!([
                    session("$10", "late", 1, json!([pane("%2", "B", agent("claude", "working"))])),
                    session("$2", "early", 1, json!([pane("%1", "A", agent("claude", "idle")), shell, companion]))
                ]),
            ),
            host("two", json!([])),
        ];
        let g = group_panes(&hosts, Filter::All);
        assert_eq!(g.len(), 1, "a machine with no agent sessions has no group");
        assert_eq!(g[0].panes.iter().map(|r| r.pane.id.as_str()).collect::<Vec<_>>(), ["%1", "%2"]);
        assert_eq!(agent_panes(&hosts).len(), 2);
        let working = group_panes(&hosts, Filter::Working);
        assert_eq!(working[0].panes.iter().map(|r| r.pane.id.as_str()).collect::<Vec<_>>(), ["%2"]);
        assert_eq!(group_panes(&hosts, Filter::Idle)[0].panes.len(), 1);
    }

    #[test]
    fn work_behind_an_idle_prompt_counts_as_working() {
        let a: AgentPaneInfo = serde_json::from_value(json!({ "kind": "opencode", "pid": 1, "state": "idle", "background": "1 shell" })).unwrap();
        assert!(Filter::Working.keeps(&a) && !Filter::Idle.keeps(&a) && Filter::All.keeps(&a));
    }

    #[test]
    fn titles_drop_the_spinner_and_generic_names() {
        let hosts = vec![host(
            "box",
            json!([session("$1", "app", 1, json!([pane("%1", "✳ Fix login bug", agent("claude", "idle")), pane("%2", "Claude Code", agent("claude", "idle"))]))]),
        )];
        let p = all_panes(&hosts);
        assert_eq!(pane_title(&p[0]), "Fix login bug");
        // the window is unnamed, so the agent's name
        assert_eq!(pane_title(&p[1]), "Claude Code");
        assert_eq!(tmux_target(&p[0]), "app:1.0");
        assert_eq!(card_meta(&p[0], true), "box · app:1.0 · ~/code/app");
        assert_eq!(pane_href("box 1", "%12"), "/next/m/box%201/p/12");
    }

    #[test]
    fn the_status_line_counts_what_matters() {
        let make = |states: &[(&str, Option<&str>)]| {
            let panes: Vec<_> = states
                .iter()
                .enumerate()
                .map(|(i, (s, bg))| {
                    let mut a = agent("claude", s);
                    if let Some(bg) = bg {
                        a["background"] = json!(bg);
                    }
                    pane(&format!("%{i}"), "t", a)
                })
                .collect();
            vec![host("box", json!([session("$1", "a", 1, json!(panes))]))]
        };
        let line = |h: &Vec<HostState>| status_line(&agent_panes(h));
        assert_eq!(line(&make(&[("waiting", None), ("working", None)])), "1 session needs you.");
        assert_eq!(line(&make(&[("waiting", None), ("waiting", None)])), "2 sessions need you.");
        assert_eq!(line(&make(&[("working", None), ("idle", Some("1 shell"))])), "1 working, 1 in the background.");
        assert_eq!(line(&make(&[("working", None), ("working", None)])), "2 sessions are working.");
        assert_eq!(line(&make(&[("idle", Some("1 shell"))])), "1 session has work running in the background.");
        assert_eq!(line(&make(&[("idle", None)])), "All quiet. Every session is idle.");
        assert_eq!(line(&make(&[])), "No agents are running right now.");
    }

    #[test]
    fn a_card_says_what_its_agent_is_doing() {
        let mut working = agent("claude", "working");
        working["activity"] = json!("Running tests…");
        let mut waiting = agent("codex", "waiting");
        waiting["waitingFor"] = json!("Run bun test?");
        let mut failed = agent("claude", "idle");
        failed["failure"] = json!({ "error": "rate_limit" });
        let hosts = vec![host(
            "box",
            json!([session(
                "$1",
                "a",
                (NOW / 1000.0) as u64 - 600,
                json!([pane("%1", "t", working), pane("%2", "t", waiting), pane("%3", "t", failed), pane("%4", "t", agent("claude", "idle"))])
            )]),
        )];
        let p = all_panes(&hosts);
        let lines: Vec<_> = p.iter().map(|r| card_line(r, CLOCK)).collect();
        assert_eq!(lines, ["Running tests…", "Run bun test?", "Stopped: rate limit", "Active 10m ago"]);
        assert_eq!(Shown::of(p[1].agent(), p[1].host), Shown::Waiting);
        let mut offline = hosts[0].clone();
        offline.status = HostStatus::Connecting;
        assert_eq!(Shown::of(p[0].agent(), &offline).label(), "Offline");
    }

    #[test]
    fn limits_take_the_freshest_reading_per_agent() {
        let mut a = agent("claude", "idle");
        a["status"] = json!({ "updatedAt": 10, "fiveHour": { "pct": 20 } });
        let mut b = agent("claude", "idle");
        b["status"] = json!({ "updatedAt": 30, "fiveHour": { "pct": 35 }, "sevenDay": { "pct": 50 } });
        let mut h = host("box", json!([session("$1", "a", 1, json!([pane("%1", "t", a), pane("%2", "t", b)]))]));
        h.limits = Some(vec![
            serde_json::from_value(json!({ "kind": "codex", "sevenDay": { "pct": 27 }, "resets": 2, "updatedAt": 5 })).unwrap(),
            serde_json::from_value(json!({ "kind": "claude", "fiveHour": { "pct": 99 }, "updatedAt": 20 })).unwrap(),
        ]);
        let rows = limit_rows(&[h]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].kind, *rows[0].five_hour.as_ref().unwrap().pct), (AgentKind::Claude, 35.0));
        assert_eq!((rows[1].kind, rows[1].resets), (AgentKind::Codex, Some(2)));
        assert_eq!(meter_tone(59.0), "ink");
        assert_eq!(meter_tone(60.0), "warn");
        assert_eq!(meter_tone(85.0), "danger");
    }

    #[test]
    fn machines_in_one_line() {
        let mut h = host("box", json!([session("$1", "a", 1, json!([pane("%1", "t", agent("claude", "working"))]))]));
        assert_eq!(machine_line(&h, CLOCK), "1 session · 1 pane");
        assert_eq!(agent_badges(&h), [(AgentKind::Claude, 1), (AgentKind::Codex, 0)]);
        assert_eq!(counts_line(std::slice::from_ref(&h), 1), "1 machine · 1 agent session · 1 pane");
        h.status = HostStatus::Error;
        h.error = Some("ssh: connection refused".into());
        assert_eq!(machine_line(&h, CLOCK), "ssh: connection refused");
        assert_eq!(machine_address(&h, &[]), "ssh: connection refused");
        h.last_seen = Some(NOW as u64 - 2 * 3_600_000);
        assert_eq!(machine_line(&h, CLOCK), "Offline · last seen 2 h ago");
        assert_eq!(tmux_cell(&h), "Offline");
        h.status = HostStatus::Online;
        h.config.kind = HostKind::Local;
        h.os = Some("Linux".into());
        assert_eq!(machine_address(&h, &[]), "This hub · Linux");
        assert_eq!(machine_kind(&h), MachineKind::Server);
    }
}
