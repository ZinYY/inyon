//! Port of claude/keep-default.ts: editing one key of Claude Code's settings.json in place, and
//! watching the file after a switch to put the old defaults back.
//!
//! Claude Code's /model and /effort change the running session and also save the pick in the
//! user's settings.json as the default for new sessions. A switch from Inyon is for one session,
//! so the hub notes these keys before the switch and, once Claude Code has written them, puts the
//! earlier values back. Only those keys change: everything else in the file stays byte for byte.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{self, stringify};
use crate::js::num::parse_int;
use crate::platform::console;
use crate::platform::rt::{sleep, spawn, Deferred};

/// Effort lands in "effortLevel" (older Claude Code) or under "modelSettings" per model (2.1.29x).
pub const DEFAULT_KEYS: [&str; 3] = ["model", "effortLevel", "modelSettings"];

// the file, and a checksum of it to notice changes (busybox builds can lack cksum; od is everywhere)
const FILE: &str = r#"f="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json"
sum() { cksum < "$1" 2>/dev/null || md5sum < "$1" 2>/dev/null || od -An -v -tx1 < "$1" | tr -d ' \n'; }"#;

/// How long after a switch the file is watched: the /model menu and "Switch model?" can wait for an answer.
pub const WINDOW_MS: u64 = 180_000;
/// After putting a value back, how long to look for a second write (a menu saves model and effort apart).
pub const GRACE_MS: u64 = 6_000;

/// The file as read: its checksum ('' when it doesn't exist) and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub sum: String,
    pub text: String,
}

/// What `readSettings` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadSettings {
    Read(Settings),
    /// the checksum still matches `known`
    Same,
    /// it can't be read or isn't UTF-8
    Unreadable,
}

/// The script `readSettings` runs: the file's checksum and, unless it still matches `known`, its
/// bytes as hex (od, so Windows' CRLF and any byte survive the trip).
pub fn read_settings_command(known: Option<&str>) -> String {
    format!(
        "{FILE}\n[ -f \"$f\" ] || {{ echo none; exit 0; }}\ns=$(sum \"$f\") || exit 1\necho \"$s\"\n[ \"$s\" = {} ] && exit 0\nod -An -v -tx1 < \"$f\"",
        shq(known.unwrap_or(""))
    )
}

/// What `readSettings` makes of that script's exit code and output.
pub fn read_settings_output(code: i32, stdout: &str, known: Option<&str>) -> ReadSettings {
    if code != 0 {
        return ReadSettings::Unreadable;
    }
    let mut lines = stdout.split('\n');
    let first = lines.next().unwrap_or("");
    use crate::js::JsStr;
    if first.jtrim() == "none" {
        return ReadSettings::Read(Settings { sum: String::new(), text: String::new() });
    }
    let sum = first.jtrim();
    if sum.is_empty() {
        return ReadSettings::Unreadable;
    }
    if known == Some(sum) {
        return ReadSettings::Same;
    }
    let hex: String = crate::js::text::squash(&lines.collect::<String>());
    if !hex.len16().is_multiple_of(2) {
        return ReadSettings::Unreadable;
    }
    let chars: Vec<char> = hex.chars().collect();
    let bytes: Vec<u8> = chars.chunks(2).map(|p| parse_int(&p.iter().collect::<String>(), 16)).map(|b| if b.is_nan() { 0 } else { b as u8 }).collect();
    match String::from_utf8(bytes) {
        Ok(text) => ReadSettings::Read(Settings { sum: sum.to_owned(), text }),
        Err(_) => ReadSettings::Unreadable,
    }
}

/// The script `writeSettings` runs with the new text on stdin: it replaces the file if it still
/// has checksum `sum`. A symlinked settings.json (a dotfiles repo) is written through the link;
/// otherwise a copy keeps the file's mode and is moved over it, so Claude Code never reads half a
/// file. Exit code 0 means written.
pub fn write_settings_command(sum: &str) -> String {
    format!(
        "{FILE}\nt=\"$f.inyon-tmp\"\n[ -f \"$f\" ] && [ \"$(sum \"$f\")\" = {} ] || exit 3\ntrap 'rm -f \"$t\"' EXIT HUP INT TERM\nif [ -L \"$f\" ]; then umask 077; cat > \"$t\" && cat \"$t\" > \"$f\"\nelse cp -p \"$f\" \"$t\" && cat > \"$t\" && mv -f \"$t\" \"$f\"; fi",
        shq(sum)
    )
}

