//! Port of claude/transcript-tail.ts: the commands that follow a transcript file on another
//! machine, and the reader that turns their output back into entries. A long session's file runs
//! to tens of megabytes, mostly screenshots as base64 the app never shows, so a remote read drops
//! those on the machine itself. Every line's real size is still counted, so after a dropped
//! connection the read picks up at the byte where it stopped instead of starting over.

use serde_json::Value;

use crate::hosts::host::shq;
use crate::js::{to_number, JsStr};
use crate::re;

/// Bytes a string takes as UTF-8, without encoding it (transcript lines can be megabytes). A Rust
/// string is UTF-8 already; the TS counts a lone surrogate as 3 bytes, which U+FFFD also takes.
pub fn utf8_length(s: &str) -> usize {
    s.len()
}

/// Prints each line's size in the file (a line of \x1e and the bytes, newline included) before
/// the line, so the count doesn't depend on how the other end decodes text. In lines that carry
/// base64 (images), the image data (1024+ base64 characters right after `"data":"`, or after
/// `;base64,` in a data URL) becomes an ellipsis. Only there: text next to an image can hold long
/// runs of the same characters, and it is part of the message. Written for any POSIX awk (gawk,
/// mawk, busybox, macOS).
const SLIM: &str = r#"BEGIN { r = "[A-Za-z0-9+/=]"; for (i = 0; i < 10; i++) r = r r; r = "(\"data\":\"|;base64,)" r "[A-Za-z0-9+/=]*" }
{
  n = length($0); s = $0; o = ""; cut = 0
  if (n > 1024 && index(s, "base64")) while (match(s, r)) {
    # both prefixes are 8 characters; keep them, drop the data
    o = o substr(s, 1, RSTART + 7) "…"; s = substr(s, RSTART + RLENGTH); cut = 1
  }
  printf "\036%d\n", n + 1
  if (cut) print o s
  else print
  fflush()
}"#;

/// Follows `path` from byte `from`; `slim` drops image data on the far side (for remote
/// machines). With `follow` off it reads to the end and exits.
///
/// A remote follow must run with stdin kept open (`keepStdin`): it ends itself when stdin closes.
/// Over SSH nothing else stops it. Closing the connection doesn't signal `tail -F | awk`, and they
/// only notice once they write to the closed connection, which for a quiet transcript is never.
pub fn tail_command(path: &str, from: u64, slim: bool, follow: bool) -> String {
    let tail = format!("tail -c +{}{} {} 2>/dev/null", from + 1, if follow { " -F" } else { "" }, shq(path));
    if !slim {
        return format!("exec {tail}");
    }
    // mawk holds back piped input until its buffer fills unless it's told the input is live
    let run = slimmed(&tail, true);
    if follow {
        until_stdin_closes(&run)
    } else {
        run
    }
}

/// The slimming step for a remote machine, or nothing for this one.
fn slimmed(source: &str, slim: bool) -> String {
    if !slim {
        return source.to_owned();
    }
    format!(
        r#"if command -v awk >/dev/null 2>&1; then a=awk; case "$(awk -W version </dev/null 2>&1 | head -n 1)" in *mawk*) a="awk -W interactive";; esac; {source} | LC_ALL=C $a {}; else {source}; fi"#,
        shq(SLIM)
    )
}

/// Bytes `from` up to `to` of the file: an earlier part of a long conversation, read on demand.
pub fn range_command(path: &str, from: u64, to: u64, slim: bool) -> String {
    slimmed(&format!("tail -c +{} {} 2>/dev/null | head -c {}", from + 1, shq(path), to.saturating_sub(from)), slim)
}

/// Lines that change what's known about a conversation as a whole (its title, goal, turn and
/// compaction counts, background tasks, subagents, artifacts, Remote Control), not what was said.
/// Picked out of the part before a long conversation's last stretch, on the machine itself, so
/// opening a 100 MB conversation sends well under a megabyte instead. Tool names only count inside
/// a tool call, so a message that merely mentions one stays behind.
fn meta_lines() -> String {
    [
        r#""subtype":"(turn_duration|compact_boundary)""#,
        r#""(startTime|aiTitle|customTitle|goal_status|task_status|bridge_status|backgroundTaskId|taskId|isAsync)""#,
        "<task-notification>",
        "Remote Control disconnected",
        r#""run_in_background":true"#,
        "Successfully (stopped task|killed shell)",
        r#""type":"tool_use","id":"[^"]*","name":"(Agent|Task|Monitor|Workflow|TaskStop|KillShell|Artifact)""#,
        r#""url":"https://claude\.ai/"#,
    ]
    .join("|")
}

