//! claude/transcript-tail.test.ts, the tests of the commands and the reader. They run the commands
//! with `sh`, as the TS tests do, so they only run where there is one. The tests of following a
//! conversation through the watcher and its saved copy stay with the watcher in TS.

use super::*;
use serde_json::json;

#[cfg(unix)]
mod with_sh {
    use super::*;
    use crate::claude::transcript::{new_meta, normalize};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    fn dir() -> crate::test_support::TempDir {
        crate::test_support::TempDir::new("inyon-core-tail-")
    }

    fn png() -> String {
        format!("iVBORw0KGgo{}", "A".repeat(50_000))
    }

    fn user(n: u32, text: &str, image: bool) -> String {
        let mut content = vec![json!({ "type": "text", "text": text })];
        if image {
            content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": png() } }));
        }
        let ts = crate::js::date::to_iso(1_800_000_000_000.0 + n as f64 * 1000.0).unwrap();
        format!("{}\n", crate::js::stringify(&json!({ "type": "user", "uuid": format!("u{n}"), "timestamp": ts, "message": { "role": "user", "content": content } })))
    }

    /// Runs a command to the end and returns its lines, as a stream would hand them over.
    fn lines(cmd: &str) -> Vec<String> {
        let out = Command::new("sh").arg("-c").arg(cmd).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut v: Vec<String> = text.split('\n').map(str::to_owned).collect();
        v.pop();
        v
    }

    fn size(p: &PathBuf) -> u64 {
        std::fs::metadata(p).unwrap().len()
    }

    fn text_of(e: &Value) -> String {
        e["message"]["content"][0]["text"].as_str().unwrap_or("").to_owned()
    }

    /// drops image data on the far side but counts every byte of the file
    #[test]
    fn drops_image_data_on_the_far_side_but_counts_every_byte_of_the_file() {
        let d = dir();
        let file = d.path("slim.jsonl");
        std::fs::write(&file, user(1, "héllo ✓ 🙂", false) + &user(2, "a screenshot", true) + &user(3, "after", false)).unwrap();
        let out = lines(&tail_command(file.to_str().unwrap(), 0, true, false));
        assert!(out.join("\n").len16() < 2000);
        let mut r = TranscriptReader::new(0);
        let entries: Vec<Value> = out.iter().filter_map(|l| r.line(l).and_then(|x| x.entry)).collect();
        assert_eq!(entries.iter().map(text_of).collect::<Vec<_>>(), vec!["héllo ✓ 🙂", "a screenshot", "after"]);
        assert_eq!(entries[1]["message"]["content"][1]["type"], "image");
        assert_eq!(r.offset, size(&file));
    }

    /// only image data is dropped; long text next to an image stays whole
    #[test]
    fn only_image_data_is_dropped_long_text_next_to_an_image_stays_whole() {
        let d = dir();
        let file = d.path("mixed.jsonl");
        let text = "Q".repeat(2048);
        let codex = crate::js::stringify(
            &json!({ "type": "response_item", "payload": { "content": [{ "type": "input_text", "text": "see" }, { "type": "input_image", "image_url": format!("data:image/png;base64,{}", png()) }] } }),
        );
        std::fs::write(&file, user(1, &text, true) + &codex + "\n").unwrap();
        let mut r = TranscriptReader::new(0);
        let entries: Vec<Value> = lines(&tail_command(file.to_str().unwrap(), 0, true, false)).iter().filter_map(|l| r.line(l).and_then(|x| x.entry)).collect();
        assert_eq!(text_of(&entries[0]), text);
        assert_eq!(entries[0]["message"]["content"][1]["source"]["data"], "…");
        assert_eq!(entries[1]["payload"]["content"][1]["image_url"], "data:image/png;base64,…");
        assert_eq!(r.offset, size(&file));
    }

