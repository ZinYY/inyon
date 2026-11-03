//! What the hub keeps about sessions: the record, its version, the limits, and finding a session
//! again when tmux hands its pane id to another pane.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};

use super::*;
use crate::clock::set_now;
use crate::hosts::host::Host;
use crate::hosts::registry::HostRuntime;
use crate::test_support::rt::run;
use inyon_protocol::project_of;

const T: u64 = 1_790_000_000_000;

fn fresh() {
    reset();
    kv::del(KEY);
    kv::del(UNREADABLE_KEY);
    set_now(Some(T));
}

/// A pane: its id, folder and agent's conversation.
type Pane<'a> = (&'a str, &'a str, Option<&'a str>);

/// A machine "box" whose tmux sessions are `sessions`: `(id, name, panes)`.
fn machine(sessions: &[(&str, &str, &[Pane])]) -> HostState {
    let sessions: Vec<Value> = sessions
        .iter()
        .map(|(id, name, panes)| {
            let panes: Vec<Value> = panes
                .iter()
                .enumerate()
                .map(|(i, (pane, cwd, conv))| {
                    let mut p = json!({ "id": pane, "index": i, "active": i == 0, "command": "claude", "cwd": cwd, "title": format!("✳ Task {pane}"), "width": 100, "height": 30, "dead": false });
                    if let Some(c) = conv {
                        p["agent"] = json!({ "kind": "claude", "pid": 1, "state": "idle", "sessionId": c });
                    }
                    p
                })
                .collect();
            json!({ "id": id, "name": name, "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": panes }] })
        })
        .collect();
    serde_json::from_value(json!({ "config": { "id": "box", "name": "Box", "kind": "ssh" }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0, "sessions": sessions }))
        .unwrap()
}

/// Puts the machine in the registry as it is (nothing polls it).
fn put(state: HostState) {
    let id = state.config.id.clone();
    let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state)));
    rt.borrow_mut().hold_polls();
    registry().insert_runtime(&id, rt);
}

/// The same machine under another id, with no tmux sessions.
fn other(id: &str) -> HostState {
    let mut h = machine(&[]);
    h.config.id = id.to_owned();
    h
}

/// The machine as it was last seen, now not connected.
fn offline(mut h: HostState) -> HostState {
    h.status = HostStatus::Error;
    h
}

/// A store whose writes of one record fail while `failing` is set, as on a full disk.
struct Flaky {
    inner: crate::platform::store::MemoryStore,
    key: &'static str,
    failing: std::cell::Cell<bool>,
}

impl crate::platform::store::Store for Flaky {
    fn get(&self, key: &str) -> Option<Value> {
        self.inner.get(key)
    }
    fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        if key == self.key && self.failing.get() {
            return Err("No space left on device".into());
        }
        self.inner.set(key, value)
    }
    fn del(&self, key: &str) {
        self.inner.del(key)
    }
    fn keys(&self, prefix: &str) -> Vec<String> {
        self.inner.keys(prefix)
    }
}

fn patch(v: Value) -> SessionPatch {
    SessionPatch::read(&v, now_ms()).unwrap()
}

fn mark(host: &str, pane: &str) -> Option<SessionMark> {
    get().sessions.into_iter().find(|m| m.live && m.host == host && m.pane == pane)
}

#[test]
fn a_change_is_written_with_its_version_and_read_back_after_a_restart() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w/inyon", Some("conv-a"))])]));
        let m = mark_pane("box", "%3", &patch(json!({ "pinned": true, "tags": ["#Review", "review", "UI"], "note": "  check the menu  " }))).unwrap().unwrap();
        assert_eq!((m.pinned, m.tags.as_deref(), m.note.as_deref()), (Some(0), Some(&["review".to_owned(), "ui".to_owned()][..]), Some("check the menu")));
        assert_eq!((m.conv.as_deref(), m.tmux_id.as_deref(), m.tmux_name.as_deref(), m.cwd.as_deref()), (Some("conv-a"), Some("$1"), Some("work"), Some("/w/inyon")));
        let saved = kv::get_value(KEY).unwrap();
        assert_eq!(saved["v"], json!(ORGANIZE_VERSION));
        assert_eq!(saved["sessions"][0]["id"], json!(m.id));

        // the hub restarts: nothing is live until the machines are matched again
        reset();
        assert!(get().sessions.iter().all(|m| !m.live));
        assert!(reconcile(&registry().overview()));
        assert_eq!(mark("box", "%3").unwrap().id, m.id);
        assert!(!reconcile(&registry().overview()), "nothing new the second time");
    });
}