/// Attachments that match but copy the whole prompt or tool list into every turn.
const META_SKIP: [&str; 2] = [r#""type":"prompt_snapshot""#, r#""type":"deferred_tools_record""#];

/// Keeps only the last title (every turn records one) and drops the output inside a task
/// notification, which can be pages long; only the tags around it are read.
const META_TRIM: &str = r#"index($0, "\"type\":\"ai-title\"") { title = $0; next }
index($0, "<result>") {
  s = $0; o = ""
  while ((a = index(s, "<result>")) && (b = index(substr(s, a), "</result>"))) { o = o substr(s, 1, a + 7); s = substr(s, a + b - 1) }
  $0 = o s
}
{ print }
END { if (title != "") print title }"#;

fn part(path: &str, from: u64, to: u64) -> String {
    if from > 0 {
        format!("tail -c +{} {} 2>/dev/null | head -c {}", from + 1, shq(path), to.saturating_sub(from))
    } else {
        format!("head -c {to} {} 2>/dev/null", shq(path))
    }
}

/// The META_LINES lines among bytes `from` up to `to` of the file.
pub fn meta_command(path: &str, to: u64, slim: bool, from: u64) -> String {
    let skip = META_SKIP.iter().map(|m| format!("-e {}", shq(m))).collect::<Vec<_>>().join(" ");
    let lines = format!("{} | grep -E {} | grep -v -F {skip}", part(path, from, to), shq(&meta_lines()));
    slimmed(&format!("if command -v awk >/dev/null 2>&1; then {lines} | LC_ALL=C awk {}; else {lines}; fi", shq(META_TRIM)), slim)
}

/// The Codex rollout lines that carry the conversation as a whole: its folder and version
/// (session_meta), model and permissions (turn_context, thread_settings_applied), usage
/// (token_count), turns, the goal, compactions, each file change (for the line counts) and the
/// sub-agents. Everything that was said stays behind.
fn codex_meta_lines() -> String {
    [
        r#""type":"session_meta""#,
        r#""type":"turn_context""#,
        r#""thread_settings_applied""#,
        r#""token_count""#,
        r#""task_started""#,
        r#""task_complete""#,
        r#""turn_aborted""#,
        r#""thread_goal_updated""#,
        r#""type":"compacted""#,
        r#"compaction""#,
        r#""type":"FileChange""#,
        r#""type":"SubAgentActivity""#,
    ]
    .join("|")
}

/// The CODEX_META_LINES lines among bytes `from` up to `to` of a rollout file.
pub fn codex_meta_command(path: &str, to: u64, slim: bool, from: u64) -> String {
    slimmed(&format!("{} | grep -E {}", part(path, from, to), shq(&codex_meta_lines())), slim)
}

/// Runs `cmd` until stdin closes (run it with `keepStdin`), for a command that never ends on its
/// own on a machine reached over SSH, where closing the connection doesn't stop it.
pub fn until_stdin_closes(cmd: &str) -> String {
    // a background job's stdin is /dev/null, so it watches the real one through fd 3; once that
    // closes it stops this shell's other children (tail and awk), and when they end it goes too
    format!("exec 3<&0; {{ cat <&3; pkill -P $$; }} >/dev/null 2>&1 & w=$!; {cmd}; kill $w 2>/dev/null")
}

/// The file's size, and whether byte `from` starts a line (the one before it is a newline), so a
/// read can carry on from there. Prints `<size> <ok>`.
pub fn size_command(path: &str, from: u64) -> String {
    let f = shq(path);
    if from == 0 {
        return format!(r#"echo "$(wc -c < {f} 2>/dev/null || echo 0) 1""#);
    }
    format!(r#"echo "$(wc -c < {f} 2>/dev/null || echo 0) $(dd if={f} bs=1 skip={} count=1 2>/dev/null | od -An -tx1 | tr -d ' \n' | grep -c '^0a$')""#, from - 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub size: u64,
    pub aligned: bool,
}

pub fn parse_size(stdout: &str) -> Size {
    let mut parts = re!(r"\s+").split(stdout.jtrim());
    let size = to_number(parts.next().unwrap_or(""));
    let ok = parts.next();
    Size { size: if size.is_finite() && size > 0.0 { size as u64 } else { 0 }, aligned: ok == Some("1") }
}

/// One line through the reader: its entry (None when it didn't parse) and the bytes it took.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadLine {
    pub entry: Option<Value>,
    pub bytes: u64,
}

/// Turns the lines of a tail into entries, keeping count of how far into the file they reach.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptReader {
    /// bytes of the file read up to the end of the last complete line
    pub offset: u64,
    size: Option<u64>,
    /// a line that didn't parse; the stream may have cut it short, so it counts once another follows
    held: u64,
}

impl TranscriptReader {
    pub fn new(from: u64) -> Self {
        TranscriptReader { offset: from, size: None, held: 0 }
    }

    /// bytes of the file that have come through, a line still held back included
    pub fn received(&self) -> u64 {
        self.offset + self.held
    }

    /// The line's entry (if it is one) and the bytes it took in the file; None for a size mark.
    pub fn line(&mut self, raw: &str) -> Option<ReadLine> {
        if let Some(mark) = raw.strip_prefix('\u{1e}') {
            // two marks in a row: the line between was empty, and streams don't pass empty lines on
            if let Some(s) = self.size {
                self.offset += s;
            }
            let n = to_number(mark);
            self.size = (n.is_finite() && n > 0.0).then_some(n as u64);
            return None;
        }
        let bytes = self.size.take().unwrap_or(utf8_length(raw) as u64 + 1);
        self.offset += self.held;
        self.held = 0;
        let Some(entry) = crate::js::json::parse(raw) else {
            self.held = bytes;
            return Some(ReadLine { entry: None, bytes });
        };
        self.offset += bytes;
        Some(ReadLine { entry: Some(entry), bytes })
    }
}

#[cfg(test)]
#[path = "transcript_tail_tests.rs"]
mod tests;
