use serde_json::json;

use super::*;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::machine;
use crate::test_support::rt::run;

fn reset() {
    CHOSEN.with(|c| *c.borrow_mut() = Some(IndexMap::new()));
}

#[test]
fn a_setting_reads_as_a_model_or_an_effort_and_nothing_else() {
    assert_eq!(from_setting("/model opus"), Some((Pick::Model, "opus".into())));
    assert_eq!(from_setting(" /effort high "), Some((Pick::Effort, "high".into())));
    assert_eq!(from_setting("/model gpt-5.5 xhigh"), Some((Pick::Model, "gpt-5.5 xhigh".into())));
    assert_eq!(from_setting("/model"), None);
    assert_eq!(from_setting("/compact"), None);
    assert_eq!(from_setting("tell me about /model opus"), None);
}

#[test]
fn a_pick_waits_while_the_agent_works_and_the_last_of_each_kind_wins() {
    run(|| async {
        reset();
        let _fake = Fake::install(|_, _, _| ok(""));
        machine("pick-busy", "local", json!([{ "id": "%1", "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        assert!(hold_if_busy("pick-busy", "%1", Pick::Model, "sonnet").unwrap());
        assert!(hold_if_busy("pick-busy", "%1", Pick::Model, "opus").unwrap());
        assert!(hold_if_busy("pick-busy", "%1", Pick::Mode, "plan").unwrap());
        assert_eq!(get("pick-busy", "%1"), Some(ChosenControls { model: Some("opus".into()), mode: Some("plan".into()), ..Default::default() }));
        assert_eq!(ready(), [("pick-busy".to_owned(), "%1".to_owned())]);
        drop_all("pick-busy", "%1");
        assert_eq!(get("pick-busy", "%1"), None);
    });
}

#[test]
fn a_free_agent_takes_its_pick_at_once_unless_others_wait_and_some_agents_never_wait() {
    run(|| async {
        reset();
        let _fake = Fake::install(|_, _, _| ok(""));
        machine(
            "pick-idle",
            "local",
            json!([
                { "id": "%1", "agent": { "kind": "codex", "pid": 7, "state": "idle" } },
                { "id": "%2", "agent": { "kind": "opencode", "pid": 8, "state": "working" } },
                { "id": "%3", "agent": { "kind": "claude", "pid": 9, "state": "idle", "waitingFor": "permission" } }
            ]),
        );
        assert!(!hold_if_busy("pick-idle", "%1", Pick::Effort, "high").unwrap());
        // OpenCode takes a model through its own server, busy or not
        assert!(!hold_if_busy("pick-idle", "%2", Pick::Model, "big-pickle").unwrap());
        // asking for permission: it can't take keys for a menu now
        assert!(hold_if_busy("pick-idle", "%3", Pick::Effort, "low").unwrap());
        // a pick already waiting keeps the order: the next one waits behind it
        with(|all| all.insert(key("pick-idle", "%1"), ChosenControls { mode: Some("plan".into()), ..Default::default() }));
        assert!(hold_if_busy("pick-idle", "%1", Pick::Effort, "high").unwrap());
        // no agent in the pane: nothing to wait for, the usual error comes from making it
        assert!(!hold_if_busy("pick-idle", "%9", Pick::Mode, "plan").unwrap());
    });
}

#[test]
fn picks_go_mode_first_and_a_failure_keeps_its_reason_and_stops_the_rest() {
    run(|| async {
        reset();
        let _fake = Fake::install(|_, _, _| ok(""));
        machine("pick-apply", "local", json!([{ "id": "%1", "agent": { "kind": "gemini", "pid": 7, "state": "idle" } }]));
        with(|all| all.insert(key("pick-apply", "%1"), ChosenControls { model: Some("opus".into()), mode: Some("plan".into()), ..Default::default() }));
        // Gemini CLI has no modes: the mode fails first, and the model isn't tried
        apply("pick-apply", "%1").await;
        let c = get("pick-apply", "%1").unwrap();
        assert_eq!((c.mode.as_deref(), c.model.as_deref()), (Some("plan"), Some("opus")));
        assert!(c.error.as_deref().is_some_and(|e| e.contains("not supported")), "{c:?}");
        assert!(ready().is_empty(), "a failed pick isn't tried again");
        assert!(!pending("pick-apply", "%1"));
    });
}
