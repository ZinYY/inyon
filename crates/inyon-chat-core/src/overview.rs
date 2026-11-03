//! `overview.ts`: every pane on every machine, the agents that need you first, and what a pane is
//! called.

use inyon_protocol::{AgentState, HostState, PaneInfo, SessionInfo, WindowInfo};

use crate::js::{encode_uri_component, is_space, trim};

/// A pane with the machine, tmux session and window it sits in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneRef<'a> {
    pub host: &'a HostState,
    pub session: &'a SessionInfo,
    pub window: &'a WindowInfo,
    pub pane: &'a PaneInfo,
}

pub fn all_panes(hosts: &[HostState]) -> Vec<PaneRef<'_>> {
    let mut out = Vec::new();
    for host in hosts {
        for session in &host.sessions {
            for window in &session.windows {
                for pane in &window.panes {
                    out.push(PaneRef { host, session, window, pane });
                }
            }
        }
    }
    out
}

fn rank(s: AgentState) -> u8 {
    match s {
        AgentState::Waiting => 0,
        AgentState::Working => 1,
        AgentState::Idle => 2,
        AgentState::Unknown => 3,
    }
}

/// The panes running an agent: the ones that need you first, then the most recently active.
pub fn agent_panes(hosts: &[HostState]) -> Vec<PaneRef<'_>> {
    let mut refs: Vec<PaneRef> = all_panes(hosts).into_iter().filter(|r| r.pane.agent.is_some()).collect();
    let state = |r: &PaneRef| r.pane.agent.as_ref().map_or(AgentState::Unknown, |a| a.state);
    refs.sort_by(|a, b| rank(state(a)).cmp(&rank(state(b))).then(b.window.activity.0.partial_cmp(&a.window.activity.0).unwrap_or(std::cmp::Ordering::Equal)));
    refs
}

