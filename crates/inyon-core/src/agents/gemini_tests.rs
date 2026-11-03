//! agents/gemini.test.ts has one test, the one before last here. The others check the reader against
//! what the TS made of the same records (printed with bun from agents/gemini.ts), and the session
//! picker; the last is the Gemini half of agents/discovery.test.ts.

use super::*;
use crate::claude::transcript::new_meta;
use serde_json::json;

/// Records covering each kind of edit; `ITEMS` is the TS GeminiDoc's output for them.
const RECORDS: &str = r#"[{"sessionId": "s1", "projectHash": "h", "startTime": "2026-10-10T08:00:00Z", "kind": "main"}, {"id": "u1", "type": "user", "timestamp": "2026-10-10T08:00:01Z", "content": [{"text": "List the files"}, {"text": " please"}], "displayContent": [{"text": "List the files please"}]}, {"id": "g1", "type": "gemini", "timestamp": "2026-10-10T08:00:02Z", "model": "gemini-3-pro", "content": "", "thoughts": [{"subject": "Looking", "description": "at the folder"}, {"subject": "", "description": ""}], "tokens": {"input": 1200, "output": 40, "cached": 300}, "toolCalls": [{"id": "c1", "name": "run_shell_command", "args": {"command": "ls", "description": "List"}, "status": "executing"}, {"id": "c2", "name": "read_many_files", "args": {"paths": ["a.ts", "b.ts"]}}, {"id": "c3", "name": "write_todos", "args": {"todos": [{"description": "one", "status": "in_progress"}, {"content": "two", "status": "x"}]}}, {"id": "c4", "name": "mystery", "args": null}]}, {"$patch": {"id": "g1", "content": "Here they are", "toolCalls": [{"id": "c1", "result": [{"functionResponse": {"response": {"output": "a.ts\nb.ts"}}}]}, {"id": "c2", "result": [{"text": "contents"}]}]}}, {"$patch": {"updates": [{"id": "u1", "content": "ignored since displayContent"}], "orderIds": ["u1", "g1", "nope"]}}, {"id": "w1", "type": "warning", "content": "Quota low"}, {"id": "e1", "type": "error", "content": "  "}, {"id": "u2", "type": "user", "content": "gone soon"}, {"$rewindTo": "u2"}, {"$set": {"summary": "Listing files"}}]"#;
const ITEMS: &str = r#"[{"kind": "user", "uuid": "u1", "ts": "2026-10-10T08:00:01Z", "text": "List the files please", "images": 0, "seq": 0}, {"kind": "assistant", "uuid": "g1:t0", "ts": "2026-10-10T08:00:02Z", "msgId": "g1", "block": {"type": "thinking", "text": "Looking: at the folder"}, "seq": 1}, {"kind": "assistant", "uuid": "g1:x", "ts": "2026-10-10T08:00:02Z", "msgId": "g1", "model": "gemini-3-pro", "block": {"type": "text", "text": "Here they are"}, "seq": 2}, {"kind": "assistant", "uuid": "g1:c1", "ts": "2026-10-10T08:00:02Z", "msgId": "g1", "block": {"type": "tool_use", "id": "c1", "name": "Bash", "input": {"command": "ls", "description": "List"}}, "seq": 3}, {"kind": "tool_result", "uuid": "g1:c1:r", "ts": "2026-10-10T08:00:02Z", "toolUseId": "c1", "isError": false, "text": "a.ts\nb.ts", "meta": {"stdout": ""}, "seq": 4}, {"kind": "assistant", "uuid": "g1:c2", "ts": "2026-10-10T08:00:02Z", "msgId": "g1", "block": {"type": "tool_use", "id": "c2", "name": "Read", "input": {"file_path": "a.ts, b.ts"}}, "seq": 5}, {"kind": "tool_result", "uuid": "g1:c2:r", "ts": "2026-10-10T08:00:02Z", "toolUseId": "c2", "isError": false, "text": "contents", "seq": 6}, {"kind": "assistant", "uuid": "g1:c3", "ts": "2026-10-10T08:00:02Z", "msgId": "g1", "block": {"type": "tool_use", "id": "c3", "name": "TodoWrite", "input": {"todos": [{"content": "one", "activeForm": "one", "status": "in_progress"}, {"content": "two", "status": "pending"}]}}, "seq": 7}, {"kind": "assistant", "uuid": "g1:c4", "ts": "2026-10-10T08:00:02Z", "msgId": "g1", "block": {"type": "tool_use", "id": "c4", "name": "mystery", "input": {}}, "seq": 8}, {"kind": "notice", "uuid": "w1", "ts": "", "level": "warn", "subtype": "info", "text": "Quota low", "seq": 9}]"#;

