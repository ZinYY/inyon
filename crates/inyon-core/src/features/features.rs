//! Port of features/features.ts: files sent from the app (uploads and the janitor that tidies
//! them), the slash commands and `@` file mentions the composer offers, the agents' config files,
//! and a session's subagent transcripts and background task output. Everything runs as shell on
//! the pane's machine.

use std::cell::RefCell;

use indexmap::{IndexMap, IndexSet};
use inyon_protocol::{agents, AgentKind, ChatItem, CommandSource, ConfigFormat, HostStatus, Num, PaneInfo, SlashCommand};
use serde::Serialize;
use serde_json::{Map, Value};

use super::paths::{is_plain_path, is_task_file};
use crate::agents::opencode_api::OpenCodeClient;
use crate::claude::transcript::{new_meta, normalize, with_seq};
use crate::clock::now_ms;
use crate::config::load_settings;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;
use crate::js::json::value_string;
use crate::js::text::{locale_compare, JsStr};
use crate::js::{fmt_number, to_number};
use crate::platform::rt;
use crate::re;

pub(super) fn host_and_home(host_id: &str) -> HubResult<(Host, String)> {
    let rt = registry().get(host_id).ok_or_else(|| http(404, "Unknown machine"))?;
    let r = rt.borrow();
    let home = r.probe.as_ref().map(|p| p.borrow().home.clone()).filter(|h| !h.is_empty()).ok_or_else(|| http(503, "Machine is not connected yet"))?;
    Ok((r.host.clone(), home))
}

fn pane_of(host_id: &str, pane_id: &str) -> HubResult<PaneInfo> {
    registry().find_pane(host_id, pane_id).map(|f| f.pane).ok_or_else(|| http(404, "Pane not found"))
}

/// `p.replace(/^~(?=\/|$)/, home).replace('{cwd}', cwd ?? '')`
fn expand(p: &str, home: &str, cwd: Option<&str>) -> String {
    let p = match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => p.to_owned(),
    };
    p.replacen("{cwd}", cwd.unwrap_or(""), 1)
}

/// `new Date().toISOString()`
fn iso_now() -> String {
    crate::js::date::to_iso(now_ms() as f64).unwrap_or_default()
}

// ---------- uploads ----------

fn safe_name(n: &str) -> String {
    let s = re!(r"[^\w.@+-]+", "g").replace_all(n, "_");
    let s = re!(r"^\.+").replace(&s, "");
    let s = s.slice16(-120, None).to_owned();
    if s.is_empty() {
        "file".into()
    } else {
        s
    }
}

/// A file as the app sent it (a part of the multipart form): its name, its type and its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UploadFile {
    pub name: String,
    /// the part's content type ("" when it gave none), `File.type`
    pub mime: String,
    pub data: Vec<u8>,
}

impl UploadFile {
    pub fn new(name: impl Into<String>, mime: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        UploadFile { name: name.into(), mime: mime.into(), data: data.into() }
    }

