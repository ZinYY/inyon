//! What the hub does on its own: messages sent later go once, and once only across a restart;
//! idle agents sleep after their rule's minutes and not before; sessions close once done; and the
//! saved prompts' limits.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};

use super::auto::{self, Act, CLOSE_SETTLE_MS};
use super::*;
use crate::clock::set_now;
use crate::hosts::host::Host;
use crate::hosts::registry::HostRuntime;
use crate::test_support::rt::run;

const T: u64 = 1_790_000_000_000;
const MIN: u64 = 60_000;

/// A pane: its id, folder, agent's conversation (None: a plain shell) and the agent's state.
type Pane<'a> = (&'a str, &'a str, Option<&'a str>, &'a str);

/// A machine "box" with one tmux session `$1` holding `panes`.
fn machine(panes: &[Pane]) -> HostState {
    let panes: Vec<Value> = panes
        .iter()
        .enumerate()
        .map(|(i, (pane, cwd, conv, state))| {
            let mut p =
                json!({ "id": pane, "index": i, "active": i == 0, "command": "claude", "cwd": cwd, "title": format!("✳ Task {pane}"), "width": 100, "height": 30, "dead": false });
            if let Some(c) = conv {
                p["agent"] = json!({ "kind": "claude", "pid": 1, "state": state });
                if !c.is_empty() {
                    p["agent"]["sessionId"] = json!(c);
                }
            }
            p
        })
        .collect();
    serde_json::from_value(
        json!({ "config": { "id": "box", "name": "Box", "kind": "ssh" }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0,
        "sessions": [{ "id": "$1", "name": "work", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": panes }] }] }),
    )
    .unwrap()
}

/// The machine as the registry has it now; returns it for the checks that take the machines.
fn put(panes: &[Pane]) -> Vec<HostState> {
    let state = machine(panes);
    let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state.clone())));
    rt.borrow_mut().hold_polls();
    registry().insert_runtime("box", rt);
    reconcile(std::slice::from_ref(&state));
    vec![state]
}

/// A fresh record, and every hand-off written down instead of done; `fail` makes them fail.
fn fresh(fail: Option<&'static str>) -> Rc<RefCell<Vec<Act>>> {
    reset();
    kv::del(KEY);
    set_now(Some(T));
    let acts = Rc::new(RefCell::new(vec![]));
    let a = acts.clone();
    auto::set_actions(Some(Rc::new(move |x: &Act| {
        a.borrow_mut().push(x.clone());
        fail.map_or(Ok(()), |m| Err(http(409, m)))
    })));
    acts
}

fn hold(pane: &str, text: &str) -> Act {
    Act::Hold { host: "box".into(), pane: pane.into(), text: text.into() }
}

#[test]
fn a_message_sent_later_goes_once_at_its_time_and_never_again_after_a_restart() {
    run(|| async {
        let acts = fresh(None);
        let hosts = put(&[("%3", "/w/inyon", Some("conv-a"), "idle")]);
        let s = auto::add_send("box", "%3", &json!({ "text": "run the tests\n", "at": T + 10 * MIN }), T).unwrap();
        assert_eq!((s.text.as_str(), s.conv.as_deref(), s.tmux_id.as_deref()), ("run the tests", Some("conv-a"), Some("$1")));
        assert!(for_overview().sends.is_some(), "the v1 overview has them, so an app can show them");

        assert!(!auto::send_due(T + 9 * MIN, &hosts));
        assert!(acts.borrow().is_empty(), "not before its time");
        // the hand-off sees it gone from the record already: stopping right after can't send it twice
        auto::set_actions(Some(Rc::new({
            let acts = acts.clone();
            move |x: &Act| {
                assert!(kv::get_value(KEY).unwrap().get("sends").is_none(), "written down before it's handed on");
                acts.borrow_mut().push(x.clone());
                Ok(())
            }
        })));
        assert!(auto::send_due(T + 10 * MIN, &hosts));
        assert_eq!(*acts.borrow(), vec![hold("%3", "run the tests")]);

        // the hub restarts: nothing goes again
        reset();
        auto::send_due(T + 20 * MIN, &hosts);
        auto::send_due(T + 30 * MIN, &hosts);
        assert_eq!(acts.borrow().len(), 1);
        assert_eq!(get().sends, None);
    });
}

#[test]
fn a_message_follows_its_conversation_and_waits_with_the_reason_when_it_cant_go() {
    run(|| async {
        let acts = fresh(None);
        put(&[("%3", "/w/inyon", Some("conv-a"), "idle"), ("%4", "/w", Some("conv-b"), "idle")]);
        auto::add_send("box", "%3", &json!({ "text": "a", "at": T + MIN }), T).unwrap();
        auto::add_send("box", "%4", &json!({ "text": "b", "at": T + MIN }), T).unwrap();
        // conv-a was resumed in %9; conv-b's agent is gone, and %4 with it
        let hosts = put(&[("%9", "/w/inyon", Some("conv-a"), "idle")]);
        auto::send_due(T + MIN, &hosts);
        assert_eq!(*acts.borrow(), vec![hold("%9", "a")]);
        let left = get().sends.unwrap();
        assert_eq!((left.len(), left[0].error.as_deref()), (1, Some("The session isn't there any more")));
        // kept to edit or dismiss, never tried again by itself
        auto::send_due(T + 2 * MIN, &hosts);
        assert_eq!(acts.borrow().len(), 1);
        auto::remove_send(&left[0].id).unwrap();
        assert_eq!(get().sends, None);
    });
}

#[test]
fn a_message_the_held_queue_refuses_comes_back_with_the_reason() {
    run(|| async {
        let acts = fresh(Some("50 messages are held for this session already. Send or drop some first."));
        let hosts = put(&[("%3", "/w/inyon", Some("conv-a"), "idle")]);
        let s = auto::add_send("box", "%3", &json!({ "text": "a", "at": T + MIN }), T).unwrap();
        auto::send_due(T + MIN, &hosts);
        let left = get().sends.unwrap();
        assert_eq!((left[0].id.as_str(), left[0].error.as_deref()), (s.id.as_str(), Some("50 messages are held for this session already. Send or drop some first.")));
        auto::send_due(T + 2 * MIN, &hosts);
        assert_eq!(acts.borrow().len(), 1);
        // edited: it can go again at the new time
        auto::update_send(&s.id, &json!({ "at": T + 5 * MIN }), T + 2 * MIN).unwrap();
        assert_eq!(get().sends.unwrap()[0].error, None);
    });
}

#[test]
fn messages_sent_later_are_checked() {
    run(|| async {
        fresh(None);
        put(&[("%3", "/w/inyon", Some("conv-a"), "idle"), ("%4", "/w", None, "idle")]);
        let err = |b: Value, pane: &str| auto::add_send("box", pane, &b, T).unwrap_err();
        assert_eq!(err(json!({ "text": "a", "at": T }), "%3").message, "Pick a time in the future to send it");
        assert_eq!(err(json!({ "text": "a", "at": T + 367 * 24 * 60 * MIN }), "%3").message, "A message can wait a year at most");
        assert_eq!(err(json!({ "text": "  ", "at": T + MIN }), "%3").message, "Message is empty");
        assert_eq!(err(json!({ "text": "x".repeat(4_001), "at": T + MIN }), "%3").message, "A message can be at most 4000 characters");
        assert_eq!(err(json!({ "text": "a", "at": T + MIN }), "%4").status, Some(409));
        assert_eq!(err(json!({ "text": "a", "at": T + MIN }), "%99").status, Some(404));
        for _ in 0..inyon_protocol::MAX_SENDS {
            auto::add_send("box", "%3", &json!({ "text": "a", "at": T + MIN }), T).unwrap();
        }
        assert_eq!(err(json!({ "text": "a", "at": T + MIN }), "%3").status, Some(409));
    });
}

#[test]
fn an_idle_agent_sleeps_after_its_rules_minutes_and_only_once() {
    run(|| async {
        let acts = fresh(None);
        let sleep = |pane: &str| Act::Sleep { host: "box".into(), pane: pane.into() };
        let panes = [("%3", "/w/inyon", Some("conv-a"), "idle"), ("%4", "/w/other", Some("conv-b"), "idle"), ("%5", "/w", Some(""), "idle")];
        let hosts = put(&panes);
        // off unless asked for
        auto::check_agents(T, &hosts);
        auto::check_agents(T + 600 * MIN, &hosts);
        assert!(acts.borrow().is_empty());

        assert_eq!(auto::set_sleep(&json!({ "minutes": 3 })).unwrap_err().message, "Pick between 5 minutes and 24 hours");
        auto::set_sleep(&json!({ "scope": "all", "minutes": 10 })).unwrap();
        let p = add_project(&json!({ "name": "Inyon", "roots": [{ "host": "box", "path": "/w/inyon" }] })).unwrap();
        auto::set_sleep(&json!({ "scope": p.id, "minutes": 30 })).unwrap();
        // idle since T: past 10 minutes, so %4 sleeps at the first look; %5 has no conversation to
        // come back to, so it stays, and %3's project gives it 30 minutes
        let hosts = put(&[("%3", "/w/inyon", Some("conv-a"), "working"), panes[1], panes[2]]);
        auto::check_agents(T + 601 * MIN, &hosts);
        assert_eq!(*acts.borrow(), vec![sleep("%4")]);
        auto::check_agents(T + 602 * MIN, &hosts);
        assert_eq!(acts.borrow().len(), 1, "once");
        let hosts = put(&panes);
        auto::check_agents(T + 610 * MIN, &hosts);
        auto::check_agents(T + 639 * MIN, &hosts);
        assert_eq!(acts.borrow().len(), 1, "not before its project's 30 minutes");
        auto::check_agents(T + 640 * MIN, &hosts);
        assert_eq!(*acts.borrow(), vec![sleep("%4"), sleep("%3")]);

        // work starts the count again, and a pinned session stays awake
        acts.borrow_mut().clear();
        mark_pane("box", "%4", &SessionPatch { pinned: Some(true), ..Default::default() }).unwrap();
        let busy = put(&[("%3", "/w/inyon", Some("conv-a"), "working"), ("%4", "/w/other", Some("conv-b"), "idle")]);
        auto::check_agents(T + 640 * MIN, &busy);
        let hosts = put(&panes);
        auto::check_agents(T + 650 * MIN, &hosts);
        auto::check_agents(T + 679 * MIN, &hosts);
        assert!(acts.borrow().is_empty());
        auto::check_agents(T + 680 * MIN, &hosts);
        assert_eq!(*acts.borrow(), vec![sleep("%3")]);

        // a project that goes takes its rule with it
        remove_project(&p.id).unwrap();
        assert_eq!(get().sleep.unwrap(), vec![inyon_protocol::SleepRule { scope: "all".into(), minutes: 10 }]);
    });
}

#[test]
fn a_session_closes_once_its_next_work_is_done_even_across_a_restart() {
    run(|| async {
        let acts = fresh(None);
        let end = Act::End { host: "box".into(), pane: "%3".into() };
        let idle = put(&[("%3", "/w/inyon", Some("conv-a"), "idle")]);
        let m = mark_pane("box", "%3", &SessionPatch::read(&json!({ "closeWhenDone": true }), T).unwrap()).unwrap().unwrap();
        assert_eq!((m.close_when_done, m.close_saw_work), (Some(true), None));
        // idle already: it waits for the next piece of work
        auto::check_agents(T, &idle);
        auto::check_agents(T + MIN, &idle);
        assert!(acts.borrow().is_empty());

        let working = put(&[("%3", "/w/inyon", Some("conv-a"), "working")]);
        auto::check_agents(T + 2 * MIN, &working);
        assert_eq!(kv::get_value(KEY).unwrap()["sessions"][0]["closeSawWork"], json!(true), "written down");

        // the hub restarts while the agent works; it finishes after
        reset();
        let idle = put(&[("%3", "/w/inyon", Some("conv-a"), "idle")]);
        auto::check_agents(T + 3 * MIN, &idle);
        assert!(acts.borrow().is_empty(), "idle for a moment first");
        auto::check_agents(T + 3 * MIN + CLOSE_SETTLE_MS, &idle);
        assert_eq!(*acts.borrow(), vec![end.clone()]);
        assert!(get().sessions.is_empty(), "nothing else was kept on it");
        auto::check_agents(T + 4 * MIN, &idle);
        assert_eq!(acts.borrow().len(), 1, "once");
    });
}

#[test]
fn saved_prompts_are_kept_ordered_and_checked() {
    run(|| async {
        fresh(None);
        let p = add_project(&json!({ "name": "Inyon" })).unwrap();
        let a = auto::add_prompt(&json!({ "name": "Review", "text": "Review the diff", "starred": true })).unwrap();
        let b = auto::add_prompt(&json!({ "name": "Ship", "text": "Open a PR", "project": p.id })).unwrap();
        assert_eq!((a.starred, b.project.as_deref()), (Some(true), Some(p.id.as_str())));
        assert_eq!(auto::add_prompt(&json!({ "text": "x" })).unwrap_err().message, "Give the prompt a name");
        assert_eq!(auto::add_prompt(&json!({ "name": "x", "text": "" })).unwrap_err().message, "Message is empty");
        assert_eq!(auto::add_prompt(&json!({ "name": "x", "text": "y", "project": "p-nope" })).unwrap_err().status, Some(404));
        auto::update_prompt(&a.id, &json!({ "starred": false, "text": "Review it" })).unwrap();
        set_order(&json!({ "prompts": [b.id] })).unwrap();
        let ids: Vec<String> = get().prompts.unwrap().into_iter().map(|q| q.id).collect();
        assert_eq!(ids, vec![b.id.clone(), a.id.clone()]);
        assert!(for_overview().prompts.is_none());
        // a removed project's prompts stay, for every session
        remove_project(&p.id).unwrap();
        assert_eq!(get().prompts.unwrap()[0].project, None);
        auto::remove_prompt(&a.id).unwrap();
        auto::remove_prompt(&b.id).unwrap();
        assert_eq!(get().prompts, None, "an empty list isn't written");
    });
}

#[test]
fn nothing_is_done_to_a_machine_that_is_not_online_until_it_is_back() {
    run(|| async {
        let acts = fresh(None);
        let panes = [("%3", "/w", Some("conv-a"), "idle")];
        let hosts = put(&panes);
        auto::add_send("box", "%3", &json!({ "text": "go on", "at": T + MIN }), T).unwrap();
        auto::set_sleep(&json!({ "scope": "all", "minutes": 10 })).unwrap();
        // it drops off, still listing the idle agent it had
        let mut away = hosts.clone();
        away[0].status = HostStatus::Error;
        auto::send_due(T + 30 * MIN, &away);
        auto::check_agents(T + 30 * MIN, &away);
        auto::check_agents(T + 60 * MIN, &away);
        assert!(acts.borrow().is_empty(), "no message, no sleep");
        assert_eq!(get().sends.unwrap()[0].error, None, "the message waits, without an error");
        // back: the message goes
        assert!(auto::send_due(T + 61 * MIN, &hosts));
        assert_eq!(*acts.borrow(), vec![hold("%3", "go on")]);
    });
}

#[test]
fn a_message_whose_conversation_is_gone_never_goes_to_another_one_in_its_pane() {
    run(|| async {
        let acts = fresh(None);
        put(&[("%3", "/w", Some("conv-a"), "idle")]);
        let s = auto::add_send("box", "%3", &json!({ "text": "for a", "at": T + MIN }), T).unwrap();
        // another conversation was started in %3
        let hosts = put(&[("%3", "/w", Some("conv-z"), "idle")]);
        auto::send_due(T + MIN, &hosts);
        assert!(acts.borrow().is_empty());
        assert_eq!(get().sends.unwrap()[0].error.as_deref(), Some("The session isn't there any more"));
        // Send now says the same
        assert_eq!(auto::send_now(&s.id).unwrap_err().status, Some(409));
        assert!(acts.borrow().is_empty());
    });
}

#[test]
fn a_waiting_message_moves_with_its_conversation_and_keeps_that_agent_awake() {
    run(|| async {
        let acts = fresh(None);
        put(&[("%3", "/w", Some("conv-a"), "idle")]);
        auto::add_send("box", "%3", &json!({ "text": "go on", "at": T + 600 * MIN }), T).unwrap();
        auto::set_sleep(&json!({ "minutes": 10 })).unwrap();
        // resumed in %9: the chat there shows it, and the agent there isn't put to sleep before it goes
        let hosts = put(&[("%9", "/w", Some("conv-a"), "idle")]);
        assert!(auto::follow_sends(&hosts));
        assert_eq!(get().sends.unwrap()[0].pane, "%9");
        assert!(!auto::follow_sends(&hosts), "once");
        auto::check_agents(T, &hosts);
        auto::check_agents(T + 60 * MIN, &hosts);
        assert!(acts.borrow().is_empty());
    });
}

#[test]
fn send_now_takes_it_off_the_list_and_holds_it_in_one_step() {
    run(|| async {
        let acts = fresh(None);
        put(&[("%3", "/w", Some("conv-a"), "idle")]);
        let s = auto::add_send("box", "%3", &json!({ "text": "now", "at": T + 600 * MIN }), T).unwrap();
        assert_eq!(auto::send_now(&s.id).unwrap(), "%3");
        assert_eq!(*acts.borrow(), vec![hold("%3", "now")]);
        assert_eq!(get().sends, None);
        assert_eq!(auto::send_now(&s.id).unwrap_err().status, Some(404));
    });
}

#[test]
fn one_put_back_keeps_the_conversation_it_was_for() {
    run(|| async {
        fresh(None);
        put(&[("%3", "/w", Some("conv-b"), "idle")]);
        let s = auto::add_send("box", "%3", &json!({ "text": "a", "at": T + MIN, "conv": "conv-a" }), T).unwrap();
        assert_eq!(s.conv.as_deref(), Some("conv-a"));
        assert_eq!(auto::add_send("box", "%3", &json!({ "text": "a", "at": T + MIN, "conv": 7 }), T).unwrap_err().status, Some(400));
    });
}

/// A store whose writes of the record fail while `failing` is set, as on a full disk.
struct Flaky {
    inner: crate::platform::store::MemoryStore,
    failing: std::cell::Cell<bool>,
}

impl crate::platform::store::Store for Flaky {
    fn get(&self, key: &str) -> Option<Value> {
        self.inner.get(key)
    }
    fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        if key == KEY && self.failing.get() {
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

#[test]
fn a_refused_message_that_cant_be_written_back_is_still_kept() {
    run(|| async {
        fresh(None);
        let store = Rc::new(Flaky { inner: crate::platform::store::MemoryStore::new(), failing: std::cell::Cell::new(false) });
        crate::platform::store::use_store(store.clone());
        reset();
        let hosts = put(&[("%3", "/w", Some("conv-a"), "idle")]);
        let s = auto::add_send("box", "%3", &json!({ "text": "a", "at": T + MIN }), T).unwrap();
        // the held queue refuses it, and the disk is full by then
        let st = store.clone();
        auto::set_actions(Some(Rc::new(move |_: &Act| {
            st.failing.set(true);
            Err(http(409, "No room in the held queue"))
        })));
        auto::send_due(T + MIN, &hosts);
        let kept = get().sends.unwrap();
        assert_eq!((kept[0].id.as_str(), kept[0].error.as_deref()), (s.id.as_str(), Some("No room in the held queue")), "still there to retry or dismiss");
        // written down with the next save that works
        store.failing.set(false);
        auto::update_send(&s.id, &json!({ "at": T + 5 * MIN }), T + 2 * MIN).unwrap();
        assert_eq!(kv::get_value(KEY).unwrap()["sends"][0]["id"], json!(s.id));
        crate::platform::store::use_store(Rc::new(crate::platform::store::MemoryStore::new()));
    });
}

#[test]
fn a_new_agent_in_the_same_pane_starts_its_own_idle_time() {
    run(|| async {
        let acts = fresh(None);
        auto::set_sleep(&json!({ "minutes": 10 })).unwrap();
        let hosts = put(&[("%3", "/w", Some("conv-a"), "idle")]);
        auto::check_agents(T, &hosts);
        // the agent exits and another one, with its own conversation, is idle in %3 by the next look
        let hosts = put(&[("%3", "/w", Some("conv-b"), "idle")]);
        auto::check_agents(T + 9 * MIN, &hosts);
        auto::check_agents(T + 11 * MIN, &hosts);
        assert!(acts.borrow().is_empty(), "idle 2 minutes, not 11");
        auto::check_agents(T + 19 * MIN, &hosts);
        assert_eq!(*acts.borrow(), vec![Act::Sleep { host: "box".into(), pane: "%3".into() }]);
    });
}

#[test]
fn a_session_that_could_not_be_ended_is_tried_again_after_a_while() {
    run(|| async {
        let acts = fresh(Some("tmux didn't answer"));
        let end = Act::End { host: "box".into(), pane: "%3".into() };
        put(&[("%3", "/w", Some("conv-a"), "idle")]);
        mark_pane("box", "%3", &SessionPatch::read(&json!({ "closeWhenDone": true }), T).unwrap()).unwrap();
        auto::check_agents(T, &put(&[("%3", "/w", Some("conv-a"), "working")]));
        let idle = put(&[("%3", "/w", Some("conv-a"), "idle")]);
        auto::check_agents(T + MIN, &idle);
        auto::check_agents(T + MIN + CLOSE_SETTLE_MS, &idle);
        assert_eq!(*acts.borrow(), vec![end.clone()]);
        let m = get().sessions.into_iter().find(|m| m.pane == "%3").unwrap();
        assert_eq!((m.close_when_done, m.close_saw_work), (Some(true), Some(true)), "still to close");
        auto::check_agents(T + MIN + CLOSE_SETTLE_MS + 5_000, &idle);
        assert_eq!(acts.borrow().len(), 1, "not again at once");
        auto::check_agents(T + 2 * MIN + CLOSE_SETTLE_MS, &idle);
        assert_eq!(*acts.borrow(), vec![end.clone(), end]);
    });
}