#[test]
fn a_session_reads_as_the_ts_reads_it() {
    let mut doc = GeminiDoc::new();
    for r in serde_json::from_str::<Vec<Value>>(RECORDS).unwrap() {
        doc.apply(&r);
    }
    let mut meta = new_meta();
    let items = doc.items(&mut meta);
    assert_eq!(serde_json::to_value(&items).unwrap(), serde_json::from_str::<Value>(ITEMS).unwrap());
    assert_eq!(meta.title.as_deref(), Some("Listing files"));
    assert_eq!(meta.model.as_deref(), Some("gemini-3-pro"));
    assert_eq!(meta.stats.turns, 1);
    assert_eq!(meta.stats.context_tokens, Some(Num(1500.0)));
    assert_eq!(meta.status.as_ref().and_then(|s| s.context_used), Some(Num(1500.0)));
}

#[test]
fn the_session_is_the_newest_one_for_the_folder_since_the_process_started() {
    let hash = sha256_hex("/w/app");
    let line = |mtime: u64, path: &str, first: Value| format!("{mtime}\t{path}\t{first}");
    let out = [
        line(2000, "/h/.gemini/tmp/x/chats/session-sub.jsonl", json!({ "projectHash": hash, "kind": "subagent" })),
        line(1500, "/h/.gemini/tmp/x/chats/session-old.jsonl", json!({ "projectHash": hash })),
        line(1900, "/h/.gemini/tmp/y/chats/session-other.jsonl", json!({ "projectHash": "nope" })),
        line(1800, "/h/.gemini/tmp/app/chats/session-named.jsonl", json!("not an object")),
        "garbage".to_owned(),
    ]
    .join("\n");
    let proc = GeminiProc { cwd: Some("/w/app".into()), started_at: 1000.0 };
    // the sub-agent's file is skipped; the folder name in the path counts as a match
    assert_eq!(guess_gemini_session(&out, &proc).as_deref(), Some("/h/.gemini/tmp/app/chats/session-named.jsonl"));
    assert_eq!(guess_gemini_session(&out, &GeminiProc { started_at: 1850.0, ..proc.clone() }), None);
    assert_eq!(guess_gemini_session(&out, &GeminiProc { cwd: None, started_at: 1000.0 }).as_deref(), Some("/h/.gemini/tmp/y/chats/session-other.jsonl"));
    assert!(guess_gemini_session_command("/h", &proc, 1000.0 + 600.0).contains("-mmin -12 "));
}

/// review: `$set.messages`, `removeIds` and a `$rewindTo` naming no message, against the TS GeminiDoc's
/// output for the same records (from bun)
#[test]
fn set_messages_remove_ids_and_a_rewind_to_nothing_read_as_the_ts_reads_them() {
    const RECORDS: &str = r#"[{"id":"old","type":"user","timestamp":"2026-10-10T07:00:00Z","content":"replaced by $set"},{"$set":{"messages":[{"id":"u1","type":"user","timestamp":"2026-10-10T08:00:01Z","content":"first"},{"id":"g1","type":"gemini","timestamp":"2026-10-10T08:00:02Z","model":"gemini-3-flash","content":"one"},{"id":"u2","type":"user","timestamp":"2026-10-10T08:00:03Z","content":"second"},{"id":"g2","type":"gemini","timestamp":"2026-10-10T08:00:04Z","content":"two"}]}},{"$patch":{"removeIds":["u2","missing"]}},{"$rewindTo":"not-there"},{"id":"i1","type":"info","content":"Note"}]"#;
    const ITEMS: &str = r#"[{"kind":"user","uuid":"u1","ts":"2026-10-10T08:00:01Z","text":"first","images":0,"seq":0},{"kind":"assistant","uuid":"g1:x","ts":"2026-10-10T08:00:02Z","msgId":"g1","model":"gemini-3-flash","block":{"type":"text","text":"one"},"seq":1},{"kind":"assistant","uuid":"g2:x","ts":"2026-10-10T08:00:04Z","msgId":"g2","block":{"type":"text","text":"two"},"seq":2},{"kind":"notice","uuid":"i1","ts":"","level":"info","subtype":"info","text":"Note","seq":3}]"#;
    let mut doc = GeminiDoc::new();
    for r in serde_json::from_str::<Vec<Value>>(RECORDS).unwrap() {
        doc.apply(&r);
    }
    let mut meta = new_meta();
    let items = doc.items(&mut meta);
    assert_eq!(serde_json::to_value(&items).unwrap(), serde_json::from_str::<Value>(ITEMS).unwrap());
    assert_eq!((meta.model.as_deref(), meta.stats.turns), (Some("gemini-3-flash"), 2));
}