// ---- one key of a JSON object, edited in place ----

/// A top-level member. Positions are byte offsets into the text (the TS counts UTF-16 units; both
/// only cut the text at ASCII punctuation, so the edits come out the same).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub key: String,
    /// where the key's opening quote is
    pub start: usize,
    pub value_start: usize,
    pub value_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopMembers {
    pub open: usize,
    pub close: usize,
    pub members: Vec<Member>,
}

struct Scan<'a> {
    text: &'a str,
    i: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.i..].chars().next()
    }
    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.i += c.len_utf8();
        }
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n' | '\u{feff}')) {
            self.bump();
        }
    }
    fn string(&mut self) -> Option<()> {
        if self.peek() != Some('"') {
            return None;
        }
        self.bump();
        while let Some(c) = self.peek() {
            self.bump();
            if c == '\\' {
                self.bump();
            } else if c == '"' {
                return Some(());
            }
        }
        None
    }
    fn value(&mut self) -> Option<()> {
        match self.peek() {
            Some('"') => self.string(),
            Some('{' | '[') => {
                let mut depth = 0;
                while let Some(c) = self.peek() {
                    match c {
                        '"' => {
                            self.string()?;
                            continue;
                        }
                        '{' | '[' => depth += 1,
                        '}' | ']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.bump();
                                return Some(());
                            }
                        }
                        _ => {}
                    }
                    self.bump();
                }
                None
            }
            _ => {
                let start = self.i;
                while self.peek().is_some_and(|c| !",}] \t\r\n".contains(c)) {
                    self.bump();
                }
                (self.i != start).then_some(())
            }
        }
    }
}

/// The top-level members of a JSON object, with where each sits in the text.
pub fn top_members(text: &str) -> Option<TopMembers> {
    let mut s = Scan { text, i: 0 };
    s.ws();
    if s.peek() != Some('{') {
        return None;
    }
    let open = s.i;
    s.bump();
    let mut members = vec![];
    s.ws();
    if s.peek() == Some('}') {
        return Some(TopMembers { open, close: s.i, members });
    }
    loop {
        s.ws();
        let start = s.i;
        s.string()?;
        let key = crate::js::json::parse(&text[start..s.i])?.as_str()?.to_owned();
        s.ws();
        if s.peek() != Some(':') {
            return None;
        }
        s.bump();
        s.ws();
        let value_start = s.i;
        s.value()?;
        json::parse(&text[value_start..s.i])?;
        members.push(Member { key, start, value_start, value_end: s.i });
        s.ws();
        match s.peek() {
            Some(',') => s.bump(),
            Some('}') => return Some(TopMembers { open, close: s.i, members }),
            _ => return None,
        }
    }
}

/// Each key's value as written (the raw JSON), or None when the key isn't there. None overall
/// when the text isn't a JSON object.
pub fn key_values(text: &str, keys: &[&str]) -> Option<IndexMap<String, Option<String>>> {
    use crate::js::JsStr;
    let obj = if text.is_blank() { TopMembers { open: 0, close: 0, members: vec![] } } else { top_members(text)? };
    Some(keys.iter().map(|k| (k.to_string(), obj.members.iter().rev().find(|m| m.key == *k).map(|m| text[m.value_start..m.value_end].to_owned()))).collect())
}