#[test]
fn clearing_everything_on_a_session_forgets_it() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None)])]));
        mark_pane("box", "%3", &patch(json!({ "parked": true }))).unwrap();
        assert_eq!(get().sessions.len(), 1);
        assert_eq!(mark_pane("box", "%3", &patch(json!({ "parked": false }))).unwrap(), None);
        assert!(get().sessions.is_empty());
        assert_eq!(mark_pane("box", "%9", &patch(json!({ "parked": true }))).unwrap_err().status, Some(404));
    });
}

#[test]
fn a_record_from_before_versions_is_read_and_one_from_a_newer_hub_is_never_written_over() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None)])]));
        kv::set_value(KEY, &json!({ "sessions": [{ "id": "s-1", "host": "box", "pane": "%3", "tmuxId": "$1", "parked": true }] })).unwrap();
        let org = current();
        assert_eq!((org.v, org.sessions.len(), org.sessions[0].live), (ORGANIZE_VERSION, 1, true));

        // version 1 (pins, parking, snoozes, tags, notes, projects) reads as it is
        fresh();
        kv::set_value(
            KEY,
            &json!({ "v": 1, "sessions": [{ "id": "s-1", "host": "box", "pane": "%3", "tmuxId": "$1", "pinned": 0 }], "projects": [{ "id": "p-1", "name": "Inyon" }] }),
        )
        .unwrap();
        let org = current();
        assert_eq!((org.v, org.sessions[0].pinned, org.projects.len(), org.sends.clone()), (2, Some(0), 1, None));

        fresh();
        let later = json!({ "v": ORGANIZE_VERSION + 1, "sessions": [{ "id": "s-1", "host": "box", "pane": "%3", "parked": true, "colourOfTheDay": "red" }], "boards": [1] });
        kv::set_value(KEY, &later).unwrap();
        assert_eq!(current().sessions.len(), 1, "what it knows is read");
        let e = mark_pane("box", "%3", &patch(json!({ "pinned": true }))).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(409), NEWER));
        assert_eq!(add_project(&json!({ "name": "x" })).unwrap_err().status, Some(409));
        assert_eq!(kv::get_value(KEY).unwrap(), later, "untouched");
        assert_eq!(get().sessions[0].pinned, None);

        fresh();
        kv::set_value(KEY, &json!({ "v": 1, "sessions": "not a list" })).unwrap();
        assert!(get().sessions.is_empty());
        assert_eq!(kv::get_value(UNREADABLE_KEY).unwrap()["sessions"], json!("not a list"), "put aside, not lost");
    });
}

