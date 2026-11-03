//! agents/antigravity.test.ts, test for test. Each test's name in the TS file is its doc comment.

use super::*;
use crate::claude::transcript::new_meta;
use crate::test_support::assert_match;
use serde_json::json;

fn kinds(items: &[ChatItem]) -> Vec<String> {
    serde_json::to_value(items).unwrap().as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap().to_owned()).collect()
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// describe('AntigravityDoc')
mod antigravity_doc {
    use super::*;

    /// Rows shaped like agy 1.3.3's transcript.jsonl (argument values are JSON-encoded strings)
    fn rows() -> Vec<Value> {
        vec![
            json!({
                "step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE", "created_at": "2026-10-10T08:29:09Z",
                "content": "<USER_REQUEST>\nRun `date +%Y` and tell me the output.\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-10-10T13:59:09+05:30.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to Gemini 3.8 Flash (High). No need to comment on this change.\n</USER_SETTINGS_CHANGE>",
            }),
            json!({
                "step_index": 1, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-10-10T08:29:09Z", "input_tokens": 11905, "cache_read_tokens": 0, "output_tokens": 231,
                "thinking": "Run it.",
                "tool_calls": [{ "name": "run_command", "args": { "CommandLine": "\"date +%Y\"", "Cwd": "\"/w\"", "WaitMsBeforeAsync": "5000", "toolAction": "\"Running date\"", "toolSummary": "\"Run date command\"" } }],
            }),
            json!({ "step_index": 2, "source": "MODEL", "type": "GENERIC", "status": "DONE", "created_at": "2026-10-10T08:29:14Z", "content": "Created At: 2026-10-10T13:59:14+05:30\nCompleted At: 2026-10-10T13:59:27+05:30\n\nThe command exited with code 0.\nOutput:\n2026\r\n\n" }),
            json!({ "step_index": 3, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-10-10T08:29:27Z", "input_tokens": 12226, "content": "It is 2026." }),
            json!({ "step_index": 4, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE", "created_at": "2026-10-10T08:30:00Z", "content": "<USER_REQUEST>\nRun false\n</USER_REQUEST>" }),
            json!({ "step_index": 5, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-10-10T08:30:01Z", "input_tokens": 12400, "tool_calls": [{ "name": "run_command", "args": { "CommandLine": "\"false\"" } }] }),
            json!({ "step_index": 6, "source": "SYSTEM", "type": "SYSTEM_MESSAGE", "status": "DONE", "created_at": "2026-10-10T08:30:02Z", "content": "<SYSTEM_MESSAGE>server restart</SYSTEM_MESSAGE>" }),
            json!({ "step_index": 7, "source": "MODEL", "type": "GENERIC", "status": "DONE", "created_at": "2026-10-10T08:30:03Z", "content": "Created At: x\nCompleted At: y\n\nThe command exited with code 1.\nOutput:\n" }),
        ]
    }

    /// turns a transcript into chat items
    #[test]
    fn turns_a_transcript_into_chat_items() {
        let mut doc = AntigravityDoc::new();
        for r in rows() {
            doc.apply(&r);
        }
        let mut meta = new_meta();
        let items = doc.items(&mut meta);
        assert_eq!(kinds(&items), ["user", "assistant", "assistant", "tool_result", "assistant", "user", "assistant", "tool_result"]);
        assert_match(&items[0], json!({ "kind": "user", "text": "Run `date +%Y` and tell me the output." }));
        assert_match(&items[1], json!({ "block": { "type": "thinking", "text": "Run it." } }));
        assert_match(
            &items[2],
            json!({ "block": { "type": "tool_use", "id": "s1:0", "name": "Bash", "input": { "command": "date +%Y", "description": "Run date command", "cwd": "/w" } } }),
        );
        assert_match(&items[3], json!({ "toolUseId": "s1:0", "isError": false, "text": "The command exited with code 0.\nOutput:\n2026", "meta": { "stdout": "2026" } }));
        // the system notice in between doesn't take the result's place
        assert_match(&items[7], json!({ "toolUseId": "s5:0", "isError": true }));
        let seqs: Vec<u64> = serde_json::to_value(&items).unwrap().as_array().unwrap().iter().map(|i| i["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(meta.model.as_deref(), Some("Gemini 3.8 Flash (High)"));
        assert_eq!(meta.stats.context_tokens, Some(Num(12400.0)));
        assert_eq!(meta.stats.turns, 1);
        assert_eq!(meta.stats.started_at.as_deref(), Some("2026-10-10T08:29:09.000Z"));
    }

    /// a step written again replaces the earlier copy
    #[test]
    fn a_step_written_again_replaces_the_earlier_copy() {
        let mut doc = AntigravityDoc::new();
        doc.apply(&json!({ "step_index": 0, "type": "PLANNER_RESPONSE", "content": "Draft" }));
        doc.apply(&json!({ "step_index": 0, "type": "PLANNER_RESPONSE", "content": "Final" }));
        assert_match(&doc.items(&mut new_meta()), json!([{ "block": { "type": "text", "text": "Final" } }]));
    }

    /// /goal opens a goal and the GOAL_COMPLETE marker closes it
    #[test]
    fn goal_opens_a_goal_and_the_goal_complete_marker_closes_it() {
        let mut doc = AntigravityDoc::new();
        doc.apply(&json!({ "step_index": 0, "type": "USER_INPUT", "created_at": "t0", "content": "<USER_REQUEST>\n/goal Reply with ready\n</USER_REQUEST>\n<ADDITIONAL_METADATA>x</ADDITIONAL_METADATA>" }));
        let mut meta = new_meta();
        assert_eq!(kinds(&doc.items(&mut meta)), ["goal"]);
        assert_match(&meta.goal, json!({ "condition": "Reply with ready", "met": false }));
        doc.apply(&json!({ "step_index": 1, "type": "PLANNER_RESPONSE", "created_at": "t1", "input_tokens": 10, "content": "ready\n<!-- GOAL_COMPLETE -->" }));
        let items = doc.items(&mut meta);
        assert_eq!(kinds(&items), ["goal", "assistant", "goal"]);
        assert_eq!(serde_json::to_value(&items[1]).unwrap()["block"]["text"], "ready");
        assert_eq!(meta.goal.map(|g| g.met), Some(true));
    }

    /// a backgrounded command becomes a task, finished by the system message
    #[test]
    fn a_backgrounded_command_becomes_a_task_finished_by_the_system_message() {
        let mut doc = AntigravityDoc::new();
        doc.apply(&json!({ "step_index": 0, "type": "PLANNER_RESPONSE", "created_at": "t0", "tool_calls": [{ "name": "run_command", "args": { "CommandLine": "\"sleep 20; echo hi\"", "toolSummary": "\"Sleep and echo\"" } }] }));
        doc.apply(&json!({ "step_index": 1, "type": "GENERIC", "status": "RUNNING", "created_at": "t1", "content": "Created At: x\nTool is running as a background task with task id: conv/task-2\nTask Description: sleep 20; echo hi\nTask logs are available at: file:///h/.gemini/antigravity-cli/brain/conv/.system_generated/tasks/task-2.log\nYOU MUST…" }));
        let mut meta = new_meta();
        doc.items(&mut meta);
        assert_match(
            &meta.tasks.get("conv/task-2"),
            json!({ "status": "running", "description": "Sleep and echo", "command": "sleep 20; echo hi", "outputFile": "/h/.gemini/antigravity-cli/brain/conv/.system_generated/tasks/task-2.log" }),
        );
        doc.apply(&json!({ "step_index": 2, "source": "SYSTEM", "type": "SYSTEM_MESSAGE", "created_at": "t2", "content": "<SYSTEM_MESSAGE>\n[Message] content=Task id \"conv/task-2\" finished with result:\n\nThe command exited with code 0.\nOutput:\nhi\r\n\n\nLog: file:///x.log\n</SYSTEM_MESSAGE>" }));
        let items = doc.items(&mut meta);
        assert_match(&meta.tasks.get("conv/task-2"), json!({ "status": "completed", "summary": "The command exited with code 0.\nOutput:\nhi" }));
        assert_match(items.last().unwrap(), json!({ "kind": "notice", "subtype": "task" }));
    }

    /// tool names map to the rows Inyon knows
    #[test]
    fn tool_names_map_to_the_rows_inyon_knows() {
        assert_eq!(map_agy_tool("view_file", &args(json!({ "AbsolutePath": "\"/a.ts\"" }))), ToolCall { name: "Read".into(), input: json!({ "file_path": "/a.ts" }) });
        assert_eq!(
            map_agy_tool("write_to_file", &args(json!({ "TargetFile": "\"/b\"", "CodeContent": "\"hi\\n\"", "Overwrite": "true" }))).input,
            json!({ "file_path": "/b", "content": "hi\n" })
        );
        assert_match(
            &map_agy_tool("replace_file_content", &args(json!({ "TargetFile": "\"/c\"", "TargetContent": "\"a\"", "ReplacementContent": "\"b\"" }))).input,
            json!({ "old_string": "a", "new_string": "b" }),
        );
        assert_eq!(map_agy_tool("replace_file_content", &Map::new()).name, "Edit");
        assert_eq!(map_agy_tool("mystery_tool", &args(json!({ "Foo": "1", "toolAction": "\"x\"" }))), ToolCall { name: "mystery_tool".into(), input: json!({ "Foo": 1 }) });
        assert_eq!(user_text("plain text"), "plain text");
    }
}

/// describe('agyStatus')
mod agy_status_tests {
    use super::*;

    fn panes(ts: f64, data: Value) -> IndexMap<String, AgyStatusPane> {
        IndexMap::from([("%7".to_owned(), AgyStatusPane { ts, data })])
    }

    /// current CLI cycle_mode beats legacy execution_mode, including returning to default
    #[test]
    fn current_cli_cycle_mode_beats_legacy_execution_mode_including_returning_to_default() {
        let mut pr = panes(1000.0, json!({ "cycle_mode": "default", "execution_mode": "PLAN" }));
        assert_eq!(agy_status(&pr, Some(true), "%7", 990.0, None).unwrap().mode.as_deref(), Some("default"));
        pr.get_mut("%7").unwrap().data["cycle_mode"] = json!("accept-edits");
        assert_eq!(agy_status(&pr, Some(true), "%7", 990.0, None).unwrap().mode.as_deref(), Some("accept-edits"));
        assert_eq!(agy_status(&pr, Some(false), "%7", 990.0, None), None);
    }

    fn payload() -> Value {
        json!({
            "conversation_id": "c1", "conversation_title": "Fix the parser", "version": "1.3.3", "agent_state": "idle", "plan_tier": "Google AI Pro",
            "model": { "id": "gemini-3.8-flash-high", "display_name": "Gemini 3.8 Flash (High)", "effort": "high" },
            "context_window": { "total_input_tokens": 15077, "total_output_tokens": 55, "context_window_size": 1048576, "used_percentage": 1.4 },
            "quota": {
                "gemini-5h": { "remaining_fraction": 0.75, "reset_time": "2026-10-10T12:00:00Z" },
                "gemini-weekly": { "remaining_fraction": 0.9, "reset_time": "2026-10-14T00:00:00Z" },
                "3p-5h": { "remaining_fraction": 0.1 },
            },
            "pending_input_count": 1, "tool_confirmation_pending": false, "execution_mode": "ACCEPT_EDITS",
        })
    }

    /// maps the status line payload
    #[test]
    fn maps_the_status_line_payload() {
        let st = agy_status(&panes(1000.0, payload()), None, "%7", 990.0, Some("c1")).unwrap();
        assert_match(
            &st,
            json!({ "model": "Gemini 3.8 Flash", "modelId": "gemini-3.8-flash-high", "effort": "high", "contextPct": 1.4, "contextUsed": 15077, "contextSize": 1048576, "plan": "Google AI Pro", "title": "Fix the parser", "queued": 1, "mode": "accept-edits" }),
        );
        let five = st.status.five_hour.unwrap();
        assert!((five.pct.0 - 25.0).abs() < 0.005);
        assert_eq!(five.resets_at, Some(Num(date::parse("2026-10-10T12:00:00Z") / 1000.0)));
        assert!((st.status.seven_day.unwrap().pct.0 - 10.0).abs() < 0.005);
    }

    /// ignores a payload from before the process started, or for another conversation
    #[test]
    fn ignores_a_payload_from_before_the_process_started_or_for_another_conversation() {
        assert_eq!(agy_status(&panes(900.0, payload()), None, "%7", 990.0, Some("c1")), None);
        assert_eq!(agy_status(&panes(1000.0, payload()), None, "%7", 990.0, Some("other")), None);
    }
}

#[test]
fn tags_without_a_request_are_dropped_and_an_unclosed_one_stays() {
    assert_eq!(user_text("<A>x</A>hi <B>y</B>"), "hi");
    assert_eq!(user_text("<OPEN>left <C>z</C>there"), "<OPEN>left there");
    assert_eq!(agy_model_name(Some("Claude Sonnet 4.6 (Thinking)")).as_deref(), Some("Claude Sonnet 4.6 (Thinking)"));
}

/// review: steps keyed by the number itself, as the TS's Map is: 3.5 stays apart from 3, -0 is 0
#[test]
fn fractional_and_huge_step_indexes_keep_their_own_place() {
    let mut doc = AntigravityDoc::new();
    for (i, text) in [(json!(1e300), "huge"), (json!(3.5), "half"), (json!(3), "three"), (json!(-0.0), "minus zero"), (json!(0), "zero")] {
        doc.apply(&json!({ "step_index": i, "type": "PLANNER_RESPONSE", "content": text }));
    }
    let items = doc.items(&mut new_meta());
    assert_match(
        &items,
        json!([
            { "msgId": "s0", "block": { "text": "zero" } },
            { "msgId": "s3", "block": { "text": "three" } },
            { "msgId": "s3.5", "block": { "text": "half" } },
            { "msgId": "s1e+300", "block": { "text": "huge" } },
        ]),
    );
}

/// review: an empty conversation id checks nothing, as in the TS
#[test]
fn an_empty_conversation_id_checks_nothing() {
    let pane = IndexMap::from([("%7".to_owned(), AgyStatusPane { ts: 1000.0, data: json!({ "conversation_id": "c1" }) })]);
    assert!(agy_status(&pane, None, "%7", 990.0, Some("")).is_some());
    assert!(agy_status(&pane, None, "%7", 990.0, Some("c2")).is_none());
}