/// The text with one top-level key set to `raw` (JSON as written) or removed (None), everything
/// around it untouched. None if the text isn't a JSON object.
pub fn set_key(text: &str, key: &str, raw: Option<&str>) -> Option<String> {
    let obj = top_members(text)?;
    let members = &obj.members;
    let at = members.iter().rposition(|m| m.key == key);
    if let Some(raw) = raw {
        if let Some(m) = at.map(|a| &members[a]) {
            return Some(format!("{}{raw}{}", &text[..m.value_start], &text[m.value_end..]));
        }
        let pair = format!("{}: {raw}", stringify(&serde_json::Value::String(key.into())));
        // first in the object, indented like the key it goes in front of
        if let Some(first) = members.first() {
            return Some(format!("{}{pair},{}{}", &text[..first.start], &text[obj.open + 1..first.start], &text[first.start..]));
        }
        return Some(format!("{}\n  {pair}\n{}", &text[..obj.open + 1], &text[obj.close..]));
    }
    let Some(at) = at else { return Some(text.to_owned()) };
    let m = &members[at];
    if members.len() == 1 {
        return Some(format!("{}{}", &text[..obj.open + 1], &text[obj.close..]));
    }
    // up to the next key (the comma and the next line's indent go with it), or from the previous value
    if at < members.len() - 1 {
        return Some(format!("{}{}", &text[..m.start], &text[members[at + 1].start..]));
    }
    Some(format!("{}{}", &text[..members[at - 1].value_end], &text[m.value_end..]))
}

fn same(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => match (json::parse(a), json::parse(b)) {
            (Some(x), Some(y)) => stringify(&x) == stringify(&y),
            _ => a == b,
        },
        _ => a == b,
    }
}

/// `now` with every key in `before` that differs put back as it was; None when nothing differs.
pub fn restore_keys(now: &str, before: &IndexMap<String, Option<String>>) -> Option<String> {
    let keys: Vec<&str> = before.keys().map(String::as_str).collect();
    let current = key_values(now, &keys)?;
    let mut text = now.to_owned();
    let mut changed = false;
    for (k, was) in before {
        if same(current.get(k).and_then(|v| v.as_deref()), was.as_deref()) {
            continue;
        }
        text = set_key(&text, k, was.as_deref())?;
        changed = true;
    }
    changed.then_some(text)
}

// ---- the watch ----

async fn read_settings(host: &Host, known: Option<&str>) -> HubResult<ReadSettings> {
    let r = host.exec(&read_settings_command(known), ExecOptions::timeout(15000)).await?;
    Ok(read_settings_output(r.code, &r.stdout, known))
}

async fn write_settings(host: &Host, sum: &str, text: &str) -> HubResult<bool> {
    let r = host.exec(&write_settings_command(sum), ExecOptions::input(text).with_timeout(15000)).await?;
    Ok(r.code == 0)
}

struct Watch {
    before: IndexMap<String, Option<String>>,
    until: u64,
}

thread_local! {
    static WATCHING: RefCell<HashMap<String, Rc<RefCell<Watch>>>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Default)]
pub struct KeepOptions {
    pub keys: Option<Vec<String>>,
    pub window_ms: Option<u64>,
    pub grace_ms: Option<u64>,
    pub poll_ms: Option<u64>,
}

/// Notes the default model and effort in the machine's Claude Code settings, then watches the file
/// in the background and puts them back once Claude Code saves a switch. A second switch while a
/// watch runs extends it and keeps the first values. Returns once the values are noted; the
/// promise it gives settles when the watch ends (for tests).
pub async fn keep_claude_defaults(host: &Host, opts: KeepOptions) -> Deferred<()> {
    let window = opts.window_ms.unwrap_or(WINDOW_MS);
    let id = host.id();
    if let Some(running) = WATCHING.with(|w| w.borrow().get(&id).cloned()) {
        let mut r = running.borrow_mut();
        r.until = r.until.max(now_ms() + window);
        return Deferred::resolved(());
    }
    let first = read_settings(host, None).await.unwrap_or(ReadSettings::Unreadable);
    let keys: Vec<String> = opts.keys.clone().unwrap_or_else(|| DEFAULT_KEYS.iter().map(|k| k.to_string()).collect());
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    // a file that isn't JSON (or isn't UTF-8) is left alone
    let ReadSettings::Read(first) = first else { return Deferred::resolved(()) };
    let Some(before) = key_values(&first.text, &keys) else { return Deferred::resolved(()) };
    let w = Rc::new(RefCell::new(Watch { before, until: now_ms() + window }));
    WATCHING.with(|m| m.borrow_mut().insert(id.clone(), w.clone()));
    let done = Deferred::new();
    let (d, host) = (done.clone(), host.clone());
    spawn(async move {
        follow(&host, &w, first.sum, &opts).await;
        WATCHING.with(|m| {
            let mut m = m.borrow_mut();
            if m.get(&id).is_some_and(|x| Rc::ptr_eq(x, &w)) {
                m.remove(&id);
            }
        });
        d.resolve(());
    });
    done
}