    /// reading on from an offset gives exactly the lines after it
    #[test]
    fn reading_on_from_an_offset_gives_exactly_the_lines_after_it() {
        let d = dir();
        let file = d.path("resume.jsonl");
        let path = file.to_str().unwrap();
        std::fs::write(&file, user(1, "one", false) + &user(2, "two", true)).unwrap();
        let mut first = TranscriptReader::new(0);
        for l in lines(&tail_command(path, 0, true, false)) {
            first.line(&l);
        }
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        std::io::Write::write_all(&mut f, (user(3, "three", false) + &user(4, "four", false)).as_bytes()).unwrap();
        let mut next = TranscriptReader::new(first.offset);
        let texts: Vec<String> = lines(&tail_command(path, first.offset, true, false)).iter().filter_map(|l| next.line(l).and_then(|x| x.entry)).map(|e| text_of(&e)).collect();
        assert_eq!(texts, vec!["three", "four"]);
        assert_eq!(next.offset, size(&file));
    }

    fn tails(file: &str) -> usize {
        let out = Command::new("ps").args(["-eo", "args="]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).split('\n').filter(|l| l.starts_with("tail ") && l.contains(file)).count()
    }

    fn ends_with_stdin(cmd: &str, file: &str) {
        // as over SSH: its own session, stdin a pipe from the hub, and nobody kills the group
        let mut p = Command::new("setsid").args(["sh", "-c", cmd]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(tails(file), 1);
        drop(p.stdin.take());
        let start = std::time::Instant::now();
        while p.try_wait().unwrap().is_none() {
            assert!(start.elapsed().as_secs() < 3, "stuck");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut out = String::new();
        std::io::Read::read_to_string(&mut p.stdout.take().unwrap(), &mut out).unwrap();
        assert!(out.contains("exited"));
        assert_eq!(tails(file), 0);
    }

    fn has_setsid() -> bool {
        Command::new("sh").args(["-c", "command -v setsid"]).output().is_ok_and(|o| o.status.success())
    }

    /// a remote follow ends once its stdin closes, tail and all
    #[test]
    fn a_remote_follow_ends_once_its_stdin_closes_tail_and_all() {
        if !has_setsid() {
            return;
        }
        let d = dir();
        let file = d.path("closed.jsonl");
        std::fs::write(&file, user(1, "quiet", false)).unwrap();
        let path = file.to_str().unwrap();
        ends_with_stdin(&format!("{}; echo exited", tail_command(path, 0, true, true)), path);
    }

    /// any command can be made to end with its stdin
    #[test]
    fn any_command_can_be_made_to_end_with_its_stdin() {
        if !has_setsid() {
            return;
        }
        let d = dir();
        let file = d.path("events.jsonl");
        std::fs::write(&file, "").unwrap();
        let path = file.to_str().unwrap();
        // the shape of the hook event follow: a setup step, then the tail
        let cmd = format!("touch {path} && {{ {}; }}; echo exited", until_stdin_closes(&format!("tail -n0 -F {path} 2>/dev/null")));
        ends_with_stdin(&cmd, path);
    }

    /// counts by the sizes the far side reports, however the text was decoded
    #[test]
    fn counts_by_the_sizes_the_far_side_reports_however_the_text_was_decoded() {
        let d = dir();
        let file = d.path("marks.jsonl");
        std::fs::write(&file, user(1, "🙂 split across chunks", false) + "\n" + &user(2, "next", false)).unwrap();
        let mut r = TranscriptReader::new(0);
        let out: Vec<String> =
            lines(&tail_command(file.to_str().unwrap(), 0, true, false)).into_iter().map(|l| if l.starts_with('{') { l.replacen('🙂', "\u{fffd}", 1) } else { l }).collect();
        let texts: Vec<String> = out.iter().filter(|l| !l.is_empty()).filter_map(|l| r.line(l).and_then(|x| x.entry)).map(|e| text_of(&e)).collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(r.offset, size(&file));
    }

    /// checks that a line starts where the read stopped
    #[test]
    fn checks_that_a_line_starts_where_the_read_stopped() {
        let d = dir();
        let file = d.path("align.jsonl");
        let one = user(1, "one", false);
        std::fs::write(&file, one.clone() + &user(2, "two", false)).unwrap();
        let at = |from: u64| {
            let out = Command::new("sh").arg("-c").arg(size_command(file.to_str().unwrap(), from)).output().unwrap();
            parse_size(&String::from_utf8_lossy(&out.stdout))
        };
        let n = utf8_length(&one) as u64;
        assert_eq!(at(0), Size { size: size(&file), aligned: true });
        assert!(at(n).aligned);
        assert!(!at(n + 7).aligned);
        assert!(!at(n - 1).aligned);
    }

    /// the summary of a long conversation leaves out what was said
    #[test]
    fn the_summary_of_a_long_conversation_leaves_out_what_was_said() {
        let d = dir();
        let file = d.path("meta.jsonl");
        let line = |o: Value| {
            let mut v = json!({ "uuid": crate::ids::random_uuid(), "timestamp": "2027-01-01T00:00:00Z" });
            v.as_object_mut().unwrap().extend(o.as_object().unwrap().clone());
            crate::js::stringify(&v) + "\n"
        };
        let tool = |id: &str, name: &str, input: Value| {
            line(json!({ "type": "assistant", "message": { "role": "assistant", "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } }))
        };
        let result = |id: &str, tur: Value| {
            line(json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": id, "content": "ok" }] }, "toolUseResult": tur }))
        };
        let notice =
            format!("<task-notification><task-id>b2</task-id><status>completed</status><summary>build done</summary><result>{}</result></task-notification>", "log ".repeat(5000));
        let text = [
            line(json!({ "type": "ai-title", "aiTitle": "first title" })),
            line(json!({ "type": "user", "message": { "role": "user", "content": format!("what does \"name\":\"Agent\" do? {}", "x".repeat(5000)) } })),
            line(json!({ "type": "attachment", "attachment": { "type": "prompt_snapshot", "prompt": format!("\"name\":\"Agent\" <task-notification> {}", "y".repeat(5000)) } })),
            tool("t1", "Agent", json!({ "description": "look around", "prompt": "go" })),
            // a command that outlived its timeout: its call never asked for the background
            tool("t2", "Bash", json!({ "command": "make", "description": "build" })),
            result("t2", json!({ "stdout": "", "backgroundTaskId": "b1" })),
            tool("t3", "TaskStop", json!({ "task_id": "b1" })),
            result("t3", json!({ "message": "Successfully stopped task: b1 (make)" })),
            line(json!({ "type": "queue-operation", "operation": "enqueue", "content": notice })),
            line(json!({ "type": "ai-title", "aiTitle": "last title" })),
        ]
        .concat();
        std::fs::write(&file, &text).unwrap();
        let out = Command::new("sh").arg("-c").arg(meta_command(file.to_str().unwrap(), size(&file), false, 0)).output().unwrap();
        let out = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(!out.contains("xxxx"));
        assert!(!out.contains("yyyy"));
        assert!(!out.contains("log log"));
        assert!(!out.contains("first title"));
        let mut meta = new_meta();
        for l in out.split('\n').filter(|l| !l.is_empty()) {
            normalize(&crate::js::json::parse(l).unwrap(), &mut meta, false);
        }
        assert_eq!(meta.title.as_deref(), Some("last title"));
        assert_eq!(meta.subagents.values().map(|s| s.description.as_str()).collect::<Vec<_>>(), vec!["look around"]);
        assert_eq!(meta.tasks["b1"].status, inyon_protocol::TaskStatus::Stopped);
        assert_eq!(meta.tasks["b2"].summary.as_deref(), Some("build done"));
    }
}

/// a line cut short by a dropped stream is read again, not skipped
#[test]
fn a_line_cut_short_by_a_dropped_stream_is_read_again_not_skipped() {
    let user = |text: &str| crate::js::stringify(&json!({ "type": "user", "uuid": "u1", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })) + "\n";
    let full = user("whole");
    let mut r = TranscriptReader::new(0);
    let before = user("before");
    r.line(&before[..before.len() - 1]);
    let at = r.offset;
    assert_eq!(r.line(&full[..20]).and_then(|x| x.entry), None);
    assert_eq!(r.offset, at);
    assert_eq!(utf8_length("é🙂"), 6);
}
