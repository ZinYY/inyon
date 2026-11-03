//! Port of features/workspace.ts.
//!
//! The folder a session works in, as the Files and Changes tabs see it: its files, read only, and
//! its git working tree (what changed since the last commit, whoever changed it), with commit and
//! push. Everything runs as one short shell command on the pane's machine, in the agent's folder.

use serde::Serialize;
use serde_json::Value;

use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;
use crate::js::text::{locale_compare, JsStr};
use crate::js::to_number;
use crate::re;

/// The agent's folder (where it was started), or the pane's shell folder when there's no agent.
fn folder(host_id: &str, pane_id: &str) -> HubResult<(Host, String)> {
    let reg = registry();
    let rt = reg.get(host_id).ok_or_else(|| http(404, "Unknown machine"))?;
    let f = reg.find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    let cwd = reg.agent_proc(host_id, pane_id).and_then(|p| p.cwd).unwrap_or(f.pane.cwd);
    if cwd.is_empty() {
        return Err(http(409, "Unknown working folder"));
    }
    let host = rt.borrow().host.clone();
    Ok((host, cwd))
}

/// A path inside the folder, relative to it: no "..", no absolute paths, nothing a shell line would split.
pub fn inside(rel: &str) -> HubResult<String> {
    let p = re!(r"^\.?\/+").replace(rel, "");
    let p = re!(r"\/+$").replace(&p, "").into_owned();
    if re!(r"[\0\n\r]").is_match(&p) || p.split('/').any(|s| s == "..") || p.starts_with('/') {
        return Err(http(400, "Bad path"));
    }
    Ok(p)
}

/// `Number(s) || 0` for a count a command printed.
fn count(s: &str) -> u64 {
    let n = to_number(s);
    if n.is_finite() && n > 0.0 {
        n as u64
    } else {
        0
    }
}

// ---------- files ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// seconds since 1970
    pub mtime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirListing {
    pub cwd: String,
    pub path: String,
    pub entries: Vec<DirEntry>,
}

const HIDE: [&str; 3] = [".git", "node_modules", ".DS_Store"];

/// One folder's entries, folders first, up to 2000.
pub async fn list_dir(host_id: &str, pane_id: &str, rel: &str) -> HubResult<DirListing> {
    let (host, cwd) = folder(host_id, pane_id)?;
    let path = inside(rel)?;
    let dir = if path.is_empty() { cwd.clone() } else { format!("{cwd}/{path}") };
    // "d|f size mtime name" per entry; GNU stat or BSD stat, whichever is there
    let r = host
        .exec(
            &format!(
                "cd {} 2>/dev/null || {{ echo 'No such folder' >&2; exit 3; }}; for f in .[!.]* ..?* *; do [ -e \"$f\" ] || [ -L \"$f\" ] || continue; if [ -d \"$f\" ]; then t=d; else t=f; fi; s=$(stat -c '%s %Y' -- \"$f\" 2>/dev/null || stat -f '%z %m' -- \"$f\" 2>/dev/null || echo '0 0'); printf '%s %s %s\\n' \"$t\" \"$s\" \"$f\"; done | head -2000",
                shq(&dir)
            ),
            ExecOptions::timeout(15000),
        )
        .await?;
    if r.code == 3 {
        return Err(http(404, "No such folder"));
    }
    let mut entries = vec![];
    for line in r.stdout.split('\n') {
        let Some(m) = re!(r"^([df]) (\d+) (\d+) (.+)$").captures(line) else { continue };
        if HIDE.contains(&&m[4]) {
            continue;
        }
        entries.push(DirEntry { name: m[4].to_owned(), dir: &m[1] == "d", size: count(&m[2]), mtime: count(&m[3]) });
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| locale_compare(&a.name, &b.name)));
    Ok(DirListing { cwd, path, entries })
}

const TEXT_MAX: u64 = 512 * 1024;
const IMAGE_MAX: u64 = 8 * 1024 * 1024;

fn image_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FileView {
    Text {
        path: String,
        text: String,
        size: u64,
        truncated: bool,
    },
    Image {
        path: String,
        #[serde(rename = "type")]
        mime: String,
        data: String,
        size: u64,
    },
    Binary {
        path: String,
        size: u64,
    },
    Large {
        path: String,
        size: u64,
    },
}

