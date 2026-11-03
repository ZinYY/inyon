//! `groups.ts`: panes grouped the way tmux holds them, machine → tmux session → window. With dozens
//! of sessions across several machines a flat list stops working; groups fold away, and a search
//! looks through all of them at once.

use std::collections::HashMap;

use inyon_protocol::{AgentState, HostState, SessionInfo};

use crate::js::{is_space, to_number};
use crate::overview::{all_panes, pane_title, terminal_name, PaneRef};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionGroup<'a> {
    pub key: String,
    pub session: &'a SessionInfo,
    pub panes: Vec<PaneRef<'a>>,
    pub waiting: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostGroup<'a> {
    pub key: String,
    pub host: &'a HostState,
    pub sessions: Vec<SessionGroup<'a>>,
    pub panes: usize,
    pub waiting: usize,
}

/// Every word of the query appears somewhere in the text (case-insensitive).
pub fn matches_query(text: &str, query: &str) -> bool {
    let t = text.to_lowercase();
    query.to_lowercase().split(is_space).filter(|w| !w.is_empty()).all(|w| t.contains(w))
}

/// Everything a search can find a pane by.
pub fn search_text(r: &PaneRef) -> String {
    let agent = r.pane.agent.as_ref();
    let target = format!("{}:{}", r.session.name, r.window.index);
    let fields: [Option<String>; 9] = [
        Some(if agent.is_some() { pane_title(r) } else { terminal_name(r).unwrap_or("").to_string() }),
        Some(agent.map_or_else(|| "terminal".to_string(), |a| a.kind.def().name.clone())),
        Some(r.pane.command.clone()),
        Some(r.host.config.name.clone()),
        r.host.hostname.clone(),
        Some(r.session.name.clone()),
        Some(target),
        Some(r.window.name.clone()),
        Some(r.pane.cwd.clone()),
    ];
    fields.into_iter().flatten().filter(|f| !f.is_empty()).collect::<Vec<_>>().join(" ")
}

/// tmux numbers sessions as it makes them ("$4"), so the number is the order they were started in.
pub fn session_order(s: &SessionInfo) -> f64 {
    let n = to_number(s.id.strip_prefix('$').unwrap_or(&s.id));
    if n.is_finite() {
        n
    } else {
        9_007_199_254_740_991.0
    }
}

/// tmux's own address for a pane: session:window, plus .pane when the window is split.
pub fn tmux_target(r: &PaneRef) -> String {
    if r.window.panes.len() > 1 {
        format!("{}:{}.{}", r.session.name, r.window.index, r.pane.index)
    } else {
        format!("{}:{}", r.session.name, r.window.index)
    }
}

/// Which panes `group_panes` keeps.
#[derive(Default)]
pub struct GroupFilter<'f> {
    pub query: Option<&'f str>,
    pub keep: Option<&'f dyn Fn(&PaneRef) -> bool>,
}

