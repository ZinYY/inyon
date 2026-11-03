//! agents/codex-live.test.ts, test for test. Each test's name in the TS file is its doc comment.
//!
//! Lines as the helper on the machine prints them, from a Codex 0.162.1 TUI thread joined through
//! its app-server daemon (0.163.0) while it answered "say hi again" and ran a sub-agent.

use super::*;
use crate::js::json::stringify;
use crate::test_support::assert_match;
use serde_json::json;

const T: &str = "01a14ac8-3abe-7402-9e85-7440d1fce2e7";
const TURN: &str = "01a14ace-d21b-7a50-a37f-66449ab41d18";
const MSG: &str = "msg_05b5d60b8898bebc016ad3a9afaa7c81919a0a99b324450b29";

fn line(m: &str, p: Value) -> String {
    stringify(&json!({ "m": m, "p": p }))
}

fn delta_with(item_id: &str, delta: &str, method: &str, more: Value) -> String {
    let mut p = json!({ "threadId": T, "turnId": TURN, "itemId": item_id, "delta": delta });
    for (k, v) in more.as_object().unwrap() {
        p[k] = v.clone();
    }
    line(method, p)
}

fn delta(item_id: &str, delta: &str) -> String {
    delta_with(item_id, delta, "item/agentMessage/delta", json!({}))
}

fn change(v: Value) -> LiveChange {
    let v = Some(&v);
    let flag = |k: &str| v.at(k).and_then(Value::as_bool);
    LiveChange {
        live: flag("live"),
        turn: flag("turn"),
        status: flag("status"),
        usage: flag("usage"),
        name: v.at("name").str().map(str::to_owned),
        queue: flag("queue"),
        ask: flag("ask"),
        agents: flag("agents"),
    }
}

fn live(id: &str, kind: LiveKind, text: &str) -> LiveItem {
    LiveItem { id: id.into(), kind, text: text.into(), turn_id: Some(TURN.into()) }
}

/// describe('Codex live state')
mod codex_live_state {
    use super::*;

    /// a reply streams in by deltas, is finished by its item, and leaves once its record is in
    #[test]
    fn a_reply_streams_in_by_deltas_is_finished_by_its_item_and_leaves_once_its_record_is_in() {
        let mut s = CodexLiveState::new();
        assert_eq!(
            s.apply(&line("turn/started", json!({ "threadId": T, "turn": { "id": TURN, "startedAt": 1792256430u64, "status": "inProgress" } }))),
            change(json!({ "turn": true }))
        );
        assert_eq!(s.turn, Some(Turn { id: Some(TURN.into()), started_at: inyon_protocol::Num(1792256430000.0) }));
        assert_eq!(
            s.apply(&line(
                "item/started",
                json!({ "item": { "type": "agentMessage", "id": MSG, "text": "", "phase": "final_answer" }, "threadId": T, "turnId": TURN, "startedAtMs": 1 })
            )),
            LiveChange::default()
        );
        assert_eq!(s.apply(&delta(MSG, "Hi")), change(json!({ "live": true })));
        s.apply(&delta(MSG, " again"));
        s.apply(&delta(MSG, "!"));
        assert_eq!(s.list(), vec![live(MSG, LiveKind::Text, "Hi again!")]);
        // the finished item carries the whole text, in case a delta was lost on the way
        s.apply(&line("item/completed", json!({ "item": { "type": "agentMessage", "id": MSG, "text": "Hi again!", "phase": "final_answer" }, "threadId": T, "turnId": TURN, "completedAtMs": 1792256431865u64 })));
        assert_eq!(s.list()[0].text, "Hi again!");
        assert_eq!(s.done.get(MSG), Some(&1792256431865.0));
        assert_eq!(
            s.apply(&line("turn/completed", json!({ "threadId": T, "turn": { "id": TURN, "status": "completed", "durationMs": 1239 } }))),
            change(json!({ "turn": true, "live": true }))
        );
        assert_eq!(s.turn, None);
        // the rollout line for the same message id arrives: the live copy goes
        assert!(s.settle(&[Some(MSG)]));
        assert_eq!(s.list(), vec![]);
    }