    /// `File.size`
    pub fn size(&self) -> f64 {
        self.data.len() as f64
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Uploaded {
    pub name: String,
    pub path: String,
    pub size: Num,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Where a new upload goes: ~/.inyon/uploads/<date>/<session>/<time>-<id>-<name> on the pane's
/// machine. The short random id keeps two files of the same name sent in the same second apart.
pub(super) fn upload_path(home: &str, host_id: &str, pane_id: Option<&str>, name: &str) -> String {
    let pane = pane_id.and_then(|p| registry().find_pane(host_id, p)).map(|f| f.pane);
    let bucket = pane.and_then(|p| p.agent.and_then(|a| a.session_id)).or_else(|| pane_id.map(|p| p.replacen('%', "pane-", 1))).unwrap_or_else(|| "misc".into());
    let bucket = bucket.slice16(0, Some(12));
    let iso = iso_now();
    let day = iso.slice16(0, Some(10));
    let stamp = iso.slice16(11, Some(19)).replace(':', "");
    let id = crate::ids::random_uuid();
    format!("{home}/.inyon/uploads/{day}/{bucket}/{stamp}-{}-{}", &id[..6], safe_name(name))
}

/// Sends a file a few megabytes at a time, so a video never sits in memory whole or crosses the
/// phone's SSH plugin (or a paired link) as one enormous message. `from` is where in the file
/// `data` starts; each later piece is added only if the file has exactly the bytes before it.
const PIECE: usize = 2 * 1024 * 1024;

async fn write_upload(host: &Host, path: &str, data: &[u8], from: f64, total: f64) -> HubResult<()> {
    let dir = &path[..path.rfind('/').unwrap_or(0)];
    // the file grows as <name>.part and takes its name once whole, so an upload cut off half way never
    // looks finished to an agent; the janitor deletes .part files an hour old
    let part = format!("{path}.part");
    let mut at = 0usize;
    while at < data.len() || at == 0 {
        let input = data[at.min(data.len())..(at + PIECE).min(data.len())].to_vec();
        let end = from + at as f64 + input.len() as f64;
        // private to your user on shared machines
        // a later piece goes on the end only where the file stops; one that's already there (the app
        // retrying after its answer got lost) is skipped, so a retry never doubles a piece
        let write = if from + at as f64 == 0.0 {
            format!("umask 077; mkdir -p {} && cat > {}", shq(dir), shq(&part))
        } else {
            format!(
                "n=\"$(wc -c < {p} | tr -d ' ')\"; if [ \"$n\" = {end} ]; then cat > /dev/null; else [ \"$n\" = {start} ] && cat >> {p}; fi",
                p = shq(&part),
                end = fmt_number(end),
                start = fmt_number(from + at as f64)
            )
        };
        let finish = if end >= total { format!(" && mv -f {} {}", shq(&part), shq(path)) } else { String::new() };
        // the whole file is already there: this is a retry of the last piece
        let cmd = format!("if [ -f {} ]; then cat > /dev/null; else {{ {write}; }}{finish}; fi", shq(path));
        let r = host.exec(&cmd, ExecOptions { input: Some(input), timeout_ms: Some(120_000) }).await?;
        if r.code != 0 {
            let err = r.stderr.jtrim();
            return Err(http(500, if err.is_empty() { format!("Could not save {}", &path[path.rfind('/').map_or(0, |i| i + 1)..]) } else { err.to_owned() }));
        }
        at += PIECE;
    }
    Ok(())
}

pub(super) fn max_mb() -> f64 {
    load_settings().uploads().map_or(100.0, |u| u.max_mb)
}

/// Copies files into ~/.inyon/uploads/<date>/<session>/ on the pane's machine.
pub async fn upload_files(host_id: &str, pane_id: Option<&str>, files: &[UploadFile]) -> HubResult<Vec<Uploaded>> {
    let (host, home) = host_and_home(host_id)?;
    let max_mb = max_mb();
    let mut out = vec![];
    for f in files {
        if f.size() > max_mb * 1024.0 * 1024.0 {
            return Err(http(413, format!("{} is larger than {} MB", f.name, fmt_number(max_mb))));
        }
        let path = upload_path(&home, host_id, pane_id, &f.name);
        write_upload(&host, &path, &f.data, 0.0, f.size()).await?;
        out.push(Uploaded { name: f.name.clone(), path, size: Num(f.size()), kind: f.mime.clone() });
    }
    janitor::soon(host_id);
    Ok(out)
}

/// Where a piece goes in the file the app sends in pieces.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PieceAt {
    pub total: f64,
    pub offset: f64,
    pub path: Option<String>,
}

/// One piece of a file the app sends in pieces (the phone app does, so it can show progress and
/// never holds a whole video). The first piece picks the path; the app sends it back with the rest.
pub async fn upload_piece(host_id: &str, pane_id: Option<&str>, piece: &UploadFile, o: &PieceAt) -> HubResult<Uploaded> {
    let (host, home) = host_and_home(host_id)?;
    let max_mb = max_mb();
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(o.total >= 0.0) || !(o.offset >= 0.0) || o.offset + piece.size() > o.total {
        return Err(http(400, "Bad upload piece"));
    }
    if o.total > max_mb * 1024.0 * 1024.0 {
        return Err(http(413, format!("{} is larger than {} MB", piece.name, fmt_number(max_mb))));
    }
    let path = if o.offset != 0.0 { o.path.clone() } else { Some(upload_path(&home, host_id, pane_id, &piece.name)) };
    let path = match path {
        Some(p) if !p.is_empty() && p.starts_with(&format!("{home}/.inyon/uploads/")) && !re!(r"/\.\.?(/|$)|[\n\0]").is_match(&p) => p,
        _ => return Err(http(400, "Bad upload path")),
    };
    write_upload(&host, &path, &piece.data, o.offset, o.total).await?;
    if o.offset + piece.size() >= o.total {
        janitor::soon(host_id);
    }
    Ok(Uploaded { name: piece.name.clone(), path, size: Num(o.total), kind: piece.mime.clone() })
}

/// What a machine's uploads folder holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UploadUsage {
    pub bytes: Num,
    pub files: Num,
    pub expired: Num,
    pub dir: String,
    /// The retention setting `expired` counts with, so the app's clean button cleans the same files
    pub days: Num,
}

/// `Number(x) || 0`
fn or_zero(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x
    }
}

