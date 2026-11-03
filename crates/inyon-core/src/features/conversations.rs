//! Port of features/conversations.ts.
//!
//! Past conversations of an agent in one folder, for the app's own picker: the agents' /resume
//! menus only work in their terminal. Everything is read on the machine through `host.exec`, so it
//! works the same for this machine, an SSH server, a joined machine or a phone's direct link.
//! Transcripts run to hundreds of megabytes, so only file times, the first lines and the last
//! stretch of each file are read, never a whole transcript.

use std::cell::RefCell;
use std::collections::HashSet;

use indexmap::IndexMap;
use inyon_protocol::{AgentKind, PastConversation};
use serde::Serialize;
use serde_json::Value;

use crate::agents::opencode_db::ocdb_sessions_of;
use crate::claude::actions::{agent_kind, is_session_id_str};
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::names::session_name;
use crate::hosts::registry::{registry, FoundPane};
use crate::js::text::JsStr;
use crate::js::to_number;
use crate::re;
use crate::tmux::tmux::claude_slug;

/// Agents whose conversations Inyon can list and reopen by id.
pub fn lists_conversations(kind: Option<AgentKind>) -> bool {
    kind.is_some_and(|k| k.def().conversations.as_ref().is_some_and(|c| !c.is_empty()))
}

const LIMIT: u32 = 50;
const TITLE: isize = 200;
const UUID: &str = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";

fn has_uuid(s: &str) -> bool {
    re!("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").is_match(s)
}

/// A JSON string's text from byte `start` (just past its opening quote) to its closing quote, or
/// to the end of `s` when the line was cut short on the machine.
pub fn json_string_at(s: &str, start: usize) -> String {
    let units: Vec<u16> = s.get(start..).unwrap_or("").encode_utf16().collect();
    let hex4 = |from: usize| -> Option<u16> {
        let h = units.get(from..from + 4)?;
        let text = String::from_utf16(h).ok()?;
        if text.len() != 4 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u16::from_str_radix(&text, 16).ok()
    };
    let mut out: Vec<u16> = vec![];
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        if c == b'"' as u16 {
            return String::from_utf16_lossy(&out);
        }
        if c != b'\\' as u16 {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&n) = units.get(i) else { break };
        if n == b'u' as u16 {
            let Some(code) = hex4(i + 1) else { break };
            out.push(code);
            i += 4;
        } else {
            let mapped = match char::from_u32(n as u32) {
                Some('n') => '\n' as u16,
                Some('t') => '\t' as u16,
                Some('r') => '\r' as u16,
                Some('b') => '\u{8}' as u16,
                Some('f') => '\u{c}' as u16,
                _ => n,
            };
            out.push(mapped);
        }
        i += 1;
    }
    String::from_utf16_lossy(&out)
}

/// The value of the first `"key":"…"` in a line of JSON, which may be cut short.
pub fn field(line: &str, key: &str) -> Option<String> {
    let at = line.find(&format!("\"{key}\":\""))?;
    Some(json_string_at(line, at + key.len() + 4))
}

/// A prompt as a one-line title: no tags the agent wrapped around it, no runs of spaces.
pub fn one_line(text: &str) -> String {
    // `<(system-reminder|local-command-caveat)>[\s\S]*?(<\/\1>|$)` without the back reference
    let t = re!(r"<system-reminder>[\s\S]*?(?:<\/system-reminder>|$)|<local-command-caveat>[\s\S]*?(?:<\/local-command-caveat>|$)").replace_all(text, " ");
    let t = re!(r"<\/?[a-z][\w-]*(\s[^>]*)?>", "gi").replace_all(&t, " ");
    let t = re!(r"\s+").replace_all(&t, " ");
    t.jtrim().to16(TITLE).to_owned()
}

fn mtime_of(f: &str) -> String {
    format!("\"$(stat -c %Y {f} 2>/dev/null || stat -f %m {f} 2>/dev/null)\"")
}

/// `Number(t) * 1000 || 0` for a time in seconds a command printed.
fn ms_of(t: &str) -> u64 {
    let n = to_number(t) * 1000.0;
    if n.is_finite() && n > 0.0 {
        n as u64
    } else {
        0
    }
}