    /// thinking summaries and a plan stream as their own kinds, paragraphs apart
    #[test]
    fn thinking_summaries_and_a_plan_stream_as_their_own_kinds_paragraphs_apart() {
        let mut s = CodexLiveState::new();
        s.apply(&delta_with("rs_1", "Looking at", "item/reasoning/summaryTextDelta", json!({ "summaryIndex": 0 })));
        s.apply(&delta_with("rs_1", " the folder", "item/reasoning/summaryTextDelta", json!({ "summaryIndex": 0 })));
        s.apply(&delta_with("rs_1", "Then the plan", "item/reasoning/summaryTextDelta", json!({ "summaryIndex": 1 })));
        s.apply(&delta_with("plan_1", "## Steps", "item/plan/delta", json!({})));
        assert_eq!(s.list(), vec![live("rs_1", LiveKind::Thinking, "Looking at the folder\n\nThen the plan"), live("plan_1", LiveKind::Plan, "## Steps")]);
    }

    /// a command's output keeps only its end, and a finished item nobody records expires
    #[test]
    fn a_commands_output_keeps_only_its_end_and_a_finished_item_nobody_records_expires() {
        let mut s = CodexLiveState::new();
        s.apply(&delta_with("exec-1", &"x".repeat(5000), "item/commandExecution/outputDelta", json!({})));
        s.apply(&delta_with("exec-1", "tail", "item/commandExecution/outputDelta", json!({})));
        assert_eq!(s.list()[0].text.len(), 4000);
        assert!(s.list()[0].text.ends_with("tail"));
        s.apply(&line(
            "item/completed",
            json!({ "item": { "type": "commandExecution", "id": "exec-1", "status": "completed" }, "threadId": T, "turnId": TURN, "completedAtMs": 1000 }),
        ));
        assert!(!s.expire(6000.0, 6999.0));
        assert!(s.expire(6000.0, 7001.0));
        assert_eq!(s.list(), vec![]);
    }

    /// status, usage, name and queue changes are reported, with the waiting flags
    #[test]
    fn status_usage_name_and_queue_changes_are_reported_with_the_waiting_flags() {
        let mut s = CodexLiveState::new();
        assert_eq!(
            s.apply(&line("thread/status/changed", json!({ "threadId": T, "status": { "type": "active", "activeFlags": ["waitingOnApproval"] } }))),
            change(json!({ "status": true }))
        );
        assert_eq!(s.status.as_deref(), Some("active"));
        assert_eq!(s.flags, ["waitingOnApproval"]);
        assert_eq!(
            s.apply(&line("thread/tokenUsage/updated", json!({ "threadId": T, "turnId": TURN, "tokenUsage": { "total": { "totalTokens": 1 } } }))),
            change(json!({ "usage": true }))
        );
        assert_eq!(s.apply(&line("account/rateLimits/updated", json!({ "rateLimits": {} }))), change(json!({ "usage": true })));
        assert_eq!(s.apply(&line("thread/name/updated", json!({ "threadId": T, "threadName": "Say hi" }))), change(json!({ "name": "Say hi" })));
        assert_eq!(s.apply(&line("thread/queue/changed", json!({ "threadId": T }))), change(json!({ "queue": true })));
        assert_eq!(
            s.apply(&stringify(&json!({ "m": "item/commandExecution/requestApproval", "p": { "threadId": T, "itemId": "exec-9" }, "req": true }))),
            change(json!({ "ask": true }))
        );
        assert_eq!(s.apply(&line("serverRequest/resolved", json!({ "threadId": T, "requestId": 0 }))), change(json!({ "ask": true })));
        assert_eq!(
            s.apply(&stringify(&json!({ "e": "resumed", "model": "gpt-6-luna", "effort": null, "tier": "default", "status": { "type": "idle" } }))),
            change(json!({ "status": true }))
        );
        assert_eq!(s.settings, Some(LiveSettings { model: Some("gpt-6-luna".into()), effort: None, tier: Some("default".into()) }));
        assert_eq!(s.status.as_deref(), Some("idle"));
        assert_eq!(s.apply("not json"), LiveChange::default());
        assert_eq!(s.apply(&stringify(&json!({ "e": "waiting", "error": "no rollout found" }))), LiveChange::default());
    }

