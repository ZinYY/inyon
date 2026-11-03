//! Port of agents/opencode-db.ts: reading OpenCode's SQLite database.
//!
//! OpenCode started without --port has no HTTP server: its TUI talks to its engine in-process.
//! What it does still lands in its SQLite database (session, message, part, todo), as the same
//! JSON its server API returns, so Inyon reads that. Read-only, and only those tables: the same
//! file holds account tokens, which are never queried.
//!
//! Every query asks SQLite for its rows as one JSON array (json_group_array), printed as is. The
//! queries and the script that runs them are built and read by pure functions; the async ones at
//! the end run them on the machine.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{self, Js};
use crate::js::text::locale_compare;
use crate::js::JsStr;
use crate::re;

/// where OpenCode installs itself (the hub's own PATH is minimal)
pub const OC_PATH: &str = r#"export PATH="$HOME/.opencode/bin:$HOME/.local/bin:$HOME/.bun/bin:$HOME/.npm-global/bin:/opt/homebrew/bin:/usr/local/bin:$PATH""#;

/// sqlite3 answers in a few ms; Python's sqlite3 module is the fallback, `opencode db` (2-3 s per
/// call) the last resort. `$Q` is the query.
fn script() -> String {
    format!(
        r#"
db="${{XDG_DATA_HOME:-$HOME/.local/share}}/opencode/opencode.db"
[ -f "$db" ] || {{ echo '@@NODB'; exit 0; }}
if command -v sqlite3 >/dev/null 2>&1 && out=$(sqlite3 -readonly -noheader -list "$db" "$Q" 2>/dev/null); then printf '%s' "$out"
elif command -v python3 >/dev/null 2>&1; then
  python3 -c 'import sqlite3,sys
c=sqlite3.connect("file:"+sys.argv[1]+"?mode=ro",uri=True)
print(c.execute(sys.argv[2]).fetchone()[0])' "$db" "$Q"
else {OC_PATH}; opencode db "$Q" --format json 2>/dev/null; fi
"#
    )
}

/// The rows of `from` as one JSON array built by SQLite, with these columns.
pub fn json_rows(columns: &[&str], from: &str) -> String {
    let cols = columns.iter().map(|c| format!("'{c}', {c}")).collect::<Vec<_>>().join(", ");
    format!("select json_group_array(json_object({cols})) as rows from ({from})")
}

pub const OCDB: &str = "opencode-db:";

fn session_id_ok(id: &str) -> bool {
    re!(r"^ses_[A-Za-z0-9]+$").is_match(id)
}

pub fn is_ocdb(uri: Option<&str>) -> bool {
    uri.is_some_and(|u| u.starts_with(OCDB))
}

pub fn ocdb_session(uri: &str) -> Option<&str> {
    let id = uri.get(OCDB.len()..)?;
    session_id_ok(id).then_some(id)
}

/// The command that runs one `json_rows` query (give it 15 s).
pub fn ocdb_query_command(sql: &str) -> String {
    format!("Q={}\n{}", shq(sql), script())
}