/// A file to look at: text (the first 512 KB), an image (up to 8 MB), or just its size.
pub async fn read_file(host_id: &str, pane_id: &str, rel: &str) -> HubResult<FileView> {
    let (host, cwd) = folder(host_id, pane_id)?;
    let path = inside(rel)?;
    if path.is_empty() {
        return Err(http(400, "Bad path"));
    }
    let file = shq(&format!("{cwd}/{path}"));
    let st = host.exec(&format!("[ -f {file} ] || exit 3; wc -c < {file}"), ExecOptions::timeout(10000)).await?;
    if st.code == 3 {
        return Err(http(404, "No such file"));
    }
    let size = count(st.stdout.jtrim());
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    if let Some(mime) = image_type(&ext) {
        if size > IMAGE_MAX {
            return Ok(FileView::Large { path, size });
        }
        let r = host.exec(&format!("base64 < {file} | tr -d '\\n'"), ExecOptions::timeout(30000)).await?;
        return Ok(FileView::Image { path, mime: mime.into(), data: r.stdout.jtrim().to_owned(), size });
    }
    // a NUL byte in the first 8 KB: not text
    let bin = host.exec(&format!("head -c 8192 {file} | tr -d '\\000' | wc -c"), ExecOptions::timeout(10000)).await?;
    if to_number(bin.stdout.jtrim()) < size.min(8192) as f64 {
        return Ok(FileView::Binary { path, size });
    }
    let r = host.exec(&format!("head -c {TEXT_MAX} {file}"), ExecOptions::timeout(30000)).await?;
    Ok(FileView::Text { path, text: r.stdout, size, truncated: size > TEXT_MAX })
}

// ---------- git ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitFile {
    pub path: String,
    /// where it was before a rename
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// M modified, A added, D deleted, R renamed, ? not tracked yet
    pub status: &'static str,
    pub add: u64,
    pub del: u64,
}

/// What `parseStatus` reads: the changed files, then the branch and how far from its upstream
/// (in the order the TS object gets its keys).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ParsedStatus {
    pub files: Vec<GitFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitState {
    pub repo: bool,
    /// the repository's top folder
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// it has somewhere to push to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<bool>,
    #[serde(flatten)]
    pub status: ParsedStatus,
}

const GIT: &str = "GIT_TERMINAL_PROMPT=0 GIT_OPTIONAL_LOCKS=0 git -c core.quotepath=off -c color.ui=false";

/// Reads `git status --porcelain=v1 -z --branch` and `git diff --numstat -z` into a list.
pub fn parse_status(status: &str, numstat: &str) -> ParsedStatus {
    let mut parts: Vec<&str> = status.split('\0').collect();
    let mut out = ParsedStatus::default();
    let head = if parts.first().is_some_and(|p| p.starts_with("## ")) { parts.remove(0)[3..].to_owned() } else { String::new() };
    if let Some(hm) = re!(r"^(?:No commits yet on |Initial commit on )?(.+?)(?:\.\.\.(\S+))?(?: \[(.+)\])?$").captures(&head) {
        out.branch = Some(if &hm[1] == "HEAD (no branch)" { "detached".into() } else { hm[1].to_owned() });
        out.upstream = hm.get(2).map(|m| m.as_str().to_owned());
        let track = hm.get(3).map(|m| m.as_str()).unwrap_or("");
        out.ahead = Some(re!(r"ahead (\d+)").captures(track).map_or(0, |c| count(&c[1])));
        out.behind = Some(re!(r"behind (\d+)").captures(track).map_or(0, |c| count(&c[1])));
    }
    let mut counts: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    let ns: Vec<&str> = numstat.split('\0').collect();
    let mut i = 0;
    while i < ns.len() {
        if let Some(m) = re!(r"^(-|\d+)\t(-|\d+)\t(.*)$", "s").captures(ns[i]) {
            // a rename is "add\tdel\t" then the old and new paths as their own fields
            let path = if m[3].is_empty() {
                i += 2;
                ns.get(i).map(|s| s.to_string())
            } else {
                Some(m[3].to_owned())
            };
            if let Some(path) = path {
                counts.insert(path, (count(&m[1]), count(&m[2])));
            }
        }
        i += 1;
    }
    let mut i = 0;
    while i < parts.len() {
        let e = parts[i];
        if e.len16() < 4 {
            i += 1;
            continue;
        }
        let xy = e.to16(2);
        let path = e.from16(3).to_owned();
        let from = if xy.contains('R') || xy.contains('C') {
            i += 1;
            parts.get(i).map(|s| s.to_string())
        } else {
            None
        };
        let status = if xy == "??" {
            "?"
        } else if xy.contains('R') {
            "R"
        } else if xy.contains('D') {
            "D"
        } else if xy.contains('A') {
            "A"
        } else {
            "M"
        };
        let (add, del) = counts.get(&path).copied().unwrap_or((0, 0));
        out.files.push(GitFile { path, from: from.filter(|f| !f.is_empty()), status, add, del });
        i += 1;
    }
    out
}

/// The working tree against the last commit: branch, how far from its upstream, and every changed file.
pub async fn git_status(host_id: &str, pane_id: &str) -> HubResult<GitState> {
    let (host, cwd) = folder(host_id, pane_id)?;
    let sep = "\u{1}INYON\u{1}";
    let r = host
        .exec(
            &format!(
                "cd {} 2>/dev/null && command -v git >/dev/null && {GIT} rev-parse --show-toplevel 2>/dev/null || exit 3; printf '{sep}'; {GIT} status --porcelain=v1 -z --branch --untracked-files=all 2>/dev/null | head -c 1000000; printf '{sep}'; {{ {GIT} diff HEAD --numstat -z 2>/dev/null || {GIT} diff --cached --numstat -z 2>/dev/null; }} | head -c 1000000; printf '{sep}'; {GIT} remote 2>/dev/null | head -1",
                shq(&cwd)
            ),
            ExecOptions::timeout(20000),
        )
        .await?;
    if r.code == 3 || !r.stdout.contains(sep) {
        return Ok(GitState { repo: false, root: None, remote: None, status: ParsedStatus::default() });
    }
    let parts: Vec<&str> = r.stdout.split(sep).collect();
    let s = parse_status(parts.get(1).copied().unwrap_or(""), parts.get(2).copied().unwrap_or(""));
    Ok(GitState { repo: true, root: Some(parts[0].jtrim().to_owned()), remote: Some(!parts.get(3).copied().unwrap_or("").jtrim().is_empty()), status: s })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<String>,
}