// ---------- Claude Code ----------

/// ~/.claude/projects/<folder>/<id>.jsonl. A /rename is written as a "custom-title" line and Claude
/// names each conversation itself ("ai-title"); both are repeated as it goes on, so the last part
/// of the file has the latest. The first prompt is near the top, after a few bookkeeping lines.
pub fn claude_script(cwd: &str) -> String {
    format!(
        r#"cd "$HOME/.claude/projects/"{} 2>/dev/null || exit 0
ls -t -- *.jsonl 2>/dev/null | head -n 80 | while IFS= read -r f; do
  printf '\036%s\t%s\n' {} "$f"
  echo '@@T'; tail -c 262144 "$f" 2>/dev/null | grep -E '"type":"(custom-title|ai-title)"' | tail -n 12 | cut -c1-1200
  echo '@@U'; head -c 131072 "$f" 2>/dev/null | grep -F '"type":"user"' | head -n 8 | cut -c1-2400
done"#,
        shq(&claude_slug(cwd)),
        mtime_of("\"$f\"")
    )
}

/// The user's own words in a Claude "user" line, or nothing for a command, a tool result or the like.
fn claude_prompt(line: &str) -> Option<String> {
    if !line.contains("\"type\":\"user\"") || re!(r#""isMeta":true|"isSidechain":true"#).is_match(line) {
        return None;
    }
    let from = line.find("\"message\":").unwrap_or(0);
    let at = from + line[from..].find("\"content\":")?;
    let rest = &line[at + 10..];
    let text = if rest.starts_with('"') {
        Some(json_string_at(rest, 1))
    } else if rest.starts_with(r#"[{"type":"text""#) {
        field(rest, "text")
    } else {
        None
    };
    let text = text.filter(|t| !t.is_empty())?;
    if re!(r"<(command-name|local-command-stdout|local-command-stderr)>|^\s*Caveat:").is_match(&text) {
        return None;
    }
    Some(one_line(&text)).filter(|t| !t.is_empty())
}

pub fn parse_claude(stdout: &str) -> Vec<PastConversation> {
    let mut out = vec![];
    for block in stdout.split('\u{1e}').skip(1) {
        let mut lines = block.split('\n');
        let head = lines.next().unwrap_or("");
        let mut cols = head.split('\t');
        let t = cols.next().unwrap_or("");
        let Some(file) = cols.next() else { continue };
        let id = re!(r"\.jsonl$").replace(file, "").into_owned();
        if id.is_empty() || !has_uuid(&id) {
            continue;
        }
        let mut section = "";
        let (mut custom, mut ai, mut prompt): (Option<String>, Option<String>, Option<String>) = (None, None, None);
        for l in lines {
            if l == "@@T" || l == "@@U" {
                section = if l == "@@T" { "@@T" } else { "@@U" };
            } else if section == "@@T" {
                if l.starts_with(r#"{"type":"custom-title""#) {
                    custom = field(l, "customTitle").or(custom);
                } else if l.starts_with(r#"{"type":"ai-title""#) {
                    ai = field(l, "aiTitle").or(ai);
                }
            } else if section == "@@U" && prompt.is_none() {
                prompt = claude_prompt(l);
            }
        }
        // nothing was said: Claude's own picker leaves these out, and they can't be resumed
        let title = [one_line(custom.as_deref().unwrap_or("")), one_line(ai.as_deref().unwrap_or(""))].into_iter().find(|t| !t.is_empty()).or(prompt);
        if let Some(title) = title.filter(|t| !t.is_empty()) {
            out.push(PastConversation { id, title, at: ms_of(t), current: None });
        }
    }
    out
}

// ---------- Codex ----------