/// The rows from what that command printed; none for no database or anything unreadable.
pub fn ocdb_query_rows(stdout: &str) -> Vec<Value> {
    let out = stdout.jtrim();
    if out.is_empty() || out == "@@NODB" {
        return vec![];
    }
    let Some(mut rows) = json::parse(out) else { return vec![] };
    // `opencode db` prints the query's result set: one row whose `rows` column holds the array
    if let Some(inner) = rows.as_array().filter(|a| a.len() == 1).and_then(|a| a[0].get("rows")).and_then(Value::as_str) {
        match json::parse(inner) {
            Some(v) => rows = v,
            None => return vec![],
        }
    }
    match rows {
        Value::Array(a) => a,
        _ => vec![],
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A top-level conversation, as `ocdb_sessions_query` lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct OcdbSessionRow {
    pub id: String,
    pub title: Option<String>,
    pub directory: Option<String>,
    pub parent_id: Option<String>,
    pub time_created: f64,
    pub time_updated: f64,
    pub agent: Option<String>,
    pub model: Option<String>,
}

/// Top-level conversations in a folder, newest activity first.
pub fn ocdb_sessions_query(directory: &str, limit: u32) -> String {
    json_rows(
        &["id", "title", "directory", "parent_id", "time_created", "time_updated", "agent", "model"],
        &format!(
            "select id, title, directory, parent_id, time_created, time_updated, agent, model from session where directory = {} and parent_id is null and time_archived is null order by time_updated desc limit {limit}",
            quote(directory)
        ),
    )
}

/// The rows that query returned, newest activity first.
pub fn ocdb_sessions(rows: &[Value]) -> Vec<OcdbSessionRow> {
    let s = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).map(str::to_owned);
    let n = |r: &Value, k: &str| r.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let mut out: Vec<OcdbSessionRow> = rows
        .iter()
        .map(|r| OcdbSessionRow {
            id: s(r, "id").unwrap_or_default(),
            title: s(r, "title"),
            directory: s(r, "directory"),
            parent_id: s(r, "parent_id"),
            time_created: n(r, "time_created"),
            time_updated: n(r, "time_updated"),
            agent: s(r, "agent"),
            model: s(r, "model"),
        })
        .collect();
    out.sort_by(|a, b| b.time_updated.total_cmp(&a.time_updated));
    out
}

/// An OpenCode process as the session picker sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenCodeProc {
    pub cwd: Option<String>,
    /// unix seconds
    pub started_at: f64,
    pub args: Option<String>,
}

/// The conversation an OpenCode TUI shows, from the folder's conversations (`rows`, read with
/// `ocdb_sessions_query` for the process's folder). Its terminal title names it, `-s <id>` too;
/// otherwise it's the folder's conversation whose title is on screen (the TUI prints it), else
/// the one that has moved since the process started. A TUI that hasn't sent anything yet has
/// none: "opencode-db:" alone. None without a folder; the rows aren't needed when the title is
/// "OpenCode".
pub fn guess_open_code_session(proc: &OpenCodeProc, rows: &[OcdbSessionRow], screen: Option<&[String]>, pane_title: Option<&str>) -> Option<String> {
    proc.cwd.as_deref().filter(|c| !c.is_empty())?;
    let args = proc.args.as_deref().unwrap_or("");
    // the TUI sets the terminal title to "OC | <conversation title>", or "OpenCode" on its home screen
    if pane_title.map(JsStr::jtrim) == Some("OpenCode") {
        return Some(OCDB.into());
    }
    let shown = pane_title.and_then(|t| re!(r"^OC \| (.+)$").captures(t)).map(|m| m[1].jtrim().to_owned()).filter(|s| !s.is_empty());
    if let Some(named) = shown.and_then(|shown| rows.iter().find(|r| r.title.as_deref() == Some(shown.as_str()))) {
        return Some(format!("{OCDB}{}", named.id));
    }
    if let Some(explicit) = re!(r"(?:^|\s)(?:-s|--session)[=\s](ses_[A-Za-z0-9]+)").captures(args) {
        return Some(format!("{OCDB}{}", &explicit[1]));
    }
    let text = screen.map(|s| s.join("\n")).unwrap_or_default();
    let titled: Vec<&OcdbSessionRow> =
        rows.iter().filter(|r| r.title.as_deref().is_some_and(|t| t.len16() >= 6 && !re!(r"^New session - ").is_match(t) && text.contains(t.slice16(0, Some(40))))).collect();
    if titled.len() == 1 {
        return Some(format!("{OCDB}{}", titled[0].id));
    }
    if let Some(since) = rows.iter().find(|r| r.time_updated >= proc.started_at * 1000.0 - 5000.0) {
        return Some(format!("{OCDB}{}", since.id));
    }
    // `-c` continues the folder's latest conversation
    if re!(r"(?:^|\s)(?:-c|--continue)(?:\s|$)").is_match(args) {
        if let Some(first) = rows.first() {
            return Some(format!("{OCDB}{}", first.id));
        }
    }
    Some(OCDB.into())
}