    /// a sub-agent is listed from its activity and its own thread, and finishes with it
    #[test]
    fn a_sub_agent_is_listed_from_its_activity_and_its_own_thread_and_finishes_with_it() {
        let mut s = CodexLiveState::new();
        let child = "01a14ae4-8cfb-7252-9639-34db58c5b1c9";
        assert_eq!(
            s.apply(&line("thread/started", json!({ "thread": { "id": child, "parentThreadId": T, "preview": "", "model": "gpt-6-luna", "createdAt": 1792257854u64 } }))),
            change(json!({ "agents": true }))
        );
        s.apply(&line("item/started", json!({ "item": { "type": "subAgentActivity", "model": "gpt-6-luna", "id": "call_2d3", "kind": "started", "agentThreadId": child, "agentPath": "/root/pong_only" }, "threadId": T, "turnId": TURN, "startedAtMs": 1792257854760u64 })));
        assert_match(&s.agents.values().collect::<Vec<_>>(), json!([{ "threadId": child, "description": "Sub-agent", "status": "running", "model": "gpt-6-luna" }]));
        // its own status changes don't touch the main thread's
        s.apply(&line("thread/status/changed", json!({ "threadId": T, "status": { "type": "active", "activeFlags": [] } })));
        assert_eq!(s.apply(&line("thread/status/changed", json!({ "threadId": child, "status": { "type": "idle" } }))), change(json!({ "agents": true })));
        assert_eq!(s.status.as_deref(), Some("active"));
        assert_eq!(s.agents[child].status, SubagentStatus::Done);
        // a thread of someone else's isn't a sub-agent
        assert_eq!(s.apply(&line("thread/started", json!({ "thread": { "id": "other", "parentThreadId": null } }))), LiveChange::default());
    }

    /// a sub-agent whose start time isn't a usable number starts now
    #[test]
    fn a_sub_agent_whose_start_time_isnt_a_usable_number_starts_now() {
        let mut s = CodexLiveState::new();
        let before = now_ms() as f64;
        s.apply(&line("thread/started", json!({ "thread": { "id": "sub1", "parentThreadId": T, "preview": "Look", "createdAt": "soon" } })));
        s.apply(&line("thread/started", json!({ "thread": { "id": "sub2", "parentThreadId": T, "createdAt": 1e20 } })));
        s.apply(&line("item/started", json!({ "item": { "type": "subAgentActivity", "kind": "started", "agentThreadId": "sub3" }, "startedAtMs": "later" })));
        s.apply(&line("thread/started", json!({ "thread": { "id": "sub4", "parentThreadId": T, "createdAt": 1792257854u64 } })));
        for id in ["sub1", "sub2", "sub3"] {
            assert!(date::parse(&s.agents[id].started_at) >= before, "{id}: {:?}", s.agents[id].started_at);
        }
        assert_eq!(s.agents["sub4"].started_at, "2026-10-17T17:24:14.000Z");
    }

