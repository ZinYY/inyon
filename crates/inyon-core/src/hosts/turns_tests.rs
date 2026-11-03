//! hosts/turns.test.ts

use inyon_protocol::AgentKind;
use serde_json::json;

use super::Shown::{Idle, Waiting, Working};
use super::*;

fn hook(name: &str, ts: f64, extra: serde_json::Value) -> HookPaneState {
    let mut data = json!({ "hook_event_name": name });
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            data[k] = v.clone();
        }
    }
    HookPaneState { pane: "%1".into(), ts, data, ..Default::default() }
}
const T0: u64 = 1_791_000_000_000;
const S0: f64 = (T0 / 1000) as f64;

#[test]
fn an_answered_prompt_shows_working_while_the_screen_reads_idle_until_stop() {
    let mut t = Turns::new();
    t.start("p", T0);
    // the screen changes each second while the approved command runs (spinner, elapsed time)
    assert_eq!(t.state("p", Idle, "Running 1s", None, T0 + 1000), Working);
    assert_eq!(t.state("p", Idle, "Running 5s", Some(&hook("PermissionRequest", S0 - 3.0, json!({}))), T0 + 5000), Working);
    assert_eq!(t.state("p", Idle, "Running 9s", Some(&hook("Stop", S0 + 9.0, json!({}))), T0 + 9500), Idle);
    // and the Stop doesn't start anything again
    assert_eq!(t.state("p", Idle, "done", Some(&hook("Stop", S0 + 9.0, json!({}))), T0 + 12_000), Idle);
}

#[test]
fn a_screen_that_stops_changing_ends_the_turn_when_no_stop_comes() {
    let mut t = Turns::new();
    t.start("p", T0);
    assert_eq!(t.state("p", Idle, "same", None, T0 + 500), Working);
    assert_eq!(t.state("p", Idle, "same", None, T0 + 4500), Working);
    assert_eq!(t.state("p", Idle, "same", None, T0 + 8600), Idle);
}

#[test]
fn a_screen_that_shows_claude_working_is_believed_when_it_goes_idle() {
    let mut t = Turns::new();
    t.start("p", T0);
    assert_eq!(t.state("p", Working, "spinner", None, T0 + 1000), Working);
    assert_eq!(t.state("p", Idle, "input box", None, T0 + 2000), Idle);
}

#[test]
fn a_question_in_the_middle_keeps_the_turn_going_after_it_is_answered_elsewhere() {
    let mut t = Turns::new();
    t.start("p", T0);
    assert_eq!(t.state("p", Waiting, "Do you want to proceed?", None, T0 + 1000), Waiting);
    assert_eq!(t.state("p", Idle, "Running", None, T0 + 2000), Working);
}

#[test]
fn hook_events_start_a_turn_the_screen_misses_and_an_idle_notification_ends_it() {
    let mut t = Turns::new();
    let now = T0 + 2000;
    assert_eq!(t.state("p", Idle, "a", Some(&hook("UserPromptSubmit", S0, json!({}))), now), Working);
    assert_eq!(t.state("p", Idle, "b", Some(&hook("Notification", S0 + 70.0, json!({ "notification_type": "idle_prompt" }))), now + 70_000), Idle);
    // the same old event doesn't bring it back
    assert_eq!(t.state("p", Idle, "c", Some(&hook("UserPromptSubmit", S0, json!({}))), now + 71_000), Idle);
    // an event from long ago starts nothing
    assert_eq!(Turns::new().state("q", Idle, "a", Some(&hook("UserPromptSubmit", S0, json!({}))), T0 + 20 * 60_000), Idle);
}

#[test]
fn esc_from_inyon_ends_it_at_once() {
    let mut t = Turns::new();
    t.start("p", T0);
    t.end("p", T0 + 100);
    assert_eq!(t.state("p", Idle, "x", Some(&hook("PermissionRequest", S0 - 1.0, json!({}))), T0 + 200), Idle);
}

fn copilot(event: &str, ts: f64) -> HookPaneState {
    HookPaneState { pane: "%1".into(), ts, data: json!({ "sessionId": "560a9dd2" }), agent: Some(AgentKind::Copilot), event: Some(event.into()) }
}

#[test]
fn copilots_first_prompt_keeps_working_through_the_session_start_that_follows_it() {
    // what Copilot CLI 1.0.95 sends for a new session's first prompt, 85 ms apart, then agentStop
    let mut t = Turns::new();
    assert_eq!(t.state("p", Idle, "", Some(&copilot("userPromptSubmitted", S0)), T0 + 50), Working);
    assert_eq!(t.state("p", Idle, "", Some(&copilot("sessionStart", S0)), T0 + 600), Working);
    assert_eq!(t.state("p", Idle, "OK", Some(&copilot("agentStop", S0 + 3.0)), T0 + 3500), Idle);
}

#[test]
fn a_turn_that_stops_on_an_error_ends_like_a_stop() {
    let mut t = Turns::new();
    t.start("p", T0);
    assert_eq!(t.state("p", Idle, "Running 1s", None, T0 + 1000), Working);
    assert_eq!(t.state("p", Idle, "Running 2s", Some(&hook("StopFailure", S0 + 2.0, json!({ "error": "rate_limit" }))), T0 + 2500), Idle);
}