/// Change marker: the newest write to the conversation's messages or parts.
pub fn ocdb_stamp_query(session: &str) -> Option<String> {
    session_id_ok(session).then(|| {
        let s = quote(session);
        json_rows(
            &["t"],
            &format!("select max(t) as t from (select max(time_updated) as t from message where session_id = {s} union all select max(time_updated) from part where session_id = {s} union all select time_updated from session where id = {s})"),
        )
    })
}

pub fn ocdb_stamp(rows: &[Value]) -> f64 {
    let t = rows.first().and_then(|r| r.get("t"));
    let n = match t {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => crate::js::to_number(s),
        Some(Value::Bool(b)) => *b as i64 as f64,
        _ => 0.0,
    };
    if n.is_nan() {
        0.0
    } else {
        n
    }
}

/// A message's `summary` is the turn's whole diff (megabytes after a lock file changed), and
/// nothing here shows it: it stays in the database.
const DATA: &str = "case when json_valid(data) then json_remove(data, '$.summary') else data end as data";

/// The two queries for the messages and parts of a conversation written since `since` (callers
/// merge them into what they already hold), in the server API's shape.
pub fn ocdb_rows_queries(session: &str, since: f64) -> Option<(String, String)> {
    if !session_id_ok(session) {
        return None;
    }
    let s = quote(session);
    let since = since.floor();
    let since = crate::js::fmt_number(since);
    Some((
        json_rows(
            &["id", "time_created", "time_updated", "data"],
            &format!("select id, time_created, time_updated, {DATA} from message where session_id = {s} and time_updated >= {since} order by time_created, id"),
        ),
        json_rows(
            &["id", "message_id", "time_created", "time_updated", "data"],
            &format!("select id, message_id, time_created, time_updated, data from part where session_id = {s} and time_updated >= {since} order by time_created, id"),
        ),
    ))
}

/// The two queries for what's still in the conversation: its message ids, and the session's
/// revert.
pub fn ocdb_shape_queries(session: &str) -> Option<(String, String)> {
    session_id_ok(session).then(|| {
        let s = quote(session);
        (json_rows(&["id"], &format!("select id from message where session_id = {s}")), json_rows(&["revert"], &format!("select revert from session where id = {s}")))
    })
}

/// What's still in the conversation: the message ids, and where /undo moved it back to (messages
/// from that one on are hidden until /redo, and deleted once a new message is sent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcdbShape {
    pub ids: HashSet<String>,
    pub revert: Option<String>,
}

pub fn ocdb_shape(ids: &[Value], revert_rows: &[Value]) -> OcdbShape {
    let row = revert_rows.first().map(Some).unwrap_or(None);
    let revert = row
        .at("revert")
        .filter(|_| row.at("revert").truthy())
        .and_then(|r| json::parse(&r.as_str().map_or_else(|| crate::js::json::value_string(r), str::to_owned)))
        .and_then(|v| Some(&v).at("messageID").str().map(str::to_owned));
    OcdbShape { ids: ids.iter().map(|r| Some(r).at("id").to_js_string()).collect(), revert }
}

/// What the session row says about the conversation as a whole: its title, cost, lines changed
/// and share link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OcdbSessionInfo {
    pub title: Option<String>,
    pub cost: Option<f64>,
    pub additions: Option<f64>,
    pub deletions: Option<f64>,
    pub files: Option<f64>,
    pub share: Option<String>,
}

