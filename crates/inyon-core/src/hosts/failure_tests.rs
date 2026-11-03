use serde_json::json;

use super::*;

const NOW: f64 = 1_791_000_000.0;

fn hook(data: serde_json::Value) -> HookPaneState {
    HookPaneState { pane: "%1".into(), ts: NOW, data, ..Default::default() }
}

fn window(pct: f64, resets_at: Option<f64>) -> RateWindow {
    RateWindow { pct: pct.into(), resets_at: resets_at.map(Into::into) }
}

#[test]
fn only_a_stop_failure_is_a_failure() {
    assert_eq!(turn_failure(&hook(json!({ "hook_event_name": "Stop" })), &[], NOW), None);
    let f = turn_failure(&hook(json!({ "hook_event_name": "StopFailure", "error": "authentication_failed" })), &[], NOW).unwrap();
    assert_eq!(f, TurnFailure { error: "authentication_failed".into(), resets_at: None });
    assert_eq!(failure_text(&f, NOW), "signed out");
}

#[test]
fn an_error_claude_code_doesnt_name_is_unknown() {
    let f = turn_failure(&hook(json!({ "hook_event_name": "StopFailure" })), &[], NOW).unwrap();
    assert_eq!(f.error, "unknown");
    assert_eq!(failure_text(&f, NOW), "an error");
}

#[test]
fn a_rate_limit_resets_when_the_last_full_limit_does() {
    let five = window(100.0, Some(NOW + 2.0 * 3600.0 + 600.0));
    let week = window(100.0, Some(NOW + 3.0 * 86400.0));
    let half = window(40.0, Some(NOW + 600.0));
    let h = hook(json!({ "hook_event_name": "StopFailure", "error": "rate_limit" }));
    let f = turn_failure(&h, &[&five, &half], NOW).unwrap();
    assert_eq!(f.resets_at, Some((NOW + 7800.0) as u64));
    assert_eq!(failure_text(&f, NOW), "rate limit, resets in 2 h 10 min");
    // both full: nothing runs until the weekly one resets
    assert_eq!(failure_text(&turn_failure(&h, &[&five, &week], NOW).unwrap(), NOW), "rate limit, resets in 3 days");
}

#[test]
fn a_rate_limit_with_no_full_limit_or_one_in_the_past_says_no_time() {
    let h = hook(json!({ "hook_event_name": "StopFailure", "error": "rate_limit" }));
    assert_eq!(turn_failure(&h, &[&window(97.0, Some(NOW + 60.0))], NOW).unwrap().resets_at, None);
    assert_eq!(turn_failure(&h, &[&window(100.0, Some(NOW - 60.0))], NOW).unwrap().resets_at, None);
    assert_eq!(turn_failure(&h, &[&window(100.0, None)], NOW).unwrap().resets_at, None);
}

#[test]
fn other_errors_carry_no_reset_time() {
    let h = hook(json!({ "hook_event_name": "StopFailure", "error": "overloaded" }));
    let f = turn_failure(&h, &[&window(100.0, Some(NOW + 60.0))], NOW).unwrap();
    assert_eq!(f.resets_at, None);
    assert_eq!(failure_text(&f, NOW), "servers overloaded");
}

#[test]
fn times_from_now_read_naturally() {
    let at = |s: f64| failure_text(&TurnFailure { error: "rate_limit".into(), resets_at: Some((NOW + s) as u64) }, NOW);
    assert_eq!(at(30.0), "rate limit, resets in 1 min");
    assert_eq!(at(25.0 * 60.0), "rate limit, resets in 25 min");
    assert_eq!(at(3.0 * 3600.0), "rate limit, resets in 3 h");
    assert_eq!(at(86400.0 + 5.0 * 3600.0), "rate limit, resets in 1 day 5 h");
}