async fn follow(host: &Host, w: &Rc<RefCell<Watch>>, sum: String, opts: &KeepOptions) {
    let poll = opts.poll_ms.unwrap_or(1000);
    let grace = opts.grace_ms.unwrap_or(GRACE_MS);
    let mut known = sum;
    while now_ms() < w.borrow().until {
        sleep(poll).await;
        let ReadSettings::Read(now) = read_settings(host, Some(&known)).await.unwrap_or(ReadSettings::Unreadable) else { continue };
        known = now.sum.clone();
        if now.sum.is_empty() {
            continue;
        }
        let Some(text) = restore_keys(&now.text, &w.borrow().before) else { continue };
        if write_settings(host, &now.sum, &text).await.unwrap_or(false) {
            console::log(&format!("[claude] kept the default model and effort in {}'s Claude Code settings", host.id()));
            let mut w = w.borrow_mut();
            w.until = w.until.min(now_ms() + grace);
        }
    }
}

/// describe('editing one key') and describe('keeping the default on the machine'), from
/// keep-default.test.ts. The second runs the scripts in `sh` against a real file; the busybox
/// variant runs too when busybox is installed.
#[cfg(test)]
mod tests {
    use super::*;

    const FILE_TEXT: &str = "{\n  \"model\": \"opus\",\n  \"env\": { \"A\": \"é ✓\" },\n  \"hooks\": {\"Stop\": [{\"x\": \"}\"}]}\n}\n";