pub async fn upload_usage(host_id: &str) -> HubResult<UploadUsage> {
    let (host, home) = host_and_home(host_id)?;
    let dir = format!("{home}/.inyon/uploads");
    let days = load_settings().uploads().map_or(7.0, |u| u.retention_days);
    let old = if days > 0.0 { format!("$(find {} -type f -mtime +{} | wc -l)", shq(&dir), fmt_number(days)) } else { "0".into() };
    let d = shq(&dir);
    let r = host.run(&format!("[ -d {d} ] || {{ echo \"0 0 0\"; exit 0; }}; echo \"$(du -sk {d} | cut -f1) $(find {d} -type f | wc -l) {old}\"")).await?;
    let nums: Vec<f64> = re!(r"\s+").split(r.stdout.jtrim()).map(to_number).collect();
    let at = |i: usize| or_zero(nums.get(i).copied().unwrap_or(f64::NAN));
    Ok(UploadUsage { bytes: Num(at(0) * 1024.0), files: Num(at(1)), expired: Num(at(2)), dir, days: Num(days) })
}

/// The shell that tidies a machine's uploads: files older than `days` (none when 0, all when -1),
/// .part (and .part.chunk) files an upload left behind over an hour ago, then the oldest days first while the folder
/// is over `max_total_mb` (0: no limit). Only find, du and rm, so it runs on macOS, Linux and busybox.
pub fn clean_script(dir: &str, days: f64, max_total_mb: f64) -> String {
    let d = shq(dir);
    let fdays = fmt_number(days.floor());
    let age = if days > 0.0 {
        format!("find {d} -type f -mtime +{fdays} -exec rm -f {{}} +;")
    } else if days < 0.0 {
        format!("find {d} -type f -exec rm -f {{}} +;")
    } else {
        String::new()
    };
    let cap = if max_total_mb > 0.0 {
        let n = if days > 0.0 { fdays.clone() } else { "365".into() };
        format!(
            "n={n}; while [ \"$n\" -ge 0 ] && [ \"$(du -sk {d} | cut -f1)\" -gt {} ]; do find {d} -type f -mtime +$n -exec rm -f {{}} +; n=$((n-1)); done;",
            fmt_number(max_total_mb.floor() * 1024.0)
        )
    } else {
        String::new()
    };
    // the machine's own inyon tidies by the same rules while no hub is connected (crates/inyon-engine)
    let policy = if days >= 0.0 {
        let policy_file = re!(r"/uploads$").replace(dir, "/upload-policy");
        format!("printf '%s %s\\n' {fdays} {} > {} 2>/dev/null;", fmt_number(max_total_mb.floor()), shq(&policy_file))
    } else {
        String::new()
    };
    format!(
        "{policy} [ -d {d} ] || exit 0; {age} find {d} -type f \\( -name '*.part' -o -name '*.part.chunk' \\) -mmin +60 -exec rm -f {{}} +; {cap} find {d} -mindepth 1 -type d -empty -delete 2>/dev/null; true"
    )
}

/// Deletes uploads older than `days` (all of them when days is 0), and anything a cut-off upload left.
pub async fn clean_uploads(host_id: &str, days: f64) -> HubResult<UploadUsage> {
    let (host, home) = host_and_home(host_id)?;
    let max_total = load_settings().uploads().map_or(1024.0, |u| u.max_total_mb);
    host.run(&clean_script(&format!("{home}/.inyon/uploads"), if days > 0.0 { days } else { -1.0 }, max_total)).await?;
    upload_usage(host_id).await
}

/// Keeps every machine's uploads within the settings: each connected machine is tidied when it's
/// first seen, when it comes back after being away, every six hours, and soon after an upload.
pub mod janitor {
    use super::*;

    thread_local! {
        static LAST: RefCell<IndexMap<String, u64>> = RefCell::new(IndexMap::new());
    }

    fn last(id: &str) -> u64 {
        LAST.with(|l| l.borrow().get(id).copied().unwrap_or(0))
    }

    fn forget(id: &str) {
        LAST.with(|l| l.borrow_mut().shift_remove(id));
    }

