// What an app may see of the agents. Apps before 0.23 look every agent kind up in their own
// catalog and crash on one they don't have, so for them a pane running a newer agent is a plain
// terminal, and the newer kinds drop out of their lists. An app says which catalog it knows with
// the `x-inyon-catalog` header or, on its WebSocket, `?catalog=` (2 from 0.23); none means 1.

use std::borrow::Cow;

use crate::agents::AgentKind;
use crate::types::{ChatInfo, HostState, ServerMsg};

/// The catalog this build's apps know: they fall back on their own for any kind they don't have.
pub const CATALOG: u32 = 2;

/// The catalog an app named, from the header or query value it sent (1 when it named none).
pub fn catalog_of(value: Option<&str>) -> u32 {
    value.and_then(|v| v.trim().parse().ok()).filter(|n| *n >= 1).unwrap_or(1)
}

fn hides(kind: AgentKind, catalog: u32) -> bool {
    !kind.known_to(catalog)
}

/// A machine as an app that knows `catalog` may see it.
pub fn host_for(h: &mut HostState, catalog: u32) {
    h.agents.retain(|k| !hides(*k, catalog));
    if let Some(s) = h.signed_in.as_mut() {
        s.retain(|k| !hides(*k, catalog));
    }
    if let Some(l) = h.limits.as_mut() {
        l.retain(|a| !hides(a.kind, catalog));
    }
    for p in h.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).flat_map(|w| w.panes.iter_mut()) {
        if p.agent.as_ref().is_some_and(|a| hides(a.kind, catalog)) {
            p.agent = None;
        }
    }
}

/// The machines as an app that knows `catalog` may see them; untouched when it knows them all.
pub fn hosts_for(hosts: &[HostState], catalog: u32) -> Cow<'_, [HostState]> {
    if catalog >= CATALOG {
        return Cow::Borrowed(hosts);
    }
    let mut out = hosts.to_vec();
    for h in &mut out {
        host_for(h, catalog);
    }
    Cow::Owned(out)
}

fn chat_for(info: &ChatInfo, catalog: u32) -> Option<Box<ChatInfo>> {
    info.agent.filter(|k| hides(*k, catalog)).map(|_| Box::new(ChatInfo { agent: None, ..info.clone() }))
}

/// A message to an app that knows `catalog`; borrowed when nothing in it needs hiding.
pub fn msg_for(msg: &ServerMsg, catalog: u32) -> Cow<'_, ServerMsg> {
    if catalog >= CATALOG {
        return Cow::Borrowed(msg);
    }
    match msg {
        ServerMsg::Overview { hosts, organize } => match hosts_for(hosts, catalog) {
            Cow::Owned(hosts) => Cow::Owned(ServerMsg::Overview { hosts, organize: organize.clone() }),
            Cow::Borrowed(_) => Cow::Borrowed(msg),
        },
        ServerMsg::ChatReset { key, info, items, has_more } => match chat_for(info, catalog) {
            Some(info) => Cow::Owned(ServerMsg::ChatReset { key: key.clone(), info, items: items.clone(), has_more: *has_more }),
            None => Cow::Borrowed(msg),
        },
        ServerMsg::ChatInfo { key, info } => match chat_for(info, catalog) {
            Some(info) => Cow::Owned(ServerMsg::ChatInfo { key: key.clone(), info }),
            None => Cow::Borrowed(msg),
        },
        ServerMsg::Notify { host, pane, kind, title, body, agent: Some(a) } if hides(*a, catalog) => {
            Cow::Owned(ServerMsg::Notify { host: host.clone(), pane: pane.clone(), kind: *kind, title: title.clone(), body: body.clone(), agent: None })
        }
        _ => Cow::Borrowed(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentPaneInfo, NotifyKind, PaneInfo};
    use serde_json::{json, Value};

    fn pane(id: &str, kind: &str) -> Value {
        json!({ "id": id, "index": 0, "active": true, "cwd": "/w", "title": "", "command": kind, "width": 80, "height": 24, "dead": false,
            "agent": { "kind": kind, "pid": 1, "state": "idle" } })
    }

    fn host() -> HostState {
        serde_json::from_value(json!({
            "config": { "id": "h", "name": "box", "kind": "local" },
            "status": "online", "integration": "installed", "tmux": "ok",
            "agents": ["claude", "pi", "codex"], "signedIn": ["pi", "claude"],
            "limits": [{ "kind": "claude", "updatedAt": 1 }, { "kind": "grok", "updatedAt": 1 }],
            "sessions": [{ "id": "$1", "name": "s", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": [pane("%1", "claude"), pane("%2", "pi")] }] }],
            "updatedAt": 1
        }))
        .expect("a host")
    }

    fn kinds(h: &HostState) -> Vec<Option<&str>> {
        h.sessions[0].windows[0].panes.iter().map(|p: &PaneInfo| p.agent.as_ref().map(|a: &AgentPaneInfo| a.kind.as_str())).collect()
    }

    #[test]
    fn an_app_before_the_new_agents_sees_their_panes_as_terminals_and_not_their_kinds() {
        let hosts = vec![host()];
        let old = hosts_for(&hosts, catalog_of(None));
        let h = &old[0];
        assert_eq!(h.agents, [AgentKind::Claude, AgentKind::Codex]);
        assert_eq!(h.signed_in.as_deref(), Some(&[AgentKind::Claude][..]));
        assert_eq!(h.limits.as_ref().map(|l| l.iter().map(|a| a.kind).collect::<Vec<_>>()), Some(vec![AgentKind::Claude]));
        assert_eq!(kinds(h), [Some("claude"), None]);
        // the pane itself stays, with its terminal
        assert_eq!(h.sessions[0].windows[0].panes.len(), 2);
        // an app that knows them gets everything as it is, without a copy
        assert!(matches!(hosts_for(&hosts, catalog_of(Some("2"))), Cow::Borrowed(_)));
        assert_eq!(kinds(&hosts_for(&hosts, 2)[0]), [Some("claude"), Some("pi")]);
    }

    #[test]
    fn messages_about_a_newer_agent_reach_an_older_app_without_its_kind() {
        let n = ServerMsg::Notify { host: "h".into(), pane: "%2".into(), kind: NotifyKind::Done, title: "Pi".into(), body: "done".into(), agent: Some(AgentKind::Pi) };
        assert!(matches!(&*msg_for(&n, 1), ServerMsg::Notify { agent: None, title, .. } if title == "Pi"));
        assert!(matches!(&*msg_for(&n, 2), ServerMsg::Notify { agent: Some(AgentKind::Pi), .. }));
        let c = ServerMsg::Notify { host: "h".into(), pane: "%1".into(), kind: NotifyKind::Done, title: "x".into(), body: "y".into(), agent: Some(AgentKind::Claude) };
        assert!(matches!(msg_for(&c, 1), Cow::Borrowed(_)));
        let o = ServerMsg::Overview { hosts: vec![host()], organize: None };
        let ServerMsg::Overview { hosts, .. } = &*msg_for(&o, 1) else { panic!("an overview") };
        assert_eq!(kinds(&hosts[0]), [Some("claude"), None]);
    }

    #[test]
    fn a_catalog_is_read_from_what_the_app_sent() {
        assert_eq!((catalog_of(None), catalog_of(Some("2")), catalog_of(Some(" 3 ")), catalog_of(Some("x")), catalog_of(Some("0"))), (1, 2, 3, 1, 1));
    }
}