pub fn ocdb_session_info_query(session: &str) -> Option<String> {
    session_id_ok(session).then(|| {
        json_rows(
            &["title", "cost", "summary_additions", "summary_deletions", "summary_files", "share_url"],
            &format!("select title, cost, summary_additions, summary_deletions, summary_files, share_url from session where id = {}", quote(session)),
        )
    })
}

pub fn ocdb_session_info(rows: &[Value]) -> Option<OcdbSessionInfo> {
    let r = Some(rows.first()?);
    if !r.truthy() {
        return None;
    }
    Some(OcdbSessionInfo {
        title: r.at("title").str().map(str::to_owned),
        cost: r.at("cost").num(),
        additions: r.at("summary_additions").num(),
        deletions: r.at("summary_deletions").num(),
        files: r.at("summary_files").num(),
        share: r.at("share_url").str().map(str::to_owned),
    })
}

/// Keeps a conversation's rows by id and rebuilds the API-shaped list after each merge.
#[derive(Debug, Clone, Default)]
pub struct OcdbCache {
    messages: IndexMap<String, Value>,
    parts: IndexMap<String, Value>,
    /// newest time_updated merged so far (a row without one leaves it as it was)
    pub stamp: f64,
}

/// `{ ...JSON.parse(data) }`: the parsed object's keys, or none
fn spread(data: Option<&Value>) -> Option<Map<String, Value>> {
    let v = json::parse(data.str()?)?;
    Some(match v {
        Value::Object(o) => o,
        _ => Map::new(),
    })
}

impl OcdbCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges the rows of the two queries of `ocdb_rows_queries`.
    pub fn merge(&mut self, messages: &[Value], parts: &[Value]) {
        for m in messages {
            let r = Some(m);
            if let Some(mut o) = spread(r.at("data")) {
                let mut time = Map::new();
                time.insert("created".into(), r.at("time_created").cloned().unwrap_or(Value::Null));
                if let Some(Value::Object(t)) = o.get("time") {
                    time.extend(t.clone());
                }
                o.insert("id".into(), r.at("id").cloned().unwrap_or(Value::Null));
                o.insert("time".into(), Value::Object(time));
                self.messages.insert(r.at("id").to_js_string(), Value::Object(o));
            }
            if let Some(t) = r.at("time_updated").num() {
                self.stamp = self.stamp.max(t);
            }
        }
        for p in parts {
            let r = Some(p);
            if let Some(mut o) = spread(r.at("data")) {
                o.insert("id".into(), r.at("id").cloned().unwrap_or(Value::Null));
                o.insert("messageID".into(), r.at("message_id").cloned().unwrap_or(Value::Null));
                o.insert("_t".into(), r.at("time_created").cloned().unwrap_or(Value::Null));
                self.parts.insert(r.at("id").to_js_string(), Value::Object(o));
            }
            if let Some(t) = r.at("time_updated").num() {
                self.stamp = self.stamp.max(t);
            }
        }
    }

    /// drops rows whose message is gone
    pub fn retain(&mut self, ids: &HashSet<String>) {
        self.messages.retain(|id, _| ids.contains(id));
        self.parts.retain(|_, p| ids.contains(&Some(&*p).at("messageID").to_js_string()));
    }

    /// The conversation as the API lists it ({info, parts}[]), hiding what /undo reverted.
    pub fn list(&self, revert: Option<&str>) -> Vec<Value> {
        let mut by_msg: HashMap<String, Vec<&Value>> = HashMap::new();
        for p in self.parts.values() {
            by_msg.entry(Some(p).at("messageID").to_js_string()).or_default().push(p);
        }
        let id = |v: &Value| Some(v).at("id").to_js_string();
        let order = |a: f64, b: f64, ia: &str, ib: &str| {
            let d = a - b;
            if d.is_nan() || d == 0.0 {
                locale_compare(ia, ib)
            } else {
                d.total_cmp(&0.0)
            }
        };
        let mut msgs: Vec<&Value> = self.messages.values().filter(|m| revert.filter(|r| !r.is_empty()).is_none_or(|r| crate::js::text::utf16_lt(&id(m), r))).collect();
        let created = |m: &Value| Some(m).at("time").at("created").present().map_or(0.0, |v| v.as_f64().unwrap_or(f64::NAN));
        msgs.sort_by(|a, b| order(created(a), created(b), &id(a), &id(b)));
        msgs.into_iter()
            .map(|info| {
                let mut parts = by_msg.get(&id(info)).cloned().unwrap_or_default();
                let t = |p: &Value| p.get("_t").and_then(Value::as_f64).unwrap_or(f64::NAN);
                parts.sort_by(|a, b| order(t(a), t(b), &id(a), &id(b)));
                serde_json::json!({ "info": info, "parts": parts })
            })
            .collect()
    }
}