    async fn tidy(id: String) {
        LAST.with(|l| l.borrow_mut().insert(id.clone(), now_ms()));
        let found = registry().get(&id).and_then(|rt| {
            let r = rt.borrow();
            let home = r.probe.as_ref().map(|p| p.borrow().home.clone()).filter(|h| !h.is_empty())?;
            Some((r.host.clone(), home))
        });
        let Some((host, home)) = found else { return forget(&id) };
        let u = load_settings().uploads();
        let (days, max_total) = u.map_or((7.0, 1024.0), |u| (u.retention_days, u.max_total_mb));
        if host.run(&clean_script(&format!("{home}/.inyon/uploads"), days, max_total)).await.is_err() {
            forget(&id);
        }
    }

    pub fn sweep() {
        let mut online = std::collections::HashSet::new();
        for h in registry().overview() {
            if h.status != HostStatus::Online {
                continue;
            }
            online.insert(h.config.id.clone());
            if now_ms().saturating_sub(last(&h.config.id)) > 6 * 3_600_000 {
                rt::spawn(tidy(h.config.id.clone()));
            }
        }
        // a machine that went away is tidied again as soon as it's back
        LAST.with(|l| l.borrow_mut().retain(|id, _| online.contains(id)));
    }

    pub fn start() {
        rt::set_timeout(60_000, sweep);
        rt::spawn(async {
            loop {
                rt::sleep(10 * 60_000).await;
                sweep();
            }
        });
    }

    /// after an upload: at most once an hour per machine
    pub fn soon(id: &str) {
        if now_ms().saturating_sub(last(id)) > 3_600_000 {
            let id = id.to_owned();
            rt::set_timeout(5_000, move || rt::spawn(tidy(id)));
        }
    }
}

/// Applies the retention setting on every connected machine, and drops chunked uploads left alone.
pub fn start_upload_janitor() {
    janitor::start();
    super::uploads::start_sweeping();
}

/// A small cache: entries go after `ttl` ms, and past `max` entries the least recently used go first.
pub struct LruCache<T> {
    ttl: u64,
    max: usize,
    entries: IndexMap<String, (u64, T)>,
}

pub fn lru_cache<T: Clone>(ttl: u64, max: usize) -> LruCache<T> {
    LruCache { ttl, max, entries: IndexMap::new() }
}

impl<T: Clone> LruCache<T> {
    pub fn get(&mut self, key: &str) -> Option<T> {
        let (at, value) = self.entries.shift_remove(key)?;
        if now_ms().saturating_sub(at) >= self.ttl {
            return None;
        }
        self.entries.insert(key.to_owned(), (at, value.clone()));
        Some(value)
    }

    pub fn set(&mut self, key: &str, value: T) {
        self.entries.shift_remove(key);
        self.entries.insert(key.to_owned(), (now_ms(), value));
        while let Some((_, (at, _))) = self.entries.first() {
            if self.entries.len() <= self.max && now_ms().saturating_sub(*at) < self.ttl {
                break;
            }
            self.entries.shift_remove_index(0);
        }
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }
}

// ---------- slash commands ----------

thread_local! {
    // short, so a skill installed a moment ago shows up the next time the list opens
    static CMD_CACHE: RefCell<LruCache<Vec<SlashCommand>>> = RefCell::new(lru_cache(5_000, 50));
    static FILE_CACHE: RefCell<LruCache<Vec<String>>> = RefCell::new(lru_cache(30_000, 20));
}

fn by_name(list: &mut [SlashCommand]) {
    list.sort_by(|a, b| locale_compare(&a.name, &b.name));
}

// prints the description line of a SKILL.md or command file
const DESC: &str = r#"awk 'NR==1&&$0!="---"{exit} NR>1&&$0=="---"{exit} /^description:/{sub(/^description:[ ]*/,""); print; exit}' "$f" 2>/dev/null | cut -c1-180"#;

// the agents read project skills and commands from the folder and each one above it up to the repo root
const UP: &str = r#"d=@CWD@; top=$(git -C "$d" rev-parse --show-toplevel 2>/dev/null || echo "$d"); dirs="$d"; while [ "$d" != "$top" ] && [ "$d" != / ] && [ -n "$d" ]; do d=$(dirname "$d"); dirs="$dirs
$d"; done"#;