/// ~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<thread>.jsonl, whose first line (session_meta)
/// names the folder; newest first, until enough are in this folder. Only threads Codex's own picker
/// shows (started in its terminal or editor, not `codex exec` or a subagent). Names are in
/// session_index.jsonl, the latest line for a thread winning.
pub fn codex_script(cwd: &str) -> String {
    let pat = format!("\"cwd\":{}", crate::js::stringify(&Value::String(cwd.to_owned())));
    format!(
        r#"r="${{CODEX_HOME:-$HOME/.codex}}"; s="$r/sessions"
[ -d "$s" ] || exit 0
p={}
if find "$s" -maxdepth 0 -printf '' >/dev/null 2>&1; then find "$s" -name 'rollout-*.jsonl' -printf '%T@\t%p\n' 2>/dev/null
else find "$s" -name 'rollout-*.jsonl' 2>/dev/null | while IFS= read -r f; do printf '%s\t%s\n' {} "$f"; done; fi | sort -rn | head -n 2000 | {{
n=0
while IFS='	' read -r t f; do
  h=$(head -c 4000 "$f" 2>/dev/null | head -n 1)
  case "$h" in *"$p"*) ;; *) continue;; esac
  case "$h" in *'"source":"cli"'*|*'"source":"vscode"'*) ;; *'"source":'*) continue;; esac
  printf '\036%s\t%s\n' "${{t%.*}}" "$f"
  head -c 262144 "$f" 2>/dev/null | grep -F '"role":"user"' | grep -v -E '"text":"[[:space:]]*<(environment_context|user_instructions|user_shell_command|codex_internal_context)' | head -n 1 | cut -c1-2400
  n=$((n+1)); [ $n -ge 80 ] && break
done; }}
printf '\036@@INDEX\n'; tail -n 4000 "$r/session_index.jsonl" 2>/dev/null"#,
        shq(&pat),
        mtime_of("\"$f\"")
    )
}

pub fn parse_codex(stdout: &str) -> Vec<PastConversation> {
    let blocks: Vec<&str> = stdout.split('\u{1e}').skip(1).collect();
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(index) = blocks.iter().find(|b| b.starts_with("@@INDEX")) {
        for l in index.split('\n') {
            let Some(e) = crate::js::json::parse(l) else { continue };
            if let (Some(id), Some(name)) = (e.get("id").and_then(Value::as_str), e.get("thread_name").and_then(Value::as_str)) {
                if !id.is_empty() && !name.is_empty() {
                    names.insert(id.to_owned(), name.to_owned());
                }
            }
        }
    }
    let file = crate::js::re::dynamic(&format!("-({UUID})(_{UUID})?\\.jsonl$"), "");
    let mut seen: IndexMap<String, PastConversation> = IndexMap::new();
    for block in &blocks {
        if block.starts_with("@@INDEX") {
            continue;
        }
        let mut lines = block.split('\n');
        let head = lines.next().unwrap_or("");
        let line = lines.next().unwrap_or("");
        let mut cols = head.split('\t');
        let t = cols.next().unwrap_or("");
        let Some(path) = cols.next() else { continue };
        // "rollout-<time>-<thread>.jsonl", or "…-<thread>_<segment>.jsonl" after a rewind
        let name = path.rsplit('/').next().unwrap_or("");
        let Some(m) = file.captures(name) else { continue };
        let id = m[1].to_owned();
        let segment = m.get(2).is_some();
        let prompt = one_line(field(line, "text").as_deref().unwrap_or(""));
        let at = ms_of(t);
        // a segment starts partway through: the thread's first file has the prompt it began with
        if let Some(was) = seen.get_mut(&id) {
            was.at = was.at.max(at);
            if !prompt.is_empty() && (was.title.is_empty() || !segment) {
                was.title = prompt;
            }
        } else {
            seen.insert(id.clone(), PastConversation { id, title: prompt, at, current: None });
        }
    }
    seen.into_values()
        .map(|mut c| {
            let named = one_line(names.get(&c.id).map(String::as_str).unwrap_or(""));
            if !named.is_empty() {
                c.title = named;
            }
            c
        })
        .filter(|c| !c.title.is_empty())
        .collect()
}

// ---------- GitHub Copilot ----------