/// A unified diff's hunks, in the shape the app's diff view draws.
pub fn parse_hunks(diff: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = vec![];
    for line in diff.split('\n') {
        if let Some(m) = re!(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").captures(line) {
            let n = |i: usize| m.get(i).map_or(1, |g| count(g.as_str()));
            hunks.push(Hunk { old_start: count(&m[1]), old_lines: n(2), new_start: count(&m[3]), new_lines: n(4), lines: vec![] });
        } else if let Some(cur) = hunks.last_mut() {
            if re!(r"^[ +\-\\]").is_match(line) {
                cur.lines.push(line.to_owned());
            }
        }
    }
    hunks
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitDiff {
    pub path: String,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// One file's changes since the last commit; a file git doesn't track yet shows as all new.
pub async fn git_diff(host_id: &str, pane_id: &str, rel: &str) -> HubResult<GitDiff> {
    let (host, cwd) = folder(host_id, pane_id)?;
    let path = inside(rel)?;
    if path.is_empty() {
        return Err(http(400, "Bad path"));
    }
    let p = shq(&path);
    let r = host
        .exec(
            &format!(
                "cd \"$({GIT} -C {} rev-parse --show-toplevel)\" || exit 3; if {GIT} ls-files --error-unmatch -- {p} >/dev/null 2>&1; then {GIT} diff HEAD --no-ext-diff -U3 -- {p} 2>/dev/null || {GIT} diff --cached --no-ext-diff -U3 -- {p}; else {GIT} diff --no-index --no-ext-diff -U3 -- /dev/null {p}; fi | head -c 2000000",
                shq(&cwd)
            ),
            ExecOptions::timeout(20000),
        )
        .await?;
    if r.code == 3 {
        return Err(http(409, "Not a git repository"));
    }
    Ok(GitDiff { path, binary: re!(r"^Binary files ", "m").is_match(&r.stdout), hunks: parse_hunks(&r.stdout) })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCommitted {
    pub commit: String,
}

/// The last few lines of what git said, or `fallback` when it said nothing.
fn last_lines(text: &str, fallback: &str) -> String {
    let lines: Vec<&str> = text.jtrim().split('\n').collect();
    let tail = lines[lines.len().saturating_sub(6)..].join("\n");
    if tail.is_empty() {
        fallback.to_owned()
    } else {
        tail
    }
}

/// Commits everything in the working tree with your message, as `git add -A && git commit` would.
/// Git's own hooks and signing run as usual; one that needs a password typed fails instead of waiting.
/// `message` is the request's field as it came (`String(message ?? '')`).
pub async fn git_commit(host_id: &str, pane_id: &str, message: Option<&Value>) -> HubResult<GitCommitted> {
    let (host, cwd) = folder(host_id, pane_id)?;
    let raw = match message {
        None | Some(Value::Null) => String::new(),
        Some(v) => crate::js::json::value_string(v),
    };
    let msg = raw.jtrim();
    if msg.is_empty() {
        return Err(http(400, "Write a commit message"));
    }
    if msg.len16() > 10000 || msg.contains('\0') {
        return Err(http(400, "That message is too long"));
    }
    let r = host.exec(&format!("cd {} && {GIT} add -A && {GIT} commit -q -F - && {GIT} log -1 --format=%h", shq(&cwd)), ExecOptions::input(msg).with_timeout(60000)).await?;
    if r.code != 0 {
        let said = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
        return Err(http(409, last_lines(said, "Could not commit")));
    }
    Ok(GitCommitted { commit: r.stdout.jtrim().split('\n').next_back().unwrap_or("").to_owned() })
}

/// What a push answers: nothing (`{}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitPushed {}

/// Pushes the branch to its upstream, or to origin under the same name when it has none yet.
pub async fn git_push(host_id: &str, pane_id: &str) -> HubResult<GitPushed> {
    let (host, cwd) = folder(host_id, pane_id)?;
    let r = host
        .exec(
            &format!(
                "cd {} && if {GIT} rev-parse --abbrev-ref --symbolic-full-name '@{{u}}' >/dev/null 2>&1; then {GIT} push -q; else {GIT} push -q -u origin HEAD; fi < /dev/null",
                shq(&cwd)
            ),
            ExecOptions::timeout(90000),
        )
        .await?;
    if r.code != 0 {
        return Err(http(409, last_lines(&r.stderr, "Could not push")));
    }
    Ok(GitPushed {})
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