const SCAN_CLAUDE: &str = r#"@UP@
for f in "$HOME"/.claude/skills/*/SKILL.md; do [ -f "$f" ] && printf 'skill\t%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done
echo "$dirs" | while read -r p; do for f in "$p"/.claude/skills/*/SKILL.md; do [ -f "$f" ] && printf 'project\t%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done; done
for f in "$HOME"/.claude/commands/*.md; do [ -f "$f" ] && printf 'user\t%s\t%s\n' "$(basename "$f" .md)" "$(@DESC@)"; done
echo "$dirs" | while read -r p; do for f in "$p"/.claude/commands/*.md; do [ -f "$f" ] && printf 'project\t%s\t%s\n' "$(basename "$f" .md)" "$(@DESC@)"; done; done
find "$HOME/.claude/plugins" -maxdepth 9 -path '*/skills/*/SKILL.md' 2>/dev/null | head -200 | while read -r f; do printf 'plugin\t%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done"#;

// Codex picks skills with "$name" (its "/" menu is built-in commands only); a plugin's skills are
// "$plugin:skill", read from the newest cached version of each plugin
const SCAN_CODEX: &str = r#"@UP@
for f in "$HOME"/.codex/prompts/*.md; do [ -f "$f" ] && printf 'user\t/prompts:%s\t%s\n' "$(basename "$f" .md)" "$(@DESC@)"; done
echo "$dirs" | while read -r p; do for f in "$p"/.agents/skills/*/SKILL.md; do [ -f "$f" ] && printf 'project\t$%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done; done
for f in "$HOME"/.agents/skills/*/SKILL.md "$HOME"/.codex/skills/*/SKILL.md "$HOME"/.codex/skills/.system/*/SKILL.md; do [ -f "$f" ] && printf 'skill\t$%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done
find "$HOME/.codex/plugins/cache" -mindepth 6 -maxdepth 6 -path '*/skills/*/SKILL.md' 2>/dev/null | sort -rV | head -300 | while read -r f; do p=${f%/*/*/*/SKILL.md}; printf 'plugin\t$%s:%s\t%s\n' "${p##*/}" "$(basename "$(dirname "$f")")" "$(@DESC@)"; done"#;

const SCAN_OPENCODE: &str = r#"for f in "$HOME"/.config/opencode/command/*.md "$HOME"/.config/opencode/commands/*.md @CWD@/.opencode/command/*.md @CWD@/.opencode/commands/*.md; do [ -f "$f" ] && printf 'user\t%s\t%s\n' "$(basename "$f" .md)" "$(@DESC@)"; done"#;

const SCAN_GEMINI: &str = r#"for f in "$HOME"/.gemini/commands/*.toml @CWD@/.gemini/commands/*.toml; do [ -f "$f" ] && printf 'user\t%s\t%s\n' "$(basename "$f" .toml)" "$(grep -m1 '^description' "$f" | sed 's/^description *= *//; s/^"//; s/"$//')"; done"#;