#[test]
fn what_an_app_sends_is_checked_against_the_limits() {
    run(|| async {
        fresh();
        let bad = |v: Value| SessionPatch::read(&v, T).unwrap_err().message;
        assert_eq!(bad(json!({ "tags": ["a", "b", "c", "d", "e", "f", "g", "h", "i"] })), "A session can have at most 8 tags");
        assert_eq!(bad(json!({ "tags": ["x".repeat(33)] })), "A tag can be at most 32 characters, on one line");
        assert_eq!(bad(json!({ "tags": "ui" })), "tags must be a list of words");
        assert_eq!(bad(json!({ "tags": ["first\nsecond"] })), "A tag can be at most 32 characters, on one line");
        assert_eq!(bad(json!({ "note": "n".repeat(501) })), "A note can be at most 500 characters");
        assert_eq!(bad(json!({ "title": "t".repeat(81) })), "A title can be at most 80 characters");
        assert_eq!(bad(json!({ "snoozedUntil": T - 1 })), "Pick a time in the future to snooze until");
        assert_eq!(bad(json!({ "snoozedUntil": T + MAX_SNOOZE_MS + 1 })), "A snooze can last a year at most");
        assert_eq!(bad(json!({ "pinned": "yes" })), "pinned must be true or false");
        // nine tags of which two are the same once normalised, and an empty one: eight
        let p = SessionPatch::read(&json!({ "tags": ["a", "b", "c", "d", "e", "f", "g", "#G", " "], "note": "n".repeat(500) }), T).unwrap();
        assert_eq!(p.tags.unwrap().len(), 7);

        assert_eq!(add_project(&json!({ "name": " " })).unwrap_err().message, "Give the project a name");
        assert_eq!(add_project(&json!({ "name": "x", "color": "gold" })).unwrap_err().message, "Unknown colour");
        assert_eq!(
            add_project(&json!({ "name": "x", "roots": [{ "host": "box", "path": "relative/dir" }] })).unwrap_err().message,
            "A project's folder is a full path on one machine"
        );
        assert_eq!(add_project(&json!({ "name": "x", "group": "g-nope" })).unwrap_err().status, Some(404));
        // a folder on a machine the hub doesn't have would never match anything
        let e = add_project(&json!({ "name": "x", "roots": [{ "host": "laptpo", "path": "/w" }] })).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(404), "Unknown machine: laptpo"));
        for i in 0..MAX_PROJECTS {
            add_project(&json!({ "name": format!("p{i}") })).unwrap();
        }
        let e = add_project(&json!({ "name": "one more" })).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(409), "Inyon keeps 100 projects at most"));
    });
}

#[test]
fn a_reused_pane_id_in_another_tmux_session_is_not_the_old_session() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w/inyon", None)])]));
        let id = mark_pane("box", "%3", &patch(json!({ "pinned": true }))).unwrap().unwrap().id;

        // tmux's server restarted: %3 is now a pane in a new session
        assert!(reconcile(&[machine(&[("$0", "scratch", &[("%3", "/tmp", None)])])]));
        assert!(mark("box", "%3").is_none(), "not taken for the old pane");
        assert!(get().sessions.iter().any(|m| m.id == id && !m.live));

        // the old session is put back under its name (tmux-resurrect), with new ids: matched by place
        assert!(reconcile(&[machine(&[("$0", "scratch", &[("%3", "/tmp", None)]), ("$1", "work", &[("%5", "/w/inyon", None)])])]));
        assert_eq!(mark("box", "%5").unwrap().id, id);
        assert!(mark("box", "%3").is_none());

        // renamed in tmux, same session: still the same
        assert!(!reconcile(&[machine(&[("$1", "work-renamed", &[("%5", "/w/inyon", None)])])]));
        assert_eq!(mark("box", "%5").unwrap().tmux_name.as_deref(), Some("work-renamed"));
        // a pane put in the same place in another folder isn't it
        assert!(reconcile(&[machine(&[("$7", "work-renamed", &[("%9", "/elsewhere", None)])])]));
        assert!(get().sessions.iter().all(|m| !m.live));
    });
}

#[test]
fn a_conversation_resumed_in_another_pane_takes_its_mark_along() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", Some("conv-a")), ("%4", "/w", None)])]));
        let id = mark_pane("box", "%3", &patch(json!({ "note": "the login bug" }))).unwrap().unwrap().id;
        // the agent left %3 and the conversation was resumed in %8, in another session
        assert!(reconcile(&[machine(&[("$1", "work", &[("%3", "/w", None)]), ("$2", "other", &[("%8", "/w", Some("conv-a"))])])]));
        let m = mark("box", "%8").unwrap();
        assert_eq!((m.id.as_str(), m.tmux_id.as_deref()), (id.as_str(), Some("$2")));
        assert!(mark("box", "%3").is_none());
        // a /clear in the same pane gives it a new conversation: same session
        assert!(!reconcile(&[machine(&[("$2", "other", &[("%8", "/w", Some("conv-b"))])])]));
        assert_eq!(mark("box", "%8").unwrap().conv.as_deref(), Some("conv-b"));
    });
}

