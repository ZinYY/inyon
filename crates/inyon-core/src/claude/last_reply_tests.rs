//! claude/last-reply.test.ts

use serde_json::json;

use super::*;
use crate::test_support::rt::run;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{machine, TempDir};

#[test]
fn markdown_becomes_one_short_plain_line() {
    assert_eq!(
        plain_line("## Done\n\n**All 12 tests** pass. See `src/app.ts` and [the PR](https://x.y).\n\n```ts\ncode\n```\n- next: ship", 180),
        "Done All 12 tests pass. See src/app.ts and the PR. next: ship"
    );
    let long = plain_line(&"word ".repeat(100), 180);
    assert!(long.len16() <= 181);
    assert!(long.ends_with("word…"));
}

#[test]
fn emphasis_markers_go_only_around_text() {
    assert_eq!(strip_emphasis("a * b * c"), "a * b * c");
    assert_eq!(strip_emphasis("__init__ and *x*"), "init and x");
    assert_eq!(strip_emphasis("~~old~~ **new"), "old **new");
}

#[test]
fn a_finished_notification_quotes_the_start_of_the_agents_reply_from_its_transcript() {
    run(|| async {
        let dir = TempDir::new("inyon-reply-");
        let line = |o: serde_json::Value| {
            let mut v = json!({ "timestamp": "2026-10-11T10:00:00Z", "uuid": crate::ids::random_uuid() });
            for (k, x) in o.as_object().unwrap() {
                v[k] = x.clone();
            }
            format!("{v}\n")
        };
        let text = format!(
            "{}\n{}{}{}",
            "x".repeat(300_000),
            line(json!({ "type": "user", "message": { "role": "user", "content": "fix the build" } })),
            line(
                json!({ "type": "assistant", "message": { "id": "m1", "role": "assistant", "content": [{ "type": "text", "text": "Looking." }, { "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "make" } }] } })
            ),
            line(json!({ "type": "assistant", "message": { "id": "m2", "role": "assistant", "content": [{ "type": "text", "text": "**Fixed.** The build passes again." }] } })),
        );
        dir.write("s.jsonl", &text);
        Sandbox::install(&dir.s());
        machine("r", "local", json!([{ "id": "%9", "agent": { "kind": "claude", "pid": 7, "state": "idle", "transcriptPath": dir.path("s.jsonl") } }]));
        assert_eq!(last_reply("r", "%9").await.unwrap().as_deref(), Some("Fixed. The build passes again."));
    });
}