// skills become slash commands: workspace .agents/skills, global ~/.gemini/config/skills, and plugins
const SCAN_ANTIGRAVITY: &str = r#"
for f in @CWD@/.agents/skills/*/SKILL.md @CWD@/.agent/skills/*/SKILL.md; do [ -f "$f" ] && printf 'project\t%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done
for f in "$HOME"/.gemini/config/skills/*/SKILL.md; do [ -f "$f" ] && printf 'skill\t%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done
find "$HOME/.gemini/antigravity-cli/plugins" -maxdepth 4 -path '*/skills/*/SKILL.md' 2>/dev/null | head -200 | while read -r f; do printf 'plugin\t%s\t%s\n' "$(basename "$(dirname "$f")")" "$(@DESC@)"; done"#;

/// The shell that lists an agent's skills and custom commands for a pane in `cwd` (already
/// quoted), one "source<TAB>name<TAB>description" line each.
pub fn scan_script(kind: AgentKind, cwd: &str) -> String {
    let script = match kind {
        AgentKind::Claude => SCAN_CLAUDE,
        AgentKind::Codex => SCAN_CODEX,
        AgentKind::Opencode => SCAN_OPENCODE,
        AgentKind::Gemini => SCAN_GEMINI,
        AgentKind::Copilot => "true",
        AgentKind::Antigravity => SCAN_ANTIGRAVITY,
        // the terminal-first agents have no commands Inyon lists
        _ => "true",
    };
    // the placeholders go in as text, never through a format string: awk's braces would read as one
    script.replace("@UP@", &UP.replace("@CWD@", cwd)).replace("@DESC@", DESC).replace("@CWD@", cwd)
}

fn source_of(s: &str) -> CommandSource {
    match s {
        "skill" => CommandSource::Skill,
        "project" => CommandSource::Project,
        "plugin" => CommandSource::Plugin,
        "built-in" => CommandSource::BuiltIn,
        _ => CommandSource::User,
    }
}

/// Built-in commands plus skills and custom commands found on the machine.
pub async fn list_commands(host_id: &str, pane_id: &str) -> HubResult<Vec<SlashCommand>> {
    let pane = pane_of(host_id, pane_id)?;
    let kind = pane.agent.as_ref().map_or(AgentKind::Claude, |a| a.kind);
    let key = format!("{host_id}:{}:{}", kind.as_str(), pane.cwd);
    if let Some(hit) = CMD_CACHE.with(|c| c.borrow_mut().get(&key)) {
        return Ok(hit);
    }
    let (host, _) = host_and_home(host_id)?;
    let uri = pane.agent.as_ref().and_then(|a| a.transcript_path.clone()).filter(|t| t.starts_with("opencode://"));
    if let (AgentKind::Opencode, Some(uri)) = (kind, uri) {
        let rows = OpenCodeClient::new(&host, &uri, Some(&pane.cwd))?.request("/command", "GET", None, None).await?;
        let dynamic: Vec<SlashCommand> = match rows {
            Some(Value::Array(rows)) => rows
                .iter()
                .map(|c| SlashCommand {
                    name: format!("/{}", c.get("name").map_or("undefined".into(), value_string)),
                    description: c.get("description").filter(|d| !d.is_null()).map_or(String::new(), value_string),
                    source: if c.get("source").and_then(Value::as_str) == Some("mcp") { CommandSource::Plugin } else { CommandSource::Project },
                    instant: None,
                })
                .collect(),
            _ => vec![],
        };
        let names: std::collections::HashSet<&str> = dynamic.iter().map(|c| c.name.as_str()).collect();
        let mut list: Vec<SlashCommand> = agents()[&AgentKind::Opencode].commands.iter().filter(|c| !names.contains(c.name.as_str())).cloned().collect();
        list.extend(dynamic.iter().cloned());
        by_name(&mut list);
        CMD_CACHE.with(|c| c.borrow_mut().set(&key, list.clone()));
        return Ok(list);
    }
    let r = host.run(&scan_script(kind, &shq(&pane.cwd))).await?;
    let mut seen = std::collections::HashSet::new();
    let mut found = vec![];
    for line in r.stdout.split('\n') {
        let mut parts = line.split('\t');
        let (source, name, description) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if name.is_empty() {
            continue;
        }
        // Codex skills ("$name") and prompts ("/prompts:name") come with their own prefix
        let full = if name.starts_with('$') || name.starts_with('/') { name.to_owned() } else { format!("/{name}") };
        if !seen.insert(full.clone()) {
            continue;
        }
        let description = re!(r#"^["'>|-]+\s*|["']$"#, "g").replace_all(description, "").into_owned();
        found.push(SlashCommand { name: full, description, source: source_of(source), instant: None });
    }
    let mut list: Vec<SlashCommand> = agents()[&kind].commands.iter().filter(|c| !seen.contains(&c.name)).cloned().collect();
    list.extend(found);
    by_name(&mut list);
    CMD_CACHE.with(|c| c.borrow_mut().set(&key, list.clone()));
    Ok(list)
}

// ---------- @ file mentions ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileList {
    pub cwd: String,
    pub files: Vec<String>,
}

pub async fn list_files(host_id: &str, pane_id: &str) -> HubResult<FileList> {
    let pane = pane_of(host_id, pane_id)?;
    let key = format!("{host_id}:{}", pane.cwd);
    if let Some(files) = FILE_CACHE.with(|c| c.borrow_mut().get(&key)) {
        return Ok(FileList { cwd: pane.cwd, files });
    }
    let (host, _) = host_and_home(host_id)?;
    let r = host
        .exec(
            &format!(
                r"cd {} 2>/dev/null || exit 0; if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then git ls-files -co --exclude-standard 2>/dev/null | head -20000; else find . -maxdepth 6 \( -name node_modules -o -name .git -o -name dist -o -name .venv -o -name __pycache__ \) -prune -o -type f -print 2>/dev/null | sed 's|^\./||' | head -20000; fi",
                shq(&pane.cwd)
            ),
            ExecOptions::timeout(15_000),
        )
        .await?;
    let files: Vec<String> = r.stdout.split('\n').filter(|l| !l.is_empty()).map(str::to_owned).collect();
    FILE_CACHE.with(|c| c.borrow_mut().set(&key, files.clone()));
    Ok(FileList { cwd: pane.cwd, files })
}

// ---------- agent config files ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigFileInfo {
    pub agent: AgentKind,
    pub label: String,
    pub path: String,
    pub format: ConfigFormat,
    pub exists: bool,
    pub size: Num,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<Num>,
}

#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    agent: AgentKind,
    label: String,
    path: String,
    format: ConfigFormat,
}

fn config_candidates(host_id: &str, home: &str) -> Vec<Candidate> {
    let Some(rt) = registry().get(host_id) else { return vec![] };
    let r = rt.borrow();
    let mut kinds: IndexSet<AgentKind> = r.state.agents.iter().copied().collect();
    let mut cwds: IndexMap<AgentKind, IndexSet<String>> = IndexMap::new();
    for s in &r.state.sessions {
        for w in &s.windows {
            for p in &w.panes {
                if let Some(a) = &p.agent {
                    kinds.insert(a.kind);
                    cwds.entry(a.kind).or_default().insert(p.cwd.clone());
                }
            }
        }
    }
    let mut out = vec![];
    for k in kinds {
        for c in &agents()[&k].config_files {
            if c.path.contains("{cwd}") {
                for cwd in cwds.get(&k).into_iter().flatten() {
                    out.push(Candidate { agent: k, label: format!("{} · {}", c.label, cwd.replacen(home, "~", 1)), path: expand(&c.path, home, Some(cwd)), format: c.format });
                }
            } else {
                out.push(Candidate { agent: k, label: c.label.clone(), path: expand(&c.path, home, None), format: c.format });
            }
        }
    }
    out
}

pub async fn list_configs(host_id: &str) -> HubResult<Vec<ConfigFileInfo>> {
    let (host, home) = host_and_home(host_id)?;
    let cands = config_candidates(host_id, &home);
    if cands.is_empty() {
        return Ok(vec![]);
    }
    let script = cands
        .iter()
        .map(|c| {
            let p = shq(&c.path);
            format!("if [ -f {p} ]; then echo \"1 $(wc -c < {p}) $(stat -c %Y {p} 2>/dev/null || stat -f %m {p})\"; else echo \"0 0 0\"; fi")
        })
        .collect::<Vec<_>>()
        .join("; ");
    let r = host.run(&script).await?;
    let rows: Vec<&str> = r.stdout.jtrim().split('\n').collect();
    Ok(cands
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let nums: Vec<f64> = rows.get(i).copied().unwrap_or("0 0 0").split(' ').map(to_number).collect();
            let at = |i: usize| nums.get(i).copied().unwrap_or(f64::NAN);
            let mtime = or_zero(at(2));
            ConfigFileInfo {
                agent: c.agent,
                label: c.label,
                path: c.path,
                format: c.format,
                exists: at(0) == 1.0,
                size: Num(or_zero(at(1))),
                mtime: (mtime != 0.0).then_some(Num(mtime)),
            }
        })
        .collect())
}

fn assert_config_path(host_id: &str, path: &str) -> HubResult<Candidate> {
    let (_, home) = host_and_home(host_id)?;
    config_candidates(host_id, &home).into_iter().find(|x| x.path == path).ok_or_else(|| http(403, "That file is not an agent config file Inyon manages"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigText {
    pub path: String,
    pub content: String,
}

pub async fn read_config(host_id: &str, path: &str) -> HubResult<ConfigText> {
    assert_config_path(host_id, path)?;
    let (host, _) = host_and_home(host_id)?;
    let r = host.run(&format!("cat {} 2>/dev/null", shq(path))).await?;
    Ok(ConfigText { path: path.to_owned(), content: r.stdout })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Saved {
    pub ok: bool,
}

/// Validates (JSON), backs the old file up next to it, then replaces it atomically.
pub async fn write_config(host_id: &str, path: &str, content: &str) -> HubResult<Saved> {
    let c = assert_config_path(host_id, path)?;
    if c.format == ConfigFormat::Json && !content.jtrim().is_empty() && crate::js::json::parse(content).is_none() {
        // V8's own wording can't be had here; serde's says where the JSON broke
        let why = serde_json::from_str::<Value>(content).err().map_or_else(|| "Unexpected token".to_owned(), |e| e.to_string());
        return Err(http(400, format!("Not valid JSON: {why}")));
    }
    let (host, _) = host_and_home(host_id)?;
    let stamp = re!(r"[:.]", "g").replace_all(&iso_now(), "-").slice16(0, Some(19)).to_owned();
    let p = shq(path);
    let tmp = shq(&format!("{path}.inyon-tmp"));
    let cmd = format!(
        "umask 077; trap {} EXIT HUP INT TERM; mkdir -p \"$(dirname {p})\" && {{ [ ! -f {p} ] || cp {p} {}; }} && cat > {tmp} && mv {tmp} {p} || exit $?; ls -t {p}.inyon-bak-* 2>/dev/null | tail -n +6 | while IFS= read -r f; do rm -f -- \"$f\"; done; true",
        shq(&format!("rm -f {tmp}")),
        shq(&format!("{path}.inyon-bak-{stamp}")),
    );
    let r = host.exec(&cmd, ExecOptions::input(content)).await?;
    if r.code != 0 {
        let err = r.stderr.jtrim();
        return Err(http(500, if err.is_empty() { "Could not save" } else { err }));
    }
    Ok(Saved { ok: true })
}

// ---------- subagents and background task output ----------

/// Transcripts are read from Claude's projects folder only: ~/.claude/projects, or the one a
/// session on that machine uses (CLAUDE_CONFIG_DIR moves it).
fn assert_transcript_path(host_id: &str, home: &str, p: &str) -> HubResult<()> {
    let mut roots = vec![format!("{home}/.claude/projects/")];
    if let Some(rt) = registry().get(host_id) {
        let r = rt.borrow();
        for s in &r.state.sessions {
            for w in &s.windows {
                for pane in &w.panes {
                    let Some(t) = pane.agent.as_ref().and_then(|a| a.transcript_path.as_deref()) else { continue };
                    if let Some(at) = t.find("/projects/").filter(|at| *at > 0) {
                        if is_plain_path(t) {
                            roots.push(t[..at + "/projects/".len()].to_owned());
                        }
                    }
                }
            }
        }
    }
    if !p.ends_with(".jsonl") || !is_plain_path(p) || !roots.iter().any(|r| p.starts_with(r.as_str())) {
        return Err(http(400, "Bad transcript path"));
    }
    Ok(())
}

/// One subagent of a session: `{ agentId, toolUseId?, type?, description?, path }`, the meta file's
/// fields passed on as they are.
pub async fn list_subagents(host_id: &str, transcript_path: &str) -> HubResult<Vec<Value>> {
    let (host, home) = host_and_home(host_id)?;
    assert_transcript_path(host_id, &home, transcript_path)?;
    let dir = format!("{}/subagents", transcript_path.strip_suffix(".jsonl").unwrap_or(transcript_path));
    let r = host.run(&format!("for m in {}/*.meta.json; do [ -f \"$m\" ] && echo \"$(basename \"$m\" .meta.json)\t$(tr -d '\\n' < \"$m\")\"; done", shq(&dir))).await?;
    Ok(r.stdout
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut parts = l.split('\t');
            let id = parts.next().unwrap_or("").to_owned();
            let meta = parts.next().and_then(crate::js::json::parse).unwrap_or(Value::Null);
            let mut out = Map::new();
            out.insert("agentId".into(), Value::String(id.clone()));
            for (to, from) in [("toolUseId", "toolUseId"), ("type", "agentType"), ("description", "description")] {
                if let Some(v) = meta.get(from) {
                    out.insert(to.into(), v.clone());
                }
            }
            out.insert("path".into(), Value::String(format!("{dir}/{id}.jsonl")));
            Value::Object(out)
        })
        .collect())
}

/// A whole (sub)agent transcript as chat items, read once.
pub async fn read_transcript(host_id: &str, path: &str) -> HubResult<Vec<ChatItem>> {
    let (host, home) = host_and_home(host_id)?;
    assert_transcript_path(host_id, &home, path)?;
    let r = host.exec(&format!("tail -c 3000000 {} 2>/dev/null", shq(path)), ExecOptions::timeout(20_000)).await?;
    let mut meta = new_meta();
    let mut items = vec![];
    let mut seq = 0;
    for line in r.stdout.split('\n') {
        if line.is_empty() {
            continue;
        }
        let Some(e) = crate::js::json::parse(line) else { continue };
        for d in normalize(&e, &mut meta, true) {
            items.push(with_seq(d, seq));
            seq += 1;
        }
    }
    let from = items.len().saturating_sub(600);
    Ok(items.split_off(from))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskOutput {
    pub path: String,
    pub output: String,
}

pub async fn task_output(host_id: &str, path: &str) -> HubResult<TaskOutput> {
    let (host, home) = host_and_home(host_id)?;
    if !is_task_file(path, Some(&home)) {
        return Err(http(400, "Not a task output file"));
    }
    let r = host.run(&format!("tail -c 80000 {} 2>/dev/null; true", shq(path))).await?;
    Ok(TaskOutput { path: path.to_owned(), output: re!(r"\x1b\[[0-9;?]*[A-Za-z]", "g").replace_all(&r.stdout, "").into_owned() })
}

#[cfg(test)]
#[path = "features_tests.rs"]
mod tests;