#[test]
fn a_mark_whose_pane_is_gone_for_a_month_is_forgotten() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None)])]));
        mark_pane("box", "%3", &patch(json!({ "parked": true }))).unwrap();
        reconcile(&[machine(&[])]);
        set_now(Some(T + FORGET_MS - 1));
        reconcile(&[machine(&[])]);
        assert_eq!(get().sessions.len(), 1);
        set_now(Some(T + FORGET_MS));
        assert!(reconcile(&[machine(&[])]));
        assert!(get().sessions.is_empty());
        assert!(kv::get_value(KEY).unwrap()["sessions"].as_array().unwrap().is_empty());
    });
}

#[test]
fn pins_keep_an_order_every_device_shares() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%1", "/w", None), ("%2", "/w", None), ("%3", "/w", None)])]));
        let ids: Vec<String> = ["%1", "%2", "%3"].iter().map(|p| mark_pane("box", p, &patch(json!({ "pinned": true }))).unwrap().unwrap().id).collect();
        let order = || {
            let mut v: Vec<(u32, String)> = get().sessions.iter().filter_map(|m| m.pinned.map(|n| (n, m.pane.clone()))).collect();
            v.sort();
            v.into_iter().map(|(_, p)| p).collect::<Vec<_>>()
        };
        assert_eq!(order(), ["%1", "%2", "%3"]);
        mark_pane("box", "%2", &patch(json!({ "pinned": false }))).unwrap();
        assert_eq!(get().sessions.iter().filter_map(|m| m.pinned).collect::<Vec<_>>(), [0, 1]);
        set_order(&json!({ "pins": [ids[2]] })).unwrap();
        assert_eq!(order(), ["%3", "%1"]);
        // pinned again: last
        mark_pane("box", "%2", &patch(json!({ "pinned": true }))).unwrap();
        assert_eq!(order(), ["%3", "%1", "%2"]);
    });
}

#[test]
fn a_snooze_ends_on_time_and_raises_the_notice_it_asked_for() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None), ("%4", "/w", None)])]));
        let heard = Rc::new(RefCell::new(vec![]));
        let h = heard.clone();
        on_event(move |e| h.borrow_mut().push(e.clone()));
        mark_pane("box", "%3", &patch(json!({ "snoozedUntil": T + 3_600_000, "snoozeNotice": true, "note": "ship it\nthen tidy" }))).unwrap();
        mark_pane("box", "%4", &patch(json!({ "snoozedUntil": T + 3_600_000, "tags": ["later"] }))).unwrap();
        heard.borrow_mut().clear();
        assert!(!wake_due(T + 3_599_999));
        assert!(heard.borrow().is_empty());
        assert!(wake_due(T + 3_600_000));
        assert_eq!(
            *heard.borrow(),
            [Event::Changed, Event::Notice { host: "box".into(), pane: "%3".into(), title: "Task %3".into(), body: "ship it".into() }],
            "one notice, for the session that asked"
        );
        let org = get();
        assert!(org.sessions.iter().all(|m| m.snoozed_until.is_none()));
        assert_eq!(mark("box", "%4").unwrap().tags.as_deref(), Some(&["later".to_owned()][..]), "the rest of the mark stays");
        assert!(!wake_due(T + 7_200_000), "once");
    });
}

