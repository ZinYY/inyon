use serde_json::json;

use super::*;

fn pane(id: &str, index: u32, agent: Option<(&str, &str)>, cwd: &str) -> Value {
    json!({ "id": id, "index": index, "active": true, "title": "", "dead": false, "command": "x", "cwd": cwd, "agent": agent.map(|(k, st)| json!({ "kind": k, "state": st })) })
}

fn session(name: &str, windows: Vec<(u32, Vec<Value>)>) -> Value {
    json!({ "id": format!("${name}"), "name": name, "windows": windows.into_iter().map(|(i, panes)| json!({ "id": format!("@{i}"), "index": i, "name": "w", "active": true, "panes": panes })).collect::<Vec<_>>() })
}

pub fn hosts() -> Vec<Value> {
    vec![
        json!({ "config": { "id": "local", "name": "abhash-arm", "kind": "local" }, "status": "online", "sessions": [
            session("main", vec![(0, vec![pane("%1", 0, Some(("claude", "working")), "/home/me/inyon")])]),
            session("notes", vec![(0, vec![pane("%2", 0, None, "/home/me")])]),
        ] }),
        json!({ "config": { "id": "laptop", "name": "Laptop", "kind": "node" }, "status": "online", "sessions": [
            session("experiment", vec![(0, vec![pane("%3", 0, Some(("codex", "idle")), "C:\\demo\\robot")])]),
            session("work", vec![(1, vec![pane("%5", 0, Some(("claude", "waiting")), "C:\\work"), pane("%6", 1, Some(("gemini", "idle")), "C:\\work")])]),
        ] }),
    ]
}

#[test]
fn machines_and_sessions_read_as_aligned_lines() {
    assert_eq!(machine_lines(&hosts()), ["abhash-arm  online  2 sessions, 1 agent, 1 working", "Laptop      online  2 sessions, 3 agents, 1 waiting for you"]);
    assert_eq!(
        session_lines(&hosts()),
        [
            "abhash-arm/main    claude  working  /home/me/inyon",
            "abhash-arm/notes   none",
            "Laptop/experiment  codex   idle     C:\\demo\\robot",
            "Laptop/work:1.0    claude  waiting  C:\\work",
            "Laptop/work:1.1    gemini  idle     C:\\work",
        ]
    );
    assert_eq!(machine_lines(&[]), ["The hub has no machines yet."]);
}

#[test]
fn a_target_names_one_agent_or_says_which_to_pick() {
    let h = hosts();
    let t = resolve(&h, "Laptop/experiment").unwrap();
    assert_eq!(t, Target { host: "laptop".into(), pane: "%3".into(), label: "Laptop/experiment".into(), agent: "codex".into() });
    // the machine by id or in any case, when that's still one machine
    assert_eq!(resolve(&h, "laptop/experiment").unwrap().pane, "%3");
    assert_eq!(resolve(&h, "LAPTOP/experiment").unwrap().pane, "%3");
    assert_eq!(resolve(&h, "Laptop/work:1.1").unwrap(), Target { host: "laptop".into(), pane: "%6".into(), label: "Laptop/work:1.1".into(), agent: "gemini".into() });
    assert_eq!(resolve(&h, "Laptop/work").unwrap_err(), "Laptop/work runs 2 agents. Name one: Laptop/work:1.0 (claude), Laptop/work:1.1 (gemini)");
    assert_eq!(resolve(&h, "abhash-arm/notes").unwrap_err(), "No agent is running in abhash-arm/notes.");
    assert_eq!(resolve(&h, "abhash-arm/nope").unwrap_err(), "abhash-arm has no session called nope. `inyon sessions` lists them.");
    assert_eq!(resolve(&h, "pi/main").unwrap_err(), "The hub has no machine called pi. `inyon hub machines` lists them.");
    assert!(resolve(&h, "main").unwrap_err().starts_with("Name the session as machine/session"));
    // a colon that isn't window.pane belongs to the session's name
    assert_eq!(resolve(&h, "Laptop/work:x").unwrap_err(), "Laptop has no session called work:x. `inyon sessions` lists them.");
}

#[test]
fn the_pane_goes_in_the_path_as_the_app_sends_it() {
    assert_eq!(pane_path("laptop", "%3"), "/api/hosts/laptop/panes/3");
    assert_eq!(pane_path("my box", "%12"), "/api/hosts/my%20box/panes/12");
}

#[test]
fn a_refusal_says_what_to_do() {
    assert!(refusal(401, &json!({ "error": "Not signed in", "reason": "revoked" }), false).contains("needs hub 0.22 or later"));
    assert!(refusal(401, &Value::Null, true).contains("inyon hub restart"));
    assert_eq!(refusal(403, &json!({ "error": "Turn it on", "reason": "send-off" }), false), "Turn it on");
    assert!(refusal(403, &json!({ "error": "x", "reason": "scope" }), false).contains("Update it to 0.22"));
    assert_eq!(refusal(404, &json!({ "error": "Pane not found" }), false), "Pane not found");
    assert_eq!(refusal(502, &Value::Null, false), "The hub answered 502.");
}

#[test]
fn a_conversation_reads_as_who_said_what() {
    assert_eq!(read_lines(&[]), ["Nothing said in that session yet."]);
    let items = [json!({ "role": "you", "text": "port the parser" }), json!({ "role": "agent", "text": "Done.\nTests pass." })];
    assert_eq!(read_lines(&items), ["you: port the parser", "agent: Done.\nTests pass."]);
}

#[test]
fn the_companion_says_whether_it_runs() {
    assert!(companion_line(&json!({ "agent": null })).starts_with("The companion is off."));
    assert_eq!(companion_line(&json!({ "agent": "codex", "pane": "%4" })), "The companion runs Codex in the tmux session companion on the hub's machine.");
    assert!(companion_line(&json!({ "agent": "claude", "pane": null })).contains("inyon companion start --agent claude"));
}