/// ~/.copilot/session-state/<id>/: workspace.yaml has the folder and the session's name (Copilot
/// names it from the first prompt, or /rename does); events.jsonl is written as it goes, so its
/// time is the last activity. Without a workspace.yaml, the first event names the folder.
pub fn copilot_script() -> String {
    format!(
        r#"s="$HOME/.copilot/session-state"
[ -d "$s" ] || exit 0
ls -t -- "$s"/*/events.jsonl 2>/dev/null | head -n 400 | while IFS= read -r f; do
  d=${{f%/events.jsonl}}
  printf '\036%s\t%s\n' {} "${{d##*/}}"
  if [ -f "$d/workspace.yaml" ]; then grep -E '^(cwd|name):' "$d/workspace.yaml" | cut -c1-1200; else head -c 4000 "$f" | head -n 1; fi
  grep -qs '^name:' "$d/workspace.yaml" || head -c 131072 "$f" 2>/dev/null | grep -F '"type":"user.message"' | head -n 1 | cut -c1-2400
done"#,
        mtime_of("\"$f\"")
    )
}

/// A one-line YAML scalar: plain, 'single' or "double" quoted.
pub fn yaml_scalar(v: &str) -> String {
    let s = v.jtrim();
    if s.starts_with('"') {
        return match crate::js::json::parse(s) {
            Some(Value::String(t)) => t,
            _ => json_string_at(s, 1),
        };
    }
    if let Some(inner) = s.strip_prefix('\'') {
        let inner = if s.ends_with('\'') && s.len() > 1 { &inner[..inner.len() - 1] } else { inner };
        return inner.replace("''", "'");
    }
    re!(r"\s+#.*$").replace(s, "").into_owned()
}

pub fn parse_copilot(stdout: &str, cwd: &str) -> Vec<PastConversation> {
    let mut out = vec![];
    for block in stdout.split('\u{1e}').skip(1) {
        let mut lines = block.split('\n');
        let head = lines.next().unwrap_or("");
        let mut cols = head.split('\t');
        let t = cols.next().unwrap_or("");
        let Some(id) = cols.next() else { continue };
        if !has_uuid(id) {
            continue;
        }
        let (mut dir, mut name, mut prompt): (Option<String>, Option<String>, Option<String>) = (None, None, None);
        for l in lines {
            if let Some(rest) = l.strip_prefix("cwd:") {
                dir = Some(yaml_scalar(rest));
            } else if let Some(rest) = l.strip_prefix("name:") {
                name = Some(yaml_scalar(rest));
            } else if l.contains(r#""type":"session.start""#) {
                dir = field(l, "cwd");
            } else if l.contains(r#""type":"user.message""#) && prompt.is_none() {
                prompt = field(l, "content");
            }
        }
        if dir.as_deref() != Some(cwd) {
            continue;
        }
        let title = Some(one_line(name.as_deref().unwrap_or(""))).filter(|t| !t.is_empty()).unwrap_or_else(|| one_line(prompt.as_deref().unwrap_or("")));
        if !title.is_empty() {
            out.push(PastConversation { id: id.to_owned(), title, at: ms_of(t), current: None });
        }
    }
    out
}

// ---------- SQLite (OpenCode, Antigravity) ----------

/// Rows of a read-only query on a database on the machine: sqlite3, else Python's sqlite3.
/// `db` is shell text (it may name $HOME).
pub async fn sqlite_rows(host: &Host, db: &str, sql: &str) -> HubResult<Vec<Value>> {
    let r = host
        .exec(
            &format!(
                r#"db={db}; Q={}
[ -f "$db" ] || exit 0
if command -v sqlite3 >/dev/null 2>&1 && out=$(sqlite3 -readonly -json "$db" "$Q" 2>/dev/null); then printf '%s' "$out"
elif command -v python3 >/dev/null 2>&1; then python3 -c 'import sqlite3,json,sys
c=sqlite3.connect("file:"+sys.argv[1]+"?mode=ro",uri=True)
c.row_factory=sqlite3.Row
print(json.dumps([dict(r) for r in c.execute(sys.argv[2])]))' "$db" "$Q"; fi"#,
                shq(sql)
            ),
            ExecOptions::timeout(15_000),
        )
        .await?;
    let text = r.stdout.jtrim();
    Ok(match crate::js::json::parse(if text.is_empty() { "[]" } else { text }) {
        Some(Value::Array(rows)) => rows,
        _ => vec![],
    })
}

fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// "2026-10-11 08:22:33.43735957+00:00", as agy's database writes times.
pub fn agy_time(s: Option<&Value>) -> u64 {
    let raw = match s {
        None | Some(Value::Null) => String::new(),
        Some(v) => crate::js::json::value_string(v),
    };
    let iso = re!(r"(\.\d{3})\d+").replace(&raw.replacen(' ', "T", 1), "$1").into_owned();
    let t = crate::js::date::parse(&iso);
    if t.is_finite() && t > 0.0 {
        t as u64
    } else {
        0
    }
}

/// `encodeURI`: everything but letters, digits and `;,/?:@&=+$-_.!~*'()#` as %XX of its UTF-8.
fn encode_uri(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b";,/?:@&=+$-_.!~*'()#".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `decodeURI`: %XX runs back to text, except those that stand for `;/?:@&=+$,#`; None where
/// JavaScript throws (a broken escape or bytes that aren't UTF-8).
fn decode_uri(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let hex = |i: usize| -> Option<u8> { std::str::from_utf8(b.get(i + 1..i + 3)?).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            let c = s[i..].chars().next()?;
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        let first = hex(i)?;
        let len = match first {
            0x00..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return None,
        };
        let mut bytes = vec![first];
        for k in 1..len {
            let at = i + 3 * k;
            if b.get(at) != Some(&b'%') {
                return None;
            }
            bytes.push(hex(at)?);
        }
        let text = String::from_utf8(bytes).ok()?;
        if len == 1 && ";/?:@&=+$,#".contains(text.as_str()) {
            out.push_str(&s[i..i + 3]);
        } else {
            out.push_str(&text);
        }
        i += 3 * len;
    }
    Some(out)
}

/// agy keeps a summary of each conversation (title, first prompt as "preview", the folders it
/// worked in) in ~/.gemini/antigravity-cli/conversation_summaries.db. Subagents' conversations
/// are left out.
pub async fn agy_conversations(host: &Host, cwd: &str, limit: u32) -> HubResult<Vec<PastConversation>> {
    let mut uris = vec![format!("file://{cwd}")];
    let encoded = format!("file://{}", encode_uri(cwd));
    if !uris.contains(&encoded) {
        uris.push(encoded);
    }
    let matches: Vec<String> = uris.iter().map(|u| format!("instr(workspace_uris, {}) > 0", sql_text(&crate::js::stringify(&Value::String(u.clone()))))).collect();
    let rows = sqlite_rows(
        host,
        "\"$HOME/.gemini/antigravity-cli/conversation_summaries.db\"",
        &format!(
            "select conversation_id as id, title, preview, last_modified_time as t, workspace_uris as w from conversation_summaries where parent_conversation_id = '' and nesting_depth = 0 and ({}) order by last_modified_time desc limit {limit}",
            matches.join(" or ")
        ),
    )
    .await?;
    let text = |v: Option<&Value>| match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => crate::js::json::value_string(v),
    };
    let truthy = |v: Option<&Value>| match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(_) => true,
    };
    Ok(rows
        .iter()
        .filter_map(|r| {
            let dirs: Vec<String> = r
                .get("w")
                .and_then(Value::as_str)
                .and_then(crate::js::json::parse)
                .and_then(|w| w.as_array().cloned())
                .and_then(|list| list.iter().map(|u| u.as_str().and_then(|u| decode_uri(&re!(r"^file:\/\/").replace(u, "")))).collect::<Option<Vec<String>>>())
                .unwrap_or_default();
            let id = text(r.get("id"));
            let raw = if truthy(r.get("title")) {
                text(r.get("title"))
            } else if truthy(r.get("preview")) {
                text(r.get("preview"))
            } else {
                String::new()
            };
            let title = one_line(&raw);
            (dirs.iter().any(|d| d == cwd) && is_session_id_str(Some(&id), Some(AgentKind::Antigravity)) && !title.is_empty()).then(|| PastConversation {
                id,
                title,
                at: agy_time(r.get("t")),
                current: None,
            })
        })
        .collect())
}