/// a $patch whose lists aren't lists is skipped, and the session reads on
///
/// (`ITEMS` is the TS GeminiDoc's output for the same records, from bun)
#[test]
fn a_patch_whose_lists_arent_lists_is_skipped_and_the_session_reads_on() {
    const ITEMS: &str = r#"[{"kind":"user","uuid":"u1","ts":"","text":"hi","images":0,"seq":0},{"kind":"assistant","uuid":"g1:x","ts":"","msgId":"g1","block":{"type":"text","text":"two"},"seq":1},{"kind":"assistant","uuid":"g1:c1","ts":"","msgId":"g1","block":{"type":"tool_use","id":"c1","name":"Bash","input":{"command":"ls"}},"seq":2},{"kind":"tool_result","uuid":"g1:c1:r","ts":"","toolUseId":"c1","isError":false,"text":"a.ts","meta":{"stdout":""},"seq":3}]"#;
    let mut doc = GeminiDoc::new();
    doc.apply(&json!({ "id": "u1", "type": "user", "content": "hi" }));
    doc.apply(&json!({ "id": "g1", "type": "gemini", "content": "one", "toolCalls": [{ "id": "c1", "name": "run_shell_command", "args": { "command": "ls" } }] }));
    for updates in [json!({ "id": "u1", "content": "x" }), json!(7), json!("u1"), json!(null)] {
        doc.apply(&json!({ "$patch": { "updates": updates } }));
    }
    doc.apply(&json!({ "$patch": { "updates": [null, { "id": "g1", "toolCalls": { "id": "c1", "result": "no" } }], "removeIds": "u1" } }));
    doc.apply(&json!({ "$patch": { "id": "g1", "content": "two", "toolCalls": [null, { "id": "c1", "result": [{ "text": "a.ts" }] }] } }));
    let items = doc.items(&mut new_meta());
    assert_eq!(serde_json::to_value(&items).unwrap(), serde_json::from_str::<Value>(ITEMS).unwrap());
}

/// an update with no tool call id gives its result to no call
#[test]
fn an_update_with_no_tool_call_id_gives_its_result_to_no_call() {
    let mut doc = GeminiDoc::new();
    doc.apply(&json!({ "id": "g1", "type": "gemini", "content": "", "toolCalls": [{ "name": "run_shell_command", "args": { "command": "ls" } }, { "id": null, "name": "read_file", "args": { "file_path": "a.ts" } }] }));
    doc.apply(&json!({ "$patch": { "id": "g1", "toolCalls": [{ "result": [{ "text": "not for the shell" }] }, { "id": null, "result": [{ "text": "not for the read" }] }] } }));
    let kinds: Vec<Value> = serde_json::to_value(doc.items(&mut new_meta())).unwrap().as_array().unwrap().iter().map(|i| i["kind"].clone()).collect();
    assert_eq!(kinds, [json!("assistant"), json!("assistant")]);
}

/// From agents/discovery.test.ts: a Gemini chat is found under a home whose path has a space
#[test]
fn a_gemini_chat_is_found_under_a_home_whose_path_has_a_space() {
    use crate::test_support::sandbox::Sandbox;
    use crate::test_support::{host, rt, TempDir};
    let root = TempDir::new("inyon-find-");
    rt::run(|| async {
        let home = root.path("my home").to_string_lossy().into_owned();
        Sandbox::install(&root.s());
        let started_at = (now_ms() / 1000) as f64 - 60.0;
        let rel = "my home/.gemini/tmp/proj/chats/session-1.json";
        root.write(rel, &(crate::js::stringify(&json!({ "sessionId": "s1", "projectHash": sha256_hex("/work/my app") })) + "\n"));
        let proc = GeminiProc { cwd: Some("/work/my app".into()), started_at };
        assert_eq!(find_gemini_session(&host("find", "local"), &home, &proc).await.unwrap(), Some(root.path(rel).to_string_lossy().into_owned()));
    });
}
