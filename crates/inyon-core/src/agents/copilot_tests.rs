//! agents/copilot.test.ts, test for test. Each test's name in the TS file is its doc comment. The
//! screen tests in that file check copilot-screen.ts and the catalog test copilot-models.ts; they
//! run against those ports here too.

use super::*;
use crate::agents::copilot_screen::parse_copilot_screen;
use crate::claude::transcript::new_meta;
use crate::test_support::{assert_match, screen};
use serde_json::json;

fn event(t: &str, data: Value) -> Value {
    json!({ "type": t, "data": data, "id": "event-id", "timestamp": "2026-10-10T12:00:00Z" })
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Copilot records user, tool execution, and final reply without duplicated tool requests
#[test]
fn copilot_records_user_tool_execution_and_final_reply_without_duplicated_tool_requests() {
    let mut meta = new_meta();
    normalize_copilot(&event("session.start", json!({ "copilotVersion": "1.0.95", "context": { "cwd": "/qa" }, "selectedModel": "auto" })), &mut meta);
    assert_eq!(meta.cwd.as_deref(), Some("/qa"));
    assert_match(&normalize_copilot(&event("user.message", json!({ "content": "Run pwd" })), &mut meta)[0], json!({ "kind": "user", "text": "Run pwd" }));
    assert_eq!(normalize_copilot(&event("assistant.message", json!({ "content": "", "toolRequests": [{ "toolCallId": "t" }] })), &mut meta), vec![]);
    assert_match(
        &normalize_copilot(&event("tool.execution_start", json!({ "toolCallId": "t", "toolName": "bash", "arguments": { "command": "pwd" } })), &mut meta)[0],
        json!({ "block": { "type": "tool_use", "name": "Bash", "input": { "command": "pwd" } } }),
    );
    assert_match(
        &normalize_copilot(&event("tool.execution_complete", json!({ "toolCallId": "t", "success": false, "error": { "message": "denied" } })), &mut meta)[0],
        json!({ "kind": "tool_result", "isError": true, "text": "denied" }),
    );
    normalize_copilot(&event("assistant.turn_end", json!({})), &mut meta);
    assert_eq!(meta.stats.turns, 1);
}

/// Copilot credits never become dollar cost and session settings reflect recorded values
#[test]
fn copilot_credits_never_become_dollar_cost_and_session_settings_reflect_recorded_values() {
    let mut meta = new_meta();
    normalize_copilot(&event("session.model_change", json!({ "newModel": "auto", "reasoningEffort": null })), &mut meta);
    normalize_copilot(&event("session.permissions_changed", json!({ "mode": "default" })), &mut meta);
    normalize_copilot(&event("assistant.usage", json!({ "cost": 8, "inputTokens": 100 })), &mut meta);
    normalize_copilot(&event("session.usage_info", json!({ "currentTokens": 100, "tokenLimit": 1000 })), &mut meta);
    assert_eq!(meta.stats.cost_usd, None);
    assert_match(&meta.status, json!({ "modelId": "auto", "contextPct": 10 }));
    assert_eq!(meta.mode.as_deref(), Some("default"));
}

/// Copilot catalog uses model-specific reasoning and excludes disabled models
#[test]
fn copilot_catalog_uses_model_specific_reasoning_and_excludes_disabled_models() {
    use crate::agents::copilot_models::parse_copilot_models;
    use inyon_protocol::AgentModel;
    let model = |id: &str, label: &str, note: Option<&str>, context: Option<f64>, efforts: &[&str]| AgentModel {
        id: id.into(),
        label: label.into(),
        note: note.map(str::to_owned),
        context: context.map(Num),
        efforts: Some(efforts.iter().map(|e| e.to_string()).collect()),
        default_effort: None,
        variants: None,
    };
    let payload = json!({ "models": [{ "id": "gpt-test", "name": "Test", "supportedReasoningEfforts": ["low", "high"], "capabilities": { "limits": { "max_context_window_tokens": 1000 } } }, { "id": "blocked", "policy": { "state": "disabled" } }] });
    assert_eq!(
        parse_copilot_models(&payload),
        Ok(vec![model("auto", "Auto", Some("Copilot chooses an available model for each task"), None, &[]), model("gpt-test", "Test", None, Some(1000.0), &["low", "high"])])
    );
    assert_eq!(parse_copilot_models(&json!({ "models": [] })).unwrap().len(), 1);
    assert!(parse_copilot_models(&json!({ "error": "unauthorized" })).is_err());
}

/// Copilot discovery refuses ambiguous same-process sessions
#[test]
fn copilot_discovery_refuses_ambiguous_same_process_sessions() {
    // the TS test's host answers both commands with the same text
    let out = "/home/u/.copilot/session-state/a/inuse.123.lock\n/home/u/.copilot/session-state/b/inuse.123.lock\n";
    assert_eq!(guess_copilot_session(out, out, &CopilotProc { pid: 123, resume: None }), None);
    assert_eq!(guess_copilot_session(out, out, &CopilotProc { pid: 123, resume: Some("b".into()) }).as_deref(), Some("/home/u/.copilot/session-state/b/events.jsonl"));
}

fn rule() -> String {
    "─".repeat(100)
}

/// Copilot remains working during silent tools and reads shell input
#[test]
fn copilot_remains_working_during_silent_tools_and_reads_shell_input() {
    let rule = rule();
    let working = parse_copilot_screen(&lines(&[&rule, "❯", &rule, " ◎ Working · 524 B esc interrupt                           Auto → gpt-test"]), None);
    assert_match(&working, json!({ "working": true, "model": "auto", "inputVisible": true }));
    let shell = parse_copilot_screen(&lines(&[&rule, "! printf hello", &rule, " Shell · Manual Approval                            Auto → gpt-test"]), None);
    assert_match(&shell, json!({ "working": false, "draft": "!printf hello", "mode": "default" }));
    assert_eq!(parse_copilot_screen(&lines(&[&rule, "❯ /plan [prompt]", &rule, " Plan · Manual Approval                         Auto"]), None).draft.as_deref(), Some("/plan"));
}

/// Copilot reads the block-style composer, not the block-style history above it
#[test]
fn copilot_reads_the_block_style_composer_not_the_block_style_history_above_it() {
    let top = format!("╻{}", "▄".repeat(100));
    let bottom = format!("╹{}", "▀".repeat(100));
    let footer = " Interactive · Manual Approval · @ files · # issues                Auto → mai-code-1.1-flash";
    let history = [format!(" {}", "▄".repeat(100)), "  ❯ Reply with just the word HELLO905.              21:44".into(), format!(" {}", "▀".repeat(100)), " ● HELLO905".into()];
    let with = |middle: &[&str]| {
        let mut v: Vec<String> = history.to_vec();
        v.push(top.clone());
        v.extend(middle.iter().map(|s| s.to_string()));
        v.push(bottom.clone());
        v.push(footer.into());
        v
    };
    assert_match(
        &parse_copilot_screen(&with(&["┃ Reply with just the word AGAIN575."]), None),
        json!({ "inputVisible": true, "draft": "Reply with just the word AGAIN575.", "model": "auto", "mode": "default" }),
    );
    let empty = parse_copilot_screen(&with(&["┃"]), None);
    assert_eq!(empty.input_visible, Some(true));
    assert_eq!(empty.draft, None);
    assert_eq!(parse_copilot_screen(&lines(&[&top, "┃ line one", "┃ line two", &bottom, footer]), None).draft.as_deref(), Some("line one\nline two"));
}

/// Copilot parses ANSI input and folder trust as a waiting prompt
#[test]
fn copilot_parses_ansi_input_and_folder_trust_as_a_waiting_prompt() {
    let rule = rule();
    let ansi =
        parse_copilot_screen(&lines(&[&format!("\x1b[2m{rule}"), "\x1b[0m❯ hello", &format!("\x1b[2m{rule}"), "\x1b[0m Interactive · Manual Approval              Auto"]), None);
    assert_eq!(ansi.draft.as_deref(), Some("hello"));
    let trust = parse_copilot_screen(
        &screen("│ Confirm folder trust │\n│ ❯ 1. Yes │\n│   2. Yes, and remember this folder │\n│   3. No (Esc) │\n│ ↑/↓ to navigate · enter to select · esc to cancel │"),
        None,
    );
    let prompt = trust.prompt.as_ref().unwrap();
    assert_eq!(prompt.options.len(), 3);
    assert_match(&prompt.options[0], json!({ "n": 1, "selected": true }));
    assert_eq!(trust.input_visible, None);
}

/// Copilot direct shell input creates a receipt and matching tool output
#[test]
fn copilot_direct_shell_input_creates_a_receipt_and_matching_tool_output() {
    let mut meta = new_meta();
    let items =
        normalize_copilot(&event("tool.user_requested", json!({ "toolName": "local_shell", "toolCallId": "shell1", "arguments": { "command": "printf hello" } })), &mut meta);
    assert_match(&items[0], json!({ "kind": "user", "text": "!printf hello" }));
    assert_match(&items[1], json!({ "kind": "assistant", "block": { "name": "Bash" } }));
    assert_match(
        &normalize_copilot(&event("tool.execution_complete", json!({ "toolCallId": "shell1", "success": true, "result": { "content": "hello" } })), &mut meta)[0],
        json!({ "kind": "tool_result", "text": "hello", "meta": { "stdout": "hello" } }),
    );
    normalize_copilot(&event("session.permissions_changed", json!({ "allowAllPermissions": false, "allowAllPermissionMode": "off" })), &mut meta);
    assert_eq!(meta.mode.as_deref(), Some("default"));
}

fn lock(id: &str, pid: i64) -> String {
    format!("/home/u/.copilot/session-state/{id}/inuse.{pid}.lock")
}

fn ps(rows: &[(i64, i64)]) -> Vec<PsRow> {
    rows.iter().map(|&(pid, ppid)| PsRow { pid, ppid }).collect()
}

fn p(pid: i64, resume: Option<&str>) -> CopilotProc {
    CopilotProc { pid, resume: resume.map(str::to_owned) }
}

/// Copilot tool calls with null arguments read as calls with no input
#[test]
fn copilot_tool_calls_with_null_arguments_read_as_calls_with_no_input() {
    let mut meta = new_meta();
    assert_match(
        &normalize_copilot(&event("tool.execution_start", json!({ "toolCallId": "c1", "toolName": "bash", "arguments": null })), &mut meta),
        json!([{ "kind": "assistant", "block": { "type": "tool_use", "id": "c1", "name": "Bash", "input": {} } }]),
    );
    assert_match(
        &normalize_copilot(&event("tool.execution_start", json!({ "toolCallId": "c2", "toolName": "mystery", "arguments": null })), &mut meta),
        json!([{ "block": { "id": "c2", "name": "mystery", "input": {} } }]),
    );
    assert_match(
        &normalize_copilot(&event("tool.user_requested", json!({ "toolCallId": "c3", "toolName": "local_shell", "arguments": null })), &mut meta),
        json!([{ "kind": "user", "text": "!" }, { "block": { "id": "c3", "name": "Bash" } }]),
    );
}

/// Copilot session ownership follows an npm launcher but prefers closest locks
#[test]
fn copilot_session_ownership_follows_an_npm_launcher_but_prefers_closest_locks() {
    let tree = ps(&[(11, 10), (12, 11), (21, 20)]);
    assert_eq!(pick_copilot_session(&[lock("mine", 12), lock("other", 21)], &tree, &p(10, None)).as_deref(), Some("/home/u/.copilot/session-state/mine/events.jsonl"));
    assert_eq!(pick_copilot_session(&[lock("near", 11), lock("far", 12)], &tree, &p(10, None)).as_deref(), Some("/home/u/.copilot/session-state/near/events.jsonl"));
    assert_eq!(pick_copilot_session(&[lock("a", 12), lock("b", 12)], &tree, &p(10, None)), None);
    assert_eq!(pick_copilot_session(&[lock("a", 12), lock("b", 12)], &tree, &p(10, Some("b"))).as_deref(), Some("/home/u/.copilot/session-state/b/events.jsonl"));
}

/// Copilot descendant search is bounded and never follows unrelated processes
#[test]
fn copilot_descendant_search_is_bounded_and_never_follows_unrelated_processes() {
    let tree: Vec<PsRow> = (0..5).map(|i| PsRow { pid: i + 2, ppid: i + 1 }).collect();
    assert_eq!(pick_copilot_session(&["/h/session/four/inuse.5.lock".into()], &tree, &p(1, None)).as_deref(), Some("/h/session/four/events.jsonl"));
    assert_eq!(pick_copilot_session(&["/h/session/five/inuse.6.lock".into()], &tree, &p(1, None)), None);
    assert_eq!(pick_copilot_session(&["/h/session/other/inuse.99.lock".into()], &tree, &p(1, None)), None);
}

#[test]
fn the_process_tree_is_read_from_ps() {
    assert_eq!(parse_ps_tree("  10     1\n 11 10 \nPID PPID\n"), ps(&[(10, 1), (11, 10)]));
    assert!(copilot_locks_command("/home/u").starts_with("find '/home/u/.copilot/session-state' "));
}