async fn opencode_conversations(host: &Host, cwd: &str, limit: u32) -> HubResult<Vec<PastConversation>> {
    let rows = ocdb_sessions_of(host, cwd, limit).await?;
    Ok(rows
        .into_iter()
        .filter(|r| is_session_id_str(Some(&r.id), Some(AgentKind::Opencode)) && !r.id.is_empty())
        .map(|r| {
            let title = one_line(r.title.as_deref().unwrap_or(""));
            let at = if r.time_updated.is_finite() && r.time_updated > 0.0 { r.time_updated as u64 } else { 0 };
            PastConversation { id: r.id, title: if title.is_empty() { "Untitled".into() } else { title }, at, current: None }
        })
        .collect())
}

// ---------- all agents ----------

/// The agent's past conversations in `cwd` on the machine, newest first (at most `limit`, 50 in the TS).
pub async fn list_conversations(host: &Host, kind: AgentKind, cwd: &str, limit: u32) -> HubResult<Vec<PastConversation>> {
    let list = match kind {
        AgentKind::Claude => parse_claude(&host.exec(&claude_script(cwd), ExecOptions::timeout(30_000)).await?.stdout),
        AgentKind::Codex => parse_codex(&host.exec(&codex_script(cwd), ExecOptions::timeout(30_000)).await?.stdout),
        AgentKind::Copilot => parse_copilot(&host.exec(&copilot_script(), ExecOptions::timeout(30_000)).await?.stdout, cwd),
        AgentKind::Opencode => opencode_conversations(host, cwd, limit).await?,
        AgentKind::Antigravity => agy_conversations(host, cwd, limit).await?,
        _ => return Err(http(400, format!("Inyon can't list {}'s conversations", kind.def().name))),
    };
    let mut list: Vec<PastConversation> = list.into_iter().filter(|c| is_session_id_str(Some(&c.id), Some(kind)) && !c.id.is_empty()).collect();
    list.sort_by_key(|c| std::cmp::Reverse(c.at));
    list.truncate(limit as usize);
    Ok(list)
}

/// `asked` as the TS reads it: missing, the empty string, or something that must name an agent.
fn asked_kind(asked: Option<&Value>) -> HubResult<Option<AgentKind>> {
    match asked {
        None => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => agent_kind(s).map(Some).ok_or_else(|| http(400, "Unknown agent")),
        Some(_) => Err(http(400, "Unknown agent")),
    }
}

/// Which agent a pane's conversations are for: the one running there, else the one asked for.
fn pane_agent(host_id: &str, pane_id: &str, asked: Option<&Value>) -> HubResult<(FoundPane, AgentKind)> {
    let found = registry().find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    let running = found.pane.agent.as_ref().map(|a| a.kind);
    let asked = asked_kind(asked)?;
    let kind = running.or(asked).ok_or_else(|| http(400, "Say which agent to list conversations for"))?;
    if !lists_conversations(Some(kind)) {
        return Err(http(400, format!("Inyon can't list {}'s conversations yet. Use its own command in the terminal.", kind.def().name)));
    }
    Ok((found, kind))
}

/// What `paneConversations` answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaneConversations {
    pub agent: AgentKind,
    pub cwd: String,
    pub conversations: Vec<PastConversation>,
}

/// Past conversations in the pane's working folder, with the open one marked. `asked` is the
/// request's `agent` with a falsy one already made None (`q(req, 'agent') || undefined`).
pub async fn pane_conversations(host_id: &str, pane_id: &str, asked: Option<&Value>) -> HubResult<PaneConversations> {
    let (found, kind) = pane_agent(host_id, pane_id, asked)?;
    let cwd = registry().agent_proc(host_id, pane_id).and_then(|p| p.cwd).unwrap_or_else(|| found.pane.cwd.clone());
    if cwd.is_empty() {
        return Err(http(409, "The pane's working folder isn't known yet"));
    }
    let list = list_conversations(&registry().host(host_id)?, kind, &cwd, LIMIT).await?;
    let current = found.pane.agent.as_ref().filter(|a| a.kind == kind).and_then(|a| a.session_id.clone());
    let conversations = list
        .into_iter()
        .map(|c| {
            let title = session_name(host_id, Some(&c.id), Some(kind)).unwrap_or(c.title);
            let current = (current.as_deref() == Some(c.id.as_str())).then_some(true);
            PastConversation { id: c.id, title, at: c.at, current }
        })
        .collect();
    Ok(PaneConversations { agent: kind, cwd, conversations })
}