/// `paneSlug` from api.ts: "%12" → "12"
pub fn pane_slug(pane: &str) -> &str {
    pane.strip_prefix('%').unwrap_or(pane)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneView {
    Terminal,
    Chat,
}

/// The app's route for a pane: /m/<machine>/p/<pane>, with ?view= when one is asked for.
pub fn pane_href(host: &str, pane: &str, view: Option<PaneView>) -> String {
    let view = match view {
        Some(PaneView::Terminal) => "?view=terminal",
        Some(PaneView::Chat) => "?view=chat",
        None => "",
    };
    format!("/m/{}/p/{}{view}", encode_uri_component(host), pane_slug(pane))
}

/// The title without a leading spinner mark ("✳ Fix login" → "Fix login"): JavaScript's
/// `/^[^\w\s]\s+/u` takes one mark that isn't a word character, and the spaces after it.
fn without_mark(title: &str) -> &str {
    let mut chars = title.chars();
    let Some(c) = chars.next() else { return title };
    if c.is_ascii_alphanumeric() || c == '_' || is_space(c) {
        return title;
    }
    let rest = chars.as_str();
    let after = rest.trim_start_matches(is_space);
    if after.len() == rest.len() {
        title
    } else {
        after
    }
}

fn is_one_of(s: &str, names: &[&str]) -> bool {
    names.iter().any(|n| s.eq_ignore_ascii_case(n))
}

/// Title the agent gives its terminal ("✳ Fix login bug"), else the window name.
pub fn pane_title(r: &PaneRef) -> String {
    if r.pane.companion == Some(true) {
        return "Companion".into();
    }
    if let Some(label) = r.pane.label.as_deref().filter(|l| !l.is_empty()) {
        return label.into();
    }
    let t = trim(without_mark(&r.pane.title));
    let agent = r.pane.agent.as_ref();
    if agent.is_some() && !t.is_empty() && !is_one_of(t, &["Claude Code", "codex", "opencode", "gemini", "agy", "Antigravity"]) && Some(t) != r.host.hostname.as_deref() {
        return t.into();
    }
    // a window tmux named after its shell (psmux on Windows names them "powershell") says nothing
    let name = &r.window.name;
    let auto = name.is_empty() || *name == r.pane.command || is_one_of(name, &["powershell", "pwsh", "cmd", "bash", "zsh", "sh", "fish", "node"]);
    if let (Some(a), true) = (agent, auto) {
        return a.custom.clone().unwrap_or_else(|| a.kind.def().name.clone());
    }
    if name.is_empty() {
        r.pane.command.clone()
    } else {
        name.clone()
    }
}

/// A terminal's tmux window name when someone named it (tmux's automatic name is the command).
pub fn terminal_name<'a>(r: &PaneRef<'a>) -> Option<&'a str> {
    let name = &r.window.name;
    (!name.is_empty() && *name != r.pane.command).then_some(name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{agent_pane, host, pane};
    use serde_json::{json, Value};

    /// One pane in its own window, for pane_title and terminal_name.
    fn machine(p: PaneInfo, window_name: &str, more: Value) -> HostState {
        let mut h = host("box", vec![p], more);
        h.sessions[0].windows[0].name = window_name.into();
        h
    }
    fn title(p: PaneInfo, window_name: &str) -> String {
        title_on(p, window_name, json!({}))
    }
    fn title_on(p: PaneInfo, window_name: &str, more: Value) -> String {
        let h = [machine(p, window_name, more)];
        pane_title(&all_panes(&h)[0])
    }

    #[test]
    fn all_panes_flattens_every_machine_session_and_window() {
        let hosts = [host("a", vec![pane("%1", json!({})), pane("%2", json!({}))], json!({})), host("b", vec![pane("%7", json!({}))], json!({}))];
        let ids: Vec<String> = all_panes(&hosts).iter().map(|r| format!("{}{}", r.host.config.id, r.pane.id)).collect();
        assert_eq!(ids, ["a%1", "a%2", "b%7"]);
        assert!(all_panes(&[]).is_empty());
    }

    #[test]
    fn agent_panes_only_agents_the_ones_that_need_you_first_then_the_most_recently_active() {
        let p = |id: &str, state: &str| agent_pane(id, "claude", json!({ "agent": { "kind": "claude", "pid": 1, "state": state } }));
        let mut h = host("a", vec![pane("%0", json!({})), p("%1", "idle"), p("%2", "working"), p("%3", "waiting"), p("%4", "unknown"), p("%5", "idle")], json!({}));
        h.sessions[0].windows[1].activity = 10.0.into();
        h.sessions[0].windows[5].activity = 20.0.into();
        let hosts = [h];
        let ids: Vec<&str> = agent_panes(&hosts).iter().map(|r| r.pane.id.as_str()).collect();
        assert_eq!(ids, ["%3", "%2", "%5", "%1", "%4"]);
    }

    #[test]
    fn pane_hrefs() {
        assert_eq!(pane_href("my box", "%12", None), "/m/my%20box/p/12");
        assert_eq!(pane_href("a/b", "%3", Some(PaneView::Terminal)), "/m/a%2Fb/p/3?view=terminal");
    }

    #[test]
    fn a_name_you_gave_it_wins() {
        assert_eq!(title(agent_pane("%1", "claude", json!({ "label": "Mine", "title": "✳ Fix login" })), ""), "Mine");
    }

    #[test]
    fn the_agents_terminal_title_without_its_spinner_mark() {
        assert_eq!(title(agent_pane("%1", "claude", json!({ "title": "✳ Fix login bug" })), ""), "Fix login bug");
        assert_eq!(title(agent_pane("%1", "claude", json!({ "title": "⠂ Refactor store" })), ""), "Refactor store");
    }

    #[test]
    fn titles_that_only_name_the_agent_or_the_machine_dont_count() {
        assert_eq!(title(agent_pane("%1", "claude", json!({ "title": "✳ Claude Code" })), "work"), "work");
        assert_eq!(title(agent_pane("%1", "codex", json!({ "title": "codex" })), "api"), "api");
        assert_eq!(title_on(agent_pane("%1", "claude", json!({ "title": "box.local" })), "api", json!({ "hostname": "box.local" })), "api");
    }

    #[test]
    fn an_agent_in_a_window_tmux_named_itself_shows_the_agents_name() {
        assert_eq!(title(agent_pane("%1", "claude", json!({})), ""), "Claude Code");
        assert_eq!(title(agent_pane("%1", "claude", json!({})), "zsh"), "Claude Code");
        assert_eq!(title(agent_pane("%1", "codex", json!({})), "powershell"), "Codex");
        assert_eq!(title(agent_pane("%1", "claude", json!({ "command": "node" })), "node"), "Claude Code");
        // your own agent shows its own name
        assert_eq!(title(agent_pane("%1", "claude", json!({ "agent": { "kind": "claude", "custom": "OpenClaude", "pid": 1, "state": "idle" } })), "sh"), "OpenClaude");
    }

    #[test]
    fn a_plain_terminal_shows_its_window_name_else_its_command() {
        assert_eq!(title(pane("%1", json!({ "command": "vim", "title": "vim notes.md" })), "notes"), "notes");
        assert_eq!(title(pane("%1", json!({ "command": "htop" })), ""), "htop");
    }

    #[test]
    fn terminal_name_is_only_a_name_someone_chose() {
        let name = |window: &str| {
            let h = [machine(pane("%1", json!({ "command": "zsh" })), window, json!({}))];
            terminal_name(&all_panes(&h)[0]).map(str::to_string)
        };
        assert_eq!(name("zsh"), None);
        assert_eq!(name(""), None);
        assert_eq!(name("logs").as_deref(), Some("logs"));
    }

    #[test]
    fn the_companion_is_called_companion() {
        assert_eq!(title(agent_pane("%1", "codex", json!({ "companion": true, "label": "x" })), ""), "Companion");
    }
}