#[test]
fn a_session_falls_into_the_project_whose_folder_holds_it_most_closely() {
    run(|| async {
        fresh();
        put(machine(&[]));
        put(other("laptop"));
        let wide = add_project(&json!({ "name": "Work", "roots": [{ "host": "box", "path": "/w" }] })).unwrap();
        let inyon = add_project(
            &json!({ "name": "Inyon", "color": "sage", "icon": "🪁", "roots": [{ "host": "box", "path": "/w/inyon" }, { "host": "laptop", "path": "C:\\code\\inyon" }] }),
        )
        .unwrap();
        let org = get();
        assert_eq!(project_of(&org, "box", "/w/inyon/apps", None).map(|p| &p.id), Some(&inyon.id));
        assert_eq!(project_of(&org, "box", "/w/other", None).map(|p| &p.id), Some(&wide.id));
        assert_eq!(project_of(&org, "laptop", "c:/code/inyon/x", None).map(|p| &p.id), Some(&inyon.id));
        assert!(project_of(&org, "laptop", "/w/inyon", None).is_none(), "another machine's folder");
        let mut m: SessionMark = serde_json::from_value(json!({ "id": "s-1", "host": "box", "pane": "%1", "project": wide.id })).unwrap();
        assert_eq!(project_of(&org, "box", "/w/inyon", Some(&m)).map(|p| &p.id), Some(&wide.id), "put there by hand");
        m.project = Some("p-gone".into());
        assert_eq!(project_of(&org, "box", "/w/inyon", Some(&m)).map(|p| &p.id), Some(&inyon.id));
    });
}

#[test]
fn removing_a_project_or_group_leaves_nothing_pointing_at_it() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None), ("%4", "/w", None)])]));
        let g = add_group(&json!({ "name": "Clients", "color": "blue" })).unwrap();
        let p = add_project(&json!({ "name": "Site", "group": g.id })).unwrap();
        let s = add_space(&json!({ "name": "Focus", "projects": [p.id] })).unwrap();
        mark_pane("box", "%3", &patch(json!({ "project": p.id }))).unwrap();
        mark_pane("box", "%4", &patch(json!({ "project": p.id, "pinned": true }))).unwrap();
        assert_eq!(mark_pane("box", "%4", &patch(json!({ "project": "p-nope" }))).unwrap_err().status, Some(404));
        remove_group(&g.id).unwrap();
        assert_eq!(get().projects[0].group, None);
        remove_project(&p.id).unwrap();
        let org = get();
        assert_eq!(org.sessions.len(), 1, "the mark that only held the project went");
        assert_eq!(org.sessions[0].project, None);
        assert!(org.spaces.iter().find(|x| x.id == s.id).unwrap().projects.is_empty());
        set_order(&json!({ "spaces": [s.id] })).unwrap();
        assert_eq!(remove_space("w-nope").unwrap_err().status, Some(404));
    });
}

#[test]
fn a_change_refused_half_way_leaves_nothing_behind() {
    run(|| async {
        fresh();
        let g = add_group(&json!({ "name": "Clients" })).unwrap();
        // the name is fine, the colour isn't: neither is kept
        assert_eq!(update_group(&g.id, &json!({ "name": "Renamed", "color": "gold" })).unwrap_err().message, "Unknown colour");
        assert_eq!(get().groups[0].name, "Clients");
        add_space(&json!({ "name": "Focus" })).unwrap();
        assert_eq!(kv::get_value(KEY).unwrap()["groups"][0]["name"], json!("Clients"), "and the next save doesn't write it either");
    });
}

#[test]
fn an_unreadable_record_is_never_written_over_before_a_copy_of_it_is_kept() {
    run(|| async {
        fresh();
        let store = Rc::new(Flaky { inner: crate::platform::store::MemoryStore::new(), key: UNREADABLE_KEY, failing: std::cell::Cell::new(true) });
        crate::platform::store::use_store(store.clone());
        put(machine(&[("$1", "work", &[("%3", "/w", None)])]));
        let broken = json!({ "v": 1, "sessions": "not a list" });
        kv::set_value(KEY, &broken).unwrap();
        assert!(get().sessions.is_empty());
        // the copy couldn't be made: nothing is saved over the original
        let e = mark_pane("box", "%3", &patch(json!({ "pinned": true }))).unwrap_err();
        assert!(e.message.starts_with(UNREADABLE), "{}", e.message);
        assert_eq!(kv::get_value(KEY).unwrap(), broken);
        // once it can be, the copy comes first and the change goes through
        store.failing.set(false);
        mark_pane("box", "%3", &patch(json!({ "pinned": true }))).unwrap();
        assert_eq!(kv::get_value(UNREADABLE_KEY).unwrap(), broken);
        assert_eq!(kv::get_value(KEY).unwrap()["sessions"][0]["pinned"], json!(0));
        crate::platform::store::use_store(Rc::new(crate::platform::store::MemoryStore::new()));
    });
}