thread_local! {
    static SWITCHING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// What a switch does (`switchSteps`), swapped out in tests.
mod steps {
    use super::*;

    #[cfg(test)]
    pub(crate) type Step = std::rc::Rc<dyn Fn(&str, &str, AgentKind, &str) -> futures_util::future::LocalBoxFuture<'static, HubResult<()>>>;

    #[cfg(test)]
    thread_local! {
        pub(crate) static EXIT: RefCell<Option<Step>> = const { RefCell::new(None) };
        pub(crate) static READY: RefCell<Option<Step>> = const { RefCell::new(None) };
        pub(crate) static START: RefCell<Option<Step>> = const { RefCell::new(None) };
    }

    #[cfg(test)]
    fn swapped(which: &'static std::thread::LocalKey<RefCell<Option<Step>>>) -> Option<Step> {
        which.with(|s| s.borrow().clone())
    }

    pub async fn exit(host_id: &str, pane_id: &str, kind: AgentKind, id: &str) -> HubResult<()> {
        #[cfg(test)]
        if let Some(f) = swapped(&EXIT) {
            return f(host_id, pane_id, kind, id).await;
        }
        let _ = (kind, id);
        crate::claude::actions::exit_agent(host_id, pane_id, false).await
    }

    pub async fn ready(host_id: &str, pane_id: &str, kind: AgentKind, id: &str) -> HubResult<()> {
        #[cfg(test)]
        if let Some(f) = swapped(&READY) {
            return f(host_id, pane_id, kind, id).await;
        }
        let _ = (kind, id);
        crate::claude::actions::shell_ready(&registry().host(host_id)?, pane_id, 8000).await
    }

    pub async fn start(host_id: &str, pane_id: &str, kind: AgentKind, id: &str) -> HubResult<()> {
        #[cfg(test)]
        if let Some(f) = swapped(&START) {
            return f(host_id, pane_id, kind, id).await;
        }
        crate::claude::actions::type_start(host_id, pane_id, kind, Some(id)).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Switched {
    pub switched: bool,
}

/// Opens another conversation in the pane. A running agent is asked to exit the way /exit would
/// (a running turn is stopped first, so the conversation it leaves is saved) and the pane stays
/// open; then the agent starts again with that conversation. `asked` is the body's `agent` with a
/// falsy one made None (`b.agent || undefined`); `id` the body's `id` as it came.
pub async fn switch_conversation(host_id: &str, pane_id: &str, asked: Option<&Value>, id: Option<&Value>) -> HubResult<Switched> {
    let (found, kind) = pane_agent(host_id, pane_id, asked)?;
    let id = match id {
        Some(Value::String(s)) if !s.is_empty() && is_session_id_str(Some(s), Some(kind)) => s.clone(),
        _ => return Err(http(400, "Bad conversation id")),
    };
    let running = found.pane.agent.clone();
    let asked = asked_kind(asked)?;
    if let (Some(r), Some(a)) = (&running, asked) {
        if a != r.kind {
            return Err(http(409, format!("{} is running here, not {}", r.kind.def().name, a.def().name)));
        }
    }
    if running.as_ref().and_then(|r| r.session_id.as_deref()) == Some(id.as_str()) {
        return Ok(Switched { switched: false });
    }
    let key = crate::js::stringify(&serde_json::json!([host_id, pane_id]));
    if !SWITCHING.with(|s| s.borrow_mut().insert(key.clone())) {
        return Err(http(409, "Already switching conversations in this session"));
    }
    let result = async {
        if running.is_some() {
            steps::exit(host_id, pane_id, kind, &id).await?;
            steps::ready(host_id, pane_id, kind, &id).await?;
        }
        steps::start(host_id, pane_id, kind, &id).await
    }
    .await;
    SWITCHING.with(|s| s.borrow_mut().remove(&key));
    result?;
    Ok(Switched { switched: true })
}

#[cfg(test)]
#[path = "conversations_tests.rs"]
mod tests;