    /// a turn that ends without an id ends no items, not the ones without a turn
    #[test]
    fn a_turn_that_ends_without_an_id_ends_no_items_not_the_ones_without_a_turn() {
        let mut s = CodexLiveState::new();
        s.apply(&line("item/agentMessage/delta", json!({ "threadId": T, "itemId": "a", "delta": "no turn" })));
        s.apply(&delta("b", "in the turn"));
        assert_eq!(s.apply(&line("turn/completed", json!({ "threadId": T, "turn": { "status": "completed" } }))), change(json!({ "turn": true, "live": true })));
        assert!(!s.expire(1.0, 9e15));
        assert_eq!(s.list().iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        s.apply(&line("turn/completed", json!({ "threadId": T, "turn": { "id": TURN, "status": "completed" } })));
        assert!(s.expire(1.0, 9e15));
        assert_eq!(s.list().iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a"]);
    }
}

/// the helper is run with node on the machine, for the thread the rollout file names
#[test]
fn the_helper_is_run_with_node_on_the_machine_for_the_thread_the_rollout_file_names() {
    assert_eq!(
        codex_thread_id("/home/me/.codex/sessions/2026/10/17/rollout-2026-10-17T16-53-18-01a14ac8-3abe-7402-9e85-7440d1fce2e7.jsonl"),
        "01a14ac8-3abe-7402-9e85-7440d1fce2e7"
    );
    assert_eq!(
        codex_thread_id("/x/rollout-2026-10-10T16-56-05-01a1255b-fbc5-7371-ab05-ee0aa808a95d_01a12590-2495-7421-bf23-e68d82d406bc.jsonl"),
        "01a1255b-fbc5-7371-ab05-ee0aa808a95d"
    );
    let cmd = codex_live_command("01a14ac8-3abe-7402-9e85-7440d1fce2e7");
    assert!(cmd.contains("node -e '"));
    assert!(cmd.ends_with("' '01a14ac8-3abe-7402-9e85-7440d1fce2e7'"));
    // the script speaks WebSocket to the daemon's socket itself and joins the thread, nothing else
    assert!(CODEX_LIVE_SCRIPT.contains("Sec-WebSocket-Key"));
    assert!(CODEX_LIVE_SCRIPT.contains("'thread/resume'"));
    assert!(!CODEX_LIVE_SCRIPT.contains("'turn/start'"));
    assert!(!CODEX_LIVE_SCRIPT.contains("'turn/steer'"));
}

/// review: a fractional turn start stays fractional on the wire
#[test]
fn a_fractional_turn_start_stays_fractional() {
    let mut s = CodexLiveState::new();
    s.apply(&line("turn/started", json!({ "threadId": T, "turn": { "id": TURN, "startedAt": 1792256430.1234 } })));
    assert_eq!(s.turn.unwrap().started_at, inyon_protocol::Num(1792256430.1234 * 1000.0));
}

/// review: summary indexes compare as `!==` does: 1 and 1.0 are the same paragraph
#[test]
fn a_summary_index_written_as_1_0_is_the_same_paragraph_as_1() {
    let mut s = CodexLiveState::new();
    let raw = |delta: &str, index: &str| {
        format!(r#"{{"m":"item/reasoning/summaryTextDelta","p":{{"threadId":"{T}","turnId":"{TURN}","itemId":"rs_1","delta":"{delta}","summaryIndex":{index}}}}}"#)
    };
    s.apply(&raw("One", "1"));
    s.apply(&raw(" two", "1.0"));
    s.apply(&raw("Three", "2"));
    assert_eq!(s.list(), vec![live("rs_1", LiveKind::Thinking, "One two\n\nThree")]);
}

/// review: an item/completed for an id that never streamed changes nothing, and a turn/completed
/// without a turn id ends no items, not even the ones without a turn; compared with the TS
/// CodexLiveState's output for the same lines (from bun)
#[test]
fn a_completion_for_an_unknown_item_and_a_turn_end_without_an_id_go_as_in_the_ts() {
    let mut s = CodexLiveState::new();
    let l = |m: &str, p: Value| stringify(&json!({ "m": m, "p": p }));
    let mut out: Vec<Value> = vec![];
    out.push(serde_json::to_value(s.apply(&l("item/agentMessage/delta", json!({ "threadId": "T", "itemId": "a", "delta": "no turn" })))).unwrap());
    out.push(serde_json::to_value(s.apply(&l("item/agentMessage/delta", json!({ "threadId": "T", "turnId": "t1", "itemId": "b", "delta": "in t1" })))).unwrap());
    out.push(
        serde_json::to_value(
            s.apply(&l("item/completed", json!({ "threadId": "T", "item": { "type": "agentMessage", "id": "ghost", "text": "never streamed" }, "completedAtMs": 5 }))),
        )
        .unwrap(),
    );
    out.push(serde_json::to_value(s.list()).unwrap());
    out.push(serde_json::to_value(s.apply(&l("turn/completed", json!({ "threadId": "T", "turn": { "status": "completed" } })))).unwrap());
    out.push(json!(s.expire(1.0, 9e15)));
    out.push(serde_json::to_value(s.list()).unwrap());
    let ts = r#"[{"live":true},{"live":true},{},[{"id":"a","kind":"text","text":"no turn"},{"id":"b","kind":"text","text":"in t1","turnId":"t1"}],{"turn":true,"live":true},false,[{"id":"a","kind":"text","text":"no turn"},{"id":"b","kind":"text","text":"in t1","turnId":"t1"}]]"#;
    assert_eq!(Value::Array(out), serde_json::from_str::<Value>(ts).unwrap());
}