    /// finds the top-level members, strings with braces and all
    #[test]
    fn finds_the_top_level_members_strings_with_braces_and_all() {
        assert_eq!(top_members(FILE_TEXT).unwrap().members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), vec!["model", "env", "hooks"]);
        assert_eq!(top_members("[1]"), None);
        assert_eq!(top_members("{\"a\": 1,}"), None);
        assert_eq!(top_members("{\"a\" 1}"), None);
    }

    /// sets a value and leaves every other byte alone
    #[test]
    fn sets_a_value_and_leaves_every_other_byte_alone() {
        assert_eq!(set_key(FILE_TEXT, "model", Some("\"sonnet\"")), Some(FILE_TEXT.replacen("\"opus\"", "\"sonnet\"", 1)));
        assert_eq!(set_key("{\"a\":1,\"model\":\"x\"}\r\n", "model", Some("\"y\"")).as_deref(), Some("{\"a\":1,\"model\":\"y\"}\r\n"));
    }

    /// removes a key wherever it sits
    #[test]
    fn removes_a_key_wherever_it_sits() {
        assert_eq!(set_key(FILE_TEXT, "model", None).as_deref(), Some("{\n  \"env\": { \"A\": \"é ✓\" },\n  \"hooks\": {\"Stop\": [{\"x\": \"}\"}]}\n}\n"));
        assert_eq!(set_key(FILE_TEXT, "hooks", None).as_deref(), Some("{\n  \"model\": \"opus\",\n  \"env\": { \"A\": \"é ✓\" }\n}\n"));
        assert_eq!(set_key("{\n  \"model\": \"sonnet\"\n}\n", "model", None).as_deref(), Some("{}\n"));
        assert_eq!(set_key(FILE_TEXT, "missing", None).as_deref(), Some(FILE_TEXT));
    }

    /// adds a key back in front, indented like the rest
    #[test]
    fn adds_a_key_back_in_front_indented_like_the_rest() {
        assert_eq!(set_key("{\n  \"theme\": \"dark\"\n}", "model", Some("\"opus\"")).as_deref(), Some("{\n  \"model\": \"opus\",\n  \"theme\": \"dark\"\n}"));
        assert_eq!(set_key("{}", "model", Some("\"opus\"")).as_deref(), Some("{\n  \"model\": \"opus\"\n}"));
    }

    /// review: a key holding half a surrogate pair still leaves the file editable, as in the TS
    #[test]
    fn a_lone_surrogate_in_a_key_leaves_the_file_editable() {
        let text = "{\n  \"note \\ud83d\": 1,\n  \"model\": \"opus\"\n}\n";
        assert_eq!(set_key(text, "model", Some("\"sonnet\"")).as_deref(), Some("{\n  \"note \\ud83d\": 1,\n  \"model\": \"sonnet\"\n}\n"));
    }

    /// puts back only the keys that changed
    #[test]
    fn puts_back_only_the_keys_that_changed() {
        let map = |e: &[(&str, Option<&str>)]| e.iter().map(|(k, v)| (k.to_string(), v.map(str::to_owned))).collect::<IndexMap<_, _>>();
        let before = map(&[("model", Some("\"opus\"")), ("effortLevel", None), ("modelSettings", None)]);
        assert_eq!(restore_keys("{\n  \"model\": \"opus\",\n  \"theme\": \"dark\"\n}", &before), None);
        let after = "{\n  \"model\": \"sonnet\",\n  \"theme\": \"dark\",\n  \"modelSettings\": {\n    \"claude-sonnet-5-5\": {\n      \"effortLevel\": \"high\"\n    }\n  }\n}\n";
        assert_eq!(restore_keys(after, &before).as_deref(), Some("{\n  \"model\": \"opus\",\n  \"theme\": \"dark\"\n}\n"));
        // the same value written with other spacing is no change
        let spaced = map(&[("model", Some("\"opus\"")), ("modelSettings", Some("{ \"a\": { \"effortLevel\": \"low\" } }"))]);
        assert_eq!(restore_keys("{\"model\":\"opus\",\"modelSettings\":{\"a\":{\"effortLevel\":\"low\"}}}", &spaced), None);
    }

    #[test]
    fn the_read_script_output_is_decoded_from_hex() {
        assert_eq!(read_settings_output(0, "none\n", None), ReadSettings::Read(Settings { sum: String::new(), text: String::new() }));
        assert_eq!(read_settings_output(0, "123 9\n", Some("123 9")), ReadSettings::Same);
        assert_eq!(read_settings_output(1, "", None), ReadSettings::Unreadable);
        assert_eq!(read_settings_output(0, "1 2\n 7b 22 c3 a9\n 22 7d\n", None), ReadSettings::Read(Settings { sum: "1 2".into(), text: "{\"é\"}".into() }));
        assert_eq!(read_settings_output(0, "1 2\n ff fe\n", None), ReadSettings::Unreadable);
        assert!(read_settings_command(Some("a'b")).contains(r#"[ "$s" = 'a'\''b' ] && exit 0"#));
        assert!(write_settings_command("s").starts_with("f=\"${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json\"\nsum() {"));
    }

    mod on_the_machine {
        use std::os::unix::fs::PermissionsExt;

        use super::*;
        use crate::test_support::rt::{run, wait};
        use crate::test_support::sandbox::Sandbox;
        use crate::test_support::{host, TempDir};

        fn fast() -> KeepOptions {
            KeepOptions { keys: None, window_ms: Some(2500), grace_ms: Some(300), poll_ms: Some(50) }
        }

        /// A machine whose HOME is a fresh folder; its settings.json is in .claude there. With `bb`
        /// it runs the scripts with only busybox's tools, as Windows machines do (busybox-w32's sh);
        /// None when busybox isn't installed.
        fn setup(bb: bool) -> Option<(TempDir, Host)> {
            let dir = TempDir::new("inyon-keep-");
            std::fs::create_dir_all(dir.path(".claude")).unwrap();
            if bb {
                let busybox = ["/usr/bin/busybox", "/bin/busybox", "/usr/local/bin/busybox"].into_iter().find(|p| std::path::Path::new(p).exists())?;
                std::fs::create_dir_all(dir.path("busybox-bin")).unwrap();
                for t in ["sh", "cat", "cksum", "md5sum", "od", "tr", "cp", "mv", "rm"] {
                    std::os::unix::fs::symlink(busybox, dir.path(&format!("busybox-bin/{t}"))).unwrap();
                }
                Sandbox::install_with(&dir.s(), &format!("PATH={}; ", dir.path("busybox-bin").display()));
            } else {
                Sandbox::install(&dir.s());
            }
            Some((dir, host("keep-test", "local")))
        }

        const S: &str = ".claude/settings.json";

        /// What Claude Code does on /model sonnet: rewrites the whole file with its own formatting.
        fn claude_writes(dir: &TempDir, v: serde_json::Value) {
            dir.write(S, &json::stringify_pretty(&v));
        }

        fn parsed(dir: &TempDir) -> serde_json::Value {
            serde_json::from_str(&dir.read(S)).unwrap()
        }

        pub(super) fn puts_the_model_back_after_claude_code_saves_the_switch_keeping_the_rest_byte_for_byte(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                let original = "{\r\n    \"model\": \"opus\",\r\n    \"statusLine\": {\"type\": \"command\", \"command\": \"~/.inyon/statusline.sh\"},\r\n    \"note\": \"héllo ✓ 日本\"\r\n}\r\n";
                dir.write(S, original);
                std::fs::set_permissions(dir.path(S), std::fs::Permissions::from_mode(0o644)).unwrap();
                let done = keep_claude_defaults(&h, fast()).await;
                dir.write(S, &original.replace("\"opus\"", "\"sonnet\""));
                done.wait().await;
                assert_eq!(dir.read(S), original);
                assert_eq!(dir.mode(S), 0o644);
                assert!(!dir.exists(".claude/settings.json.inyon-tmp"));
            });
        }

        pub(super) fn removes_a_key_that_was_not_set_before_effort_included(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                dir.write(S, "{\n  \"theme\": \"dark\"\n}\n");
                let done = keep_claude_defaults(&h, fast()).await;
                claude_writes(&dir, serde_json::json!({ "theme": "dark", "model": "sonnet", "modelSettings": { "claude-sonnet-5-5": { "effortLevel": "high" } } }));
                done.wait().await;
                assert_eq!(parsed(&dir), serde_json::json!({ "theme": "dark" }));
            });
        }

        pub(super) fn a_settings_file_claude_code_creates_loses_only_those_keys(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                let done = keep_claude_defaults(&h, fast()).await;
                claude_writes(&dir, serde_json::json!({ "model": "sonnet" }));
                done.wait().await;
                assert_eq!(parsed(&dir), serde_json::json!({}));
            });
        }

        pub(super) fn a_later_write_by_claude_code_is_caught_too_and_a_second_switch_keeps_the_first_values(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                dir.write(S, "{\"model\":\"opus\"}");
                let done = keep_claude_defaults(&h, KeepOptions { grace_ms: Some(1500), ..fast() }).await;
                claude_writes(&dir, serde_json::json!({ "model": "sonnet" }));
                wait(400).await;
                assert_eq!(parsed(&dir)["model"], "opus");
                keep_claude_defaults(&h, fast()).await;
                claude_writes(&dir, serde_json::json!({ "model": "haiku", "effortLevel": "low" }));
                done.wait().await;
                assert_eq!(parsed(&dir), serde_json::json!({ "model": "opus" }));
            });
        }

        pub(super) fn writes_through_a_symlinked_settings_json(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                dir.write(".claude/dotfiles-settings.json", "{\"model\": \"opus\"}\n");
                std::os::unix::fs::symlink(dir.path(".claude/dotfiles-settings.json"), dir.path(S)).unwrap();
                let done = keep_claude_defaults(&h, fast()).await;
                dir.write(".claude/dotfiles-settings.json", "{\"model\": \"sonnet\"}\n");
                done.wait().await;
                assert!(std::fs::symlink_metadata(dir.path(S)).unwrap().file_type().is_symlink());
                assert_eq!(dir.read(".claude/dotfiles-settings.json"), "{\"model\": \"opus\"}\n");
            });
        }

        pub(super) fn leaves_a_file_it_cannot_read_as_json_alone(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                dir.write(S, "{\"model\": \"opus\", // comment\n}");
                let done = keep_claude_defaults(&h, fast()).await;
                done.wait().await;
                assert_eq!(dir.read(S), "{\"model\": \"opus\", // comment\n}");
            });
        }

        pub(super) fn nothing_changes_when_the_pick_was_for_this_session_only(bb: bool) {
            run(|| async {
                let Some((dir, h)) = setup(bb) else { return };
                dir.write(S, "{\"model\": \"opus\"}");
                let before = std::fs::metadata(dir.path(S)).unwrap().modified().unwrap();
                let done = keep_claude_defaults(&h, KeepOptions { window_ms: Some(400), ..fast() }).await;
                done.wait().await;
                assert_eq!(dir.read(S), "{\"model\": \"opus\"}");
                assert_eq!(std::fs::metadata(dir.path(S)).unwrap().modified().unwrap(), before);
            });
        }
    }

    #[test]
    fn puts_the_model_back_after_claude_code_saves_the_switch_keeping_the_rest_byte_for_byte_sh() {
        on_the_machine::puts_the_model_back_after_claude_code_saves_the_switch_keeping_the_rest_byte_for_byte(false);
    }

    #[test]
    fn puts_the_model_back_after_claude_code_saves_the_switch_keeping_the_rest_byte_for_byte_busybox() {
        on_the_machine::puts_the_model_back_after_claude_code_saves_the_switch_keeping_the_rest_byte_for_byte(true);
    }

    #[test]
    fn removes_a_key_that_was_not_set_before_effort_included_sh() {
        on_the_machine::removes_a_key_that_was_not_set_before_effort_included(false);
    }

    #[test]
    fn removes_a_key_that_was_not_set_before_effort_included_busybox() {
        on_the_machine::removes_a_key_that_was_not_set_before_effort_included(true);
    }

    #[test]
    fn a_settings_file_claude_code_creates_loses_only_those_keys_sh() {
        on_the_machine::a_settings_file_claude_code_creates_loses_only_those_keys(false);
    }

    #[test]
    fn a_settings_file_claude_code_creates_loses_only_those_keys_busybox() {
        on_the_machine::a_settings_file_claude_code_creates_loses_only_those_keys(true);
    }

    #[test]
    fn a_later_write_by_claude_code_is_caught_too_and_a_second_switch_keeps_the_first_values_sh() {
        on_the_machine::a_later_write_by_claude_code_is_caught_too_and_a_second_switch_keeps_the_first_values(false);
    }

    #[test]
    fn a_later_write_by_claude_code_is_caught_too_and_a_second_switch_keeps_the_first_values_busybox() {
        on_the_machine::a_later_write_by_claude_code_is_caught_too_and_a_second_switch_keeps_the_first_values(true);
    }

    #[test]
    fn writes_through_a_symlinked_settings_json_sh() {
        on_the_machine::writes_through_a_symlinked_settings_json(false);
    }

    #[test]
    fn writes_through_a_symlinked_settings_json_busybox() {
        on_the_machine::writes_through_a_symlinked_settings_json(true);
    }

    #[test]
    fn leaves_a_file_it_cannot_read_as_json_alone_sh() {
        on_the_machine::leaves_a_file_it_cannot_read_as_json_alone(false);
    }

    #[test]
    fn leaves_a_file_it_cannot_read_as_json_alone_busybox() {
        on_the_machine::leaves_a_file_it_cannot_read_as_json_alone(true);
    }

    #[test]
    fn nothing_changes_when_the_pick_was_for_this_session_only_sh() {
        on_the_machine::nothing_changes_when_the_pick_was_for_this_session_only(false);
    }

    #[test]
    fn nothing_changes_when_the_pick_was_for_this_session_only_busybox() {
        on_the_machine::nothing_changes_when_the_pick_was_for_this_session_only(true);
    }
}