/// Groups panes by machine and tmux session, keeping only those that pass `keep` and match `query`.
/// Everything keeps its place: machines in the order they were added, sessions in the order tmux
/// started them, panes in tmux's window and pane order. Nothing moves when an agent starts working
/// or asks something, so a row is still where you aimed when the click lands.
pub fn group_panes<'a>(hosts: &'a [HostState], o: &GroupFilter) -> Vec<HostGroup<'a>> {
    let refs = all_panes(hosts).into_iter().filter(|r| o.keep.is_none_or(|k| k(r)) && o.query.is_none_or(|q| q.is_empty() || matches_query(&search_text(r), q)));
    let mut groups: Vec<HostGroup> = Vec::new();
    // where each machine and session went in `groups`
    let mut by_host: HashMap<&str, usize> = HashMap::new();
    let mut by_session: HashMap<String, (usize, usize)> = HashMap::new();
    for r in refs {
        let hk = r.host.config.id.as_str();
        let h = *by_host.entry(hk).or_insert_with(|| {
            groups.push(HostGroup { key: hk.to_string(), host: r.host, sessions: Vec::new(), panes: 0, waiting: 0 });
            groups.len() - 1
        });
        let sk = format!("{hk}\u{0}{}", r.session.id);
        let (h, s) = *by_session.entry(sk.clone()).or_insert_with(|| {
            groups[h].sessions.push(SessionGroup { key: sk, session: r.session, panes: Vec::new(), waiting: 0 });
            (h, groups[h].sessions.len() - 1)
        });
        let waiting = usize::from(r.pane.agent.as_ref().is_some_and(|a| a.state == AgentState::Waiting));
        let hg = &mut groups[h];
        hg.sessions[s].panes.push(r);
        hg.sessions[s].waiting += waiting;
        hg.panes += 1;
        hg.waiting += waiting;
    }
    let order: HashMap<&str, usize> = hosts.iter().enumerate().map(|(i, h)| (h.config.id.as_str(), i)).collect();
    for g in &mut groups {
        g.sessions.sort_by(|a, b| session_order(a.session).partial_cmp(&session_order(b.session)).unwrap_or(std::cmp::Ordering::Equal));
    }
    groups.sort_by_key(|g| order.get(g.key.as_str()).copied().unwrap_or(usize::MAX));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{agent_pane, host, pane};
    use serde_json::{json, Value};

    /// A machine with two tmux sessions: "api" (two windows) and "web" (one window split in two).
    fn machine(id: &str, more: Value) -> HostState {
        let mut h = host(id, vec![], more);
        h.sessions = serde_json::from_value(json!([
            {
                "id": "$1", "name": "api", "attached": 0,
                "windows": [
                    { "id": "@1", "index": 1, "name": "", "active": true, "activity": 100, "panes": [agent_pane("%1", "claude", json!({ "title": "✳ Fix the login test", "cwd": "/srv/api" }))] },
                    { "id": "@2", "index": 2, "name": "logs", "active": false, "activity": 50, "panes": [pane("%2", json!({ "command": "tail" }))] },
                ],
            },
            {
                "id": "$2", "name": "web", "attached": 0,
                "windows": [{ "id": "@3", "index": 1, "name": "", "active": true, "activity": 300, "panes": [agent_pane("%3", "codex", json!({ "title": "Tidy the CSS", "index": 0 })), pane("%4", json!({ "index": 1 }))] }],
            },
        ]))
        .unwrap();
        h
    }
    fn all() -> GroupFilter<'static> {
        GroupFilter::default()
    }
    fn names(groups: &[HostGroup]) -> Vec<String> {
        groups[0].sessions.iter().map(|s| s.session.name.clone()).collect()
    }

    #[test]
    fn machine_then_tmux_session_then_panes_in_window_order() {
        let hosts = [machine("box", json!({}))];
        let groups = group_panes(&hosts, &all());
        let g = &groups[0];
        assert_eq!(g.host.config.id, "box");
        // in the order tmux started them, though "web" was active more recently
        assert_eq!(names(&groups), ["api", "web"]);
        assert_eq!(g.sessions[0].panes.iter().map(|r| r.pane.id.as_str()).collect::<Vec<_>>(), ["%1", "%2"]);
        assert_eq!(g.panes, 4);
    }

    // rows moved up and down as agents worked and asked, so clicks landed on the wrong session
    #[test]
    fn nothing_moves_when_an_agent_asks_something_works_or_goes_quiet() {
        let mut h = machine("box", json!({}));
        assert_eq!(names(&group_panes(std::slice::from_ref(&h), &all())), ["api", "web"]);
        h.sessions[1].windows[0].panes[0].agent.as_mut().unwrap().state = AgentState::Waiting;
        h.sessions[1].windows[0].activity = 900.0.into();
        let hosts = [h.clone()];
        let groups = group_panes(&hosts, &all());
        assert_eq!(names(&groups), ["api", "web"]);
        assert_eq!((groups[0].waiting, groups[0].sessions[1].waiting), (1, 1));
        h.sessions[0].windows[0].panes[0].agent.as_mut().unwrap().state = AgentState::Working;
        h.sessions[0].windows[0].activity = 1000.0.into();
        assert_eq!(names(&group_panes(std::slice::from_ref(&h), &all())), ["api", "web"]);
    }

    #[test]
    fn sessions_follow_tmux_numbering_and_machines_keep_the_order_they_were_added_in() {
        let mut h = machine("box", json!({}));
        h.sessions.reverse();
        h.sessions[0].id = "$10".into();
        h.sessions[1].id = "$9".into();
        let hosts = [h];
        let ids: Vec<&str> = group_panes(&hosts, &all())[0].sessions.iter().map(|s| s.session.id.as_str()).collect();
        assert_eq!(ids, ["$9", "$10"]);
        let hosts = [machine("old", json!({ "status": "error" })), machine("new", json!({}))];
        assert_eq!(group_panes(&hosts, &all()).iter().map(|g| g.host.config.id.as_str()).collect::<Vec<_>>(), ["old", "new"]);
    }

    #[test]
    fn a_search_looks_through_every_machine_and_session() {
        let hosts = [machine("box", json!({})), machine("laptop", json!({}))];
        let ids = |q: &str| -> Vec<String> {
            group_panes(&hosts, &GroupFilter { query: Some(q), keep: None })
                .iter()
                .flat_map(|g| g.sessions.iter().flat_map(|s| s.panes.iter().map(|r| format!("{}{}", g.key, r.pane.id))))
                .collect()
        };
        assert_eq!(ids("login"), ["box%1", "laptop%1"]);
        assert_eq!(ids("laptop css"), ["laptop%3"]);
        assert_eq!(ids("api:2"), ["box%2", "laptop%2"]);
        assert_eq!(ids("/srv/api claude"), ["box%1", "laptop%1"]);
        assert!(ids("nothing like this").is_empty());
    }

    #[test]
    fn keep_picks_which_panes_count_and_empty_groups_disappear() {
        let hosts = [machine("box", json!({}))];
        let codex = |r: &PaneRef| r.pane.agent.as_ref().is_some_and(|a| a.kind.as_str() == "codex");
        let groups = group_panes(&hosts, &GroupFilter { query: None, keep: Some(&codex) });
        assert_eq!(names(&groups), ["web"]);
    }

    #[test]
    fn tmux_target_has_the_pane_only_in_a_split_window() {
        let hosts = [machine("box", json!({}))];
        assert_eq!(all_panes(&hosts).iter().map(tmux_target).collect::<Vec<_>>(), ["api:1", "api:2", "web:1.0", "web:1.1"]);
    }

    #[test]
    fn every_word_must_match_in_any_order_and_case() {
        assert!(matches_query("Fix the login test · box", "LOGIN box"));
        assert!(!matches_query("Fix the login test", "login laptop"));
        assert!(matches_query("anything", "   "));
    }
}