// ---- run on a machine ----

/// `ocdbQuery(host, sql)`: runs a `json_rows` query and returns its rows.
pub async fn ocdb_query(host: &Host, sql: &str) -> HubResult<Vec<Value>> {
    let r = host.exec(&ocdb_query_command(sql), ExecOptions::timeout(15_000)).await?;
    Ok(ocdb_query_rows(&r.stdout))
}

/// `ocdbSessions(host, directory, limit)`
pub async fn ocdb_sessions_of(host: &Host, directory: &str, limit: u32) -> HubResult<Vec<OcdbSessionRow>> {
    Ok(ocdb_sessions(&ocdb_query(host, &ocdb_sessions_query(directory, limit)).await?))
}

/// `guessOpenCodeSession(host, proc, screen, paneTitle)`: the folder's conversations are only read
/// when the title doesn't already say.
pub async fn find_open_code_session(host: &Host, proc: &OpenCodeProc, screen: Option<&[String]>, pane_title: Option<&str>) -> HubResult<Option<String>> {
    let Some(cwd) = proc.cwd.as_deref().filter(|c| !c.is_empty()) else { return Ok(None) };
    if pane_title.map(JsStr::jtrim) == Some("OpenCode") {
        return Ok(Some(OCDB.into()));
    }
    let rows = ocdb_sessions_of(host, cwd, 30).await?;
    Ok(guess_open_code_session(proc, &rows, screen, pane_title))
}

/// `ocdbStamp(host, session)`
pub async fn ocdb_stamp_of(host: &Host, session: &str) -> HubResult<f64> {
    let Some(q) = ocdb_stamp_query(session) else { return Ok(0.0) };
    Ok(ocdb_stamp(&ocdb_query(host, &q).await?))
}

/// `ocdbRows(host, session, since)`: (messages, parts).
pub async fn ocdb_rows_of(host: &Host, session: &str, since: f64) -> HubResult<(Vec<Value>, Vec<Value>)> {
    let Some((m, p)) = ocdb_rows_queries(session, since) else { return Ok((vec![], vec![])) };
    let (messages, parts) = futures_util::future::join(ocdb_query(host, &m), ocdb_query(host, &p)).await;
    Ok((messages?, parts?))
}

/// `ocdbShape(host, session)`
pub async fn ocdb_shape_of(host: &Host, session: &str) -> HubResult<OcdbShape> {
    let Some((ids, revert)) = ocdb_shape_queries(session) else { return Ok(OcdbShape::default()) };
    let (ids, revert) = futures_util::future::join(ocdb_query(host, &ids), ocdb_query(host, &revert)).await;
    Ok(ocdb_shape(&ids?, &revert?))
}

/// `ocdbSessionInfo(host, session)`
pub async fn ocdb_session_info_of(host: &Host, session: &str) -> HubResult<Option<OcdbSessionInfo>> {
    let Some(q) = ocdb_session_info_query(session) else { return Ok(None) };
    Ok(ocdb_session_info(&ocdb_query(host, &q).await?))
}

#[cfg(test)]
#[path = "opencode_db_tests.rs"]
mod tests;