#[test]
fn a_new_session_of_the_same_name_given_an_old_pane_id_is_not_the_old_session() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w/inyon", None)])]));
        mark_pane("box", "%3", &patch(json!({ "pinned": true }))).unwrap();
        // tmux restarted and a new "work" session got %3, in another folder
        assert!(reconcile(&[machine(&[("$0", "work", &[("%3", "/tmp", None)])])]));
        assert!(mark("box", "%3").is_none());
        // the same name in the same folder is still taken for it
        assert!(reconcile(&[machine(&[("$0", "work", &[("%3", "/w/inyon", None)])])]));
        assert!(mark("box", "%3").is_some());
    });
}

#[test]
fn marks_on_a_machine_that_is_not_connected_wait_for_it() {
    run(|| async {
        fresh();
        let box_ = machine(&[("$1", "work", &[("%3", "/w", None)])]);
        put(box_.clone());
        let heard = Rc::new(RefCell::new(vec![]));
        let h = heard.clone();
        on_event(move |e| h.borrow_mut().push(e.clone()));
        mark_pane("box", "%3", &patch(json!({ "snoozedUntil": T + 60_000, "snoozeNotice": true }))).unwrap();
        // the machine drops off, still listing the panes it had
        put(offline(box_.clone()));
        assert!(!reconcile(&[offline(box_.clone())]), "nothing moves while it's away");
        heard.borrow_mut().clear();
        assert!(!wake_due(T + 60_000), "its snooze waits for it");
        assert!(heard.borrow().is_empty());
        // a month away: it isn't refreshed, and it isn't forgotten either while the machine is still there
        set_now(Some(T + FORGET_MS + 1));
        reconcile(&[offline(box_.clone())]);
        assert_eq!(get().sessions.len(), 1);
        // back: the snooze ends and says so
        put(box_.clone());
        assert!(wake_due(T + FORGET_MS + 1));
        assert!(heard.borrow().iter().any(|e| matches!(e, Event::Notice { pane, .. } if pane == "%3")));
    });
}

#[test]
fn a_request_that_keeps_nothing_costs_no_other_session_its_mark() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None)])]));
        let full: Vec<Value> = (0..MAX_MARKS).map(|i| json!({ "id": format!("s-{i}"), "host": "box", "pane": format!("%{}", 1000 + i), "seen": T, "parked": true })).collect();
        kv::set_value(KEY, &json!({ "v": 1, "sessions": full })).unwrap();
        assert_eq!(mark_pane("box", "%3", &patch(json!({}))).unwrap(), None);
        assert_eq!(get().sessions.len(), MAX_MARKS);
        // one that keeps something makes room by letting the oldest gone one go
        mark_pane("box", "%3", &patch(json!({ "pinned": true }))).unwrap().unwrap();
        assert_eq!(get().sessions.len(), MAX_MARKS);
        assert!(mark("box", "%3").is_some());
    });
}

#[test]
fn the_clock_forgets_marks_gone_a_month_even_when_nothing_else_happens() {
    run(|| async {
        fresh();
        put(machine(&[("$1", "work", &[("%3", "/w", None)])]));
        mark_pane("box", "%3", &patch(json!({ "parked": true }))).unwrap();
        put(machine(&[]));
        tick(T);
        assert_eq!(get().sessions.len(), 1);
        set_now(Some(T + FORGET_MS));
        tick(T + FORGET_MS);
        assert!(kv::get_value(KEY).unwrap()["sessions"].as_array().unwrap().is_empty());
    });
}
