//! `inyon temp`: the temporary files coding agents leave on this machine (pasted images,
//! screenshots, scratchpads, shell snapshots), listed per agent so they can be looked at and
//! deleted. The hubs run `inyon temp … --json` on each machine for the app's Temp files screen, and
//! the Windows app calls these functions in-process.
//!
//! Only folders that hold throwaway files are listed. Transcripts and history (~/.claude/projects,
//! ~/.codex/sessions, file-history, Gemini's saved chats) are never listed or deleted: losing them
//! loses sessions. Every path a caller hands in must be strictly inside one of the roots, after
//! links are resolved, and a link is deleted as a link, never followed.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use serde_json::json;

use crate::commands::Outcome;
use crate::paths;

/// Changed this recently, an agent may still be using it: the app asks twice, `clean` skips it.
/// An item that couldn't be read all the way through counts as recent too.
pub const RECENT_MS: f64 = 30.0 * 60_000.0;
/// The most files `files` lists for one item.
pub const FILES_MAX: usize = 5000;
/// Text previews stop here.
pub const TEXT_MAX: u64 = 256 * 1024;
/// Images bigger than this aren't sent whole.
pub const IMAGE_MAX: u64 = 8 * 1024 * 1024;
/// How many images an item names for its thumbnails.
const THUMBS: usize = 4;
/// Deeper than this, a folder isn't walked any further (a link loop can't happen, links aren't
/// followed, but a runaway tree can).
pub const DEPTH_MAX: usize = 64;

pub const NO_PATH: &str = "Give a path from inyon temp ls";
pub const NOT_ABSOLUTE: &str = "Give the full path, as inyon temp ls shows it";
pub const DOTS: &str = "Paths with . or .. in them aren't accepted";
pub const NOT_FOUND: &str = "Nothing is there (already deleted?)";
pub const OUTSIDE: &str = "That isn't inside one of the temp folders Inyon lists";
pub const LINK_OUTSIDE: &str = "That leads outside the temp folders through a link";
pub const GEMINI_CHATS: &str = "That holds Gemini CLI's saved chats or history, which Inyon doesn't delete";
pub const A_LINK: &str = "That's a link, so there's nothing to preview";
pub const NOT_A_FILE: &str = "That isn't a file";
pub const BAD_AGE: &str = "Give an age like 7d, 24h or 30m";

/// A folder agents keep temporary files in.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Root {
    /// claude-tmp, claude-paste, claude-shell, inyon-uploads, gemini-tmp, codex-shell
    pub id: String,
    /// the agent that writes it: claude, inyon, gemini or codex
    pub agent: String,
    pub label: String,
    pub path: PathBuf,
}

/// A root with what's in it.
#[derive(Debug, Clone, Serialize)]
pub struct RootSummary {
    #[serde(flatten)]
    pub root: Root,
    pub size: u64,
    pub items: u64,
    pub exists: bool,
}

/// One thing to delete or keep: a Claude session's folder, a day of uploads, a loose file.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// `<root id>:<path inside the root>`
    pub id: String,
    pub root: String,
    pub path: String,
    pub name: String,
    /// "dir" or "file"
    pub kind: String,
    pub size: u64,
    pub files: u64,
    /// the newest change inside, in ms since 1970
    pub modified: f64,
    pub recent: bool,
    /// up to four images inside, newest first, for thumbnails
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub v: u32,
    /// this machine's name
    pub host: String,
    pub roots: Vec<RootSummary>,
    pub items: Vec<Item>,
}

/// A file inside an item.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub path: String,
    /// its path inside the item, with forward slashes
    pub rel: String,
    pub size: u64,
    pub modified: f64,
    /// "image", "text" or "other"
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub path: String,
    pub mime: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    /// only part of the text was read, or the image was too big to send
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Removed {
    pub removed: Vec<String>,
    pub freed: u64,
    /// each path that stayed, and why
    pub failed: Vec<(String, String)>,
}

// ---------- where they are ----------

/// The folder temp folders go in: what Claude Code uses, CLAUDE_CODE_TMPDIR or the system's
/// (TMPDIR or /tmp; TEMP on Windows).
pub fn temp_base() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_CODE_TMPDIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    for k in ["TEMP", "TMP"] {
        if let Some(d) = std::env::var_os(k).filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
    }
    std::env::temp_dir()
}

/// Claude Code's own folder under the temp folder: `claude-<uid>`. Windows has no uid, so it is
/// `claude-0` there; versions before that used plain `claude`.
fn claude_tmp_names() -> Vec<String> {
    #[cfg(unix)]
    {
        // SAFETY: getuid can't fail and touches no memory
        vec![format!("claude-{}", unsafe { libc::getuid() })]
    }
    #[cfg(not(unix))]
    {
        vec!["claude-0".into(), "claude".into()]
    }
}

fn root(id: &str, agent: &str, label: &str, path: PathBuf) -> Root {
    Root { id: id.into(), agent: agent.into(), label: label.into(), path }
}

/// Every root this machine could have, there or not, for a home folder and a temp folder.
pub fn known_roots(home: &Path, tmp: &Path) -> Vec<Root> {
    let mut out = Vec::new();
    for (i, name) in claude_tmp_names().iter().enumerate() {
        let p = tmp.join(name);
        // the older Windows name only counts when it is there
        if i == 0 || p.is_dir() {
            out.push(root(if i == 0 { "claude-tmp" } else { "claude-tmp-old" }, "claude", "Claude Code temp files", p));
        }
    }
    out.push(root("claude-paste", "claude", "Claude Code pasted text", home.join(".claude").join("paste-cache")));
    out.push(root("claude-shell", "claude", "Claude Code shell snapshots", home.join(".claude").join("shell-snapshots")));
    out.push(root("inyon-uploads", "inyon", "Inyon uploads", home.join(".inyon").join("uploads")));
    out.push(root("gemini-tmp", "gemini", "Gemini CLI temp files", home.join(".gemini").join("tmp")));
    out.push(root("codex-shell", "codex", "Codex shell snapshots", home.join(".codex").join("shell_snapshots")));
    out
}

/// The roots that are there, for a home folder and a temp folder (what tests point elsewhere).
pub fn roots_in(home: &Path, tmp: &Path) -> Vec<Root> {
    known_roots(home, tmp).into_iter().filter(|r| r.path.is_dir()).collect()
}

/// The roots on this machine that are there.
pub fn roots() -> Vec<Root> {
    roots_in(&paths::home(), &temp_base())
}

// ---------- listing ----------

/// Everything in the temp folders, one item per meaningful unit.
pub fn list(now_ms: f64) -> Listing {
    list_from(&known_roots(&paths::home(), &temp_base()), now_ms)
}

/// `list` over the roots given.
pub fn list_from(roots: &[Root], now_ms: f64) -> Listing {
    let mut summaries = Vec::new();
    let mut items = Vec::new();
    let mut names = ProjectNames::default();
    for r in roots {
        let exists = r.path.is_dir();
        let mut mine: Vec<Item> = if exists { units(r, &mut names).into_iter().map(|(p, name)| item(r, &p, name, now_ms)).collect() } else { Vec::new() };
        mine.sort_by(|a, b| b.modified.total_cmp(&a.modified));
        summaries.push(RootSummary { root: r.clone(), size: mine.iter().map(|i| i.size).sum(), items: mine.len() as u64, exists });
        items.extend(mine);
    }
    Listing { v: 1, host: inyon_engine::hostname(), roots: summaries, items }
}

fn entries(dir: &Path) -> Vec<(String, PathBuf, fs::FileType)> {
    let mut out: Vec<_> =
        fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.path(), e.file_type().ok()?))).collect()).unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// What a root's items are, as (path, readable name).
fn units(r: &Root, names: &mut ProjectNames) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for (name, path, ft) in entries(&r.path) {
        let project_dir = ft.is_dir() && !ft.is_symlink();
        if r.id.starts_with("claude-tmp") && project_dir && is_project_slug(&name) {
            // a project's folder holds one folder per session (images, scratchpad, tasks)
            let project = names.get(&name);
            for (child, cpath, _) in entries(&path) {
                let label = if is_uuid(&child) { child[..8].to_string() } else { child.clone() };
                out.push((cpath, format!("{project} · {label}")));
            }
        } else if r.id == "gemini-tmp" && project_dir {
            // a project's folder: its saved chats and history stay out of reach
            for (child, cpath, _) in entries(&path) {
                if !gemini_kept(&child) {
                    out.push((cpath, format!("{name} · {child}")));
                }
            }
        } else {
            out.push((path, name));
        }
    }
    out
}

/// What Gemini CLI keeps in a project's temp folder that is history, not scratch: its saved chats
/// (for /chat resume), the prompt log, shell history and checkpoints.
fn gemini_kept(name: &str) -> bool {
    matches!(name, "chats" | "logs.json" | "shell_history") || name.starts_with("checkpoint")
}

fn item(r: &Root, path: &Path, name: String, now_ms: f64) -> Item {
    let m = measure(path);
    let rel = path.strip_prefix(&r.path).map(slashed).unwrap_or_default();
    Item {
        id: format!("{}:{rel}", r.id),
        root: r.id.clone(),
        path: text(path),
        name,
        kind: if m.dir { "dir" } else { "file" }.into(),
        size: m.size,
        files: m.files,
        modified: m.modified,
        // what couldn't be read may have changed a moment ago: never call that old
        recent: !m.complete || now_ms - m.modified < RECENT_MS,
        images: m.images.into_iter().map(|(_, p)| p).collect(),
    }
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn slashed(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn mtime_ms(m: &fs::Metadata) -> f64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

#[derive(Default)]
struct Measure {
    dir: bool,
    size: u64,
    files: u64,
    modified: f64,
    /// (mtime, path), newest first
    images: Vec<(f64, String)>,
    /// everything in it was read: no folder too deep or unreadable, nothing gone mid-walk
    complete: bool,
}

/// What a file adds to an item: a link counts as nothing (its own length is the target's name).
fn disk_len(meta: &fs::Metadata) -> u64 {
    if meta.file_type().is_symlink() {
        0
    } else {
        meta.len()
    }
}

/// Size, file count, newest change and newest images of a file or folder, links not followed.
fn measure(path: &Path) -> Measure {
    let mut m = Measure::default();
    let Ok(meta) = fs::symlink_metadata(path) else { return m };
    m.modified = mtime_ms(&meta);
    m.complete = true;
    if !meta.is_dir() {
        m.size = disk_len(&meta);
        m.files = 1;
        if is_image(path) && meta.is_file() {
            m.images.push((m.modified, text(path)));
        }
        return m;
    }
    m.dir = true;
    m.complete = walk(path, &mut |p, meta| {
        let t = mtime_ms(meta);
        m.modified = m.modified.max(t);
        if !meta.is_dir() {
            m.size += disk_len(meta);
            m.files += 1;
            if meta.is_file() && is_image(p) {
                m.images.push((t, text(p)));
                m.images.sort_by(|a, b| b.0.total_cmp(&a.0));
                m.images.truncate(THUMBS);
            }
        }
    });
    m
}

/// Calls `f` for everything under `dir` (not `dir` itself), never following a link. False when
/// something was left out: a folder it couldn't read, an entry it couldn't look at, or a folder
/// deeper than DEPTH_MAX.
fn walk(dir: &Path, f: &mut dyn FnMut(&Path, &fs::Metadata)) -> bool {
    let mut complete = true;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            complete = false;
            continue;
        };
        for e in rd {
            let found = e.ok().map(|e| e.path()).and_then(|p| fs::symlink_metadata(&p).ok().map(|m| (p, m)));
            let Some((p, meta)) = found else {
                complete = false;
                continue;
            };
            f(&p, &meta);
            if meta.is_dir() {
                if depth < DEPTH_MAX {
                    stack.push((p, depth + 1));
                } else {
                    complete = false;
                }
            }
        }
    }
    complete
}

const IMAGE_EXT: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "ico", "svg"];
const TEXT_EXT: [&str; 44] = [
    "txt", "md", "markdown", "json", "jsonl", "js", "mjs", "cjs", "ts", "tsx", "jsx", "rs", "py", "sh", "bash", "zsh", "ps1", "html", "htm", "css", "log", "yaml", "yml", "toml",
    "csv", "tsv", "xml", "output", "diff", "patch", "go", "java", "kt", "c", "h", "cpp", "rb", "php", "sql", "ini", "cfg", "env", "lua", "swift",
];

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn is_image(p: &Path) -> bool {
    IMAGE_EXT.contains(&ext(p).as_str())
}

fn mime_of(p: &Path) -> &'static str {
    match ext(p).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "md" | "markdown" => "text/markdown",
        "json" | "jsonl" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "pdf" => "application/pdf",
        e if TEXT_EXT.contains(&e) => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Whether bytes read from the start of a file look like text: no NUL, and UTF-8 apart from a
/// character cut off at the end.
fn looks_like_text(b: &[u8]) -> bool {
    if b.contains(&0) {
        return false;
    }
    match std::str::from_utf8(b) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

/// The first `n` bytes of a regular file.
fn head(p: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    fs::File::open(p)?.take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

fn kind_of(p: &Path, meta: &fs::Metadata) -> &'static str {
    if is_image(p) {
        return "image";
    }
    let e = ext(p);
    if TEXT_EXT.contains(&e.as_str()) {
        return "text";
    }
    // no telling extension: a look at the start decides (only plain files, a FIFO would hang)
    if meta.is_file() && meta.len() > 0 && head(p, 1024).is_ok_and(|b| looks_like_text(&b)) {
        return "text";
    }
    "other"
}

// ---------- readable project names ----------

/// Claude Code names a project's temp folder after its path with every character that isn't a
/// letter or digit turned into a dash (`/home/me/dev/custom-tmux` is `-home-me-dev-custom-tmux`),
/// so where one word ends is lost. Walking the disk from the top finds the folder the name was made
/// from, and its last part is the readable name.
#[derive(Default)]
struct ProjectNames(HashMap<String, String>);

impl ProjectNames {
    fn get(&mut self, slug: &str) -> String {
        self.0.entry(slug.to_string()).or_insert_with(|| project_name(slug, &paths::home())).clone()
    }
}

fn dashed(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// `-home-…` on Linux and macOS, `C--Users-…` on Windows.
fn is_project_slug(name: &str) -> bool {
    let b = name.as_bytes();
    name.starts_with('-') || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b'-' && b[2] == b'-')
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// The readable name of a project folder's slug: the last part of the folder it came from; when
/// that folder is gone, what's left of the slug past the deepest folder still there (a deleted
/// worktree reads as its own name), or past the home folder's part.
pub fn project_name(slug: &str, home: &Path) -> String {
    let whole = slug.trim_start_matches('-');
    let mut left = whole.to_string();
    if let Some(p) = resolve_slug(slug, &mut left) {
        if let Some(n) = p.file_name() {
            return n.to_string_lossy().into_owned();
        }
    }
    if !left.is_empty() && left.len() < whole.len() {
        return left;
    }
    let home_slug = dashed(&text(home));
    if slug == home_slug {
        return home.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "~".into());
    }
    let rest = slug.strip_prefix(&home_slug).and_then(|r| r.strip_prefix('-')).unwrap_or(slug).trim_start_matches('-');
    if rest.is_empty() {
        slug.to_string()
    } else {
        rest.to_string()
    }
}

/// The folder a slug was made from, if it is still there; `left` ends as the shortest part of the
/// slug no folder matched.
fn resolve_slug(slug: &str, left: &mut String) -> Option<PathBuf> {
    let b = slug.as_bytes();
    let (start, rest) = if let Some(rest) = slug.strip_prefix('-') {
        (PathBuf::from("/"), rest)
    } else if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b'-' && b[2] == b'-' {
        (PathBuf::from(format!("{}:\\", b[0] as char)), &slug[3..])
    } else {
        return None;
    };
    find_slug(&start, rest, 0, left)
}

fn find_slug(dir: &Path, rest: &str, depth: usize, left: &mut String) -> Option<PathBuf> {
    if rest.len() < left.len() {
        *left = rest.to_string();
    }
    if rest.is_empty() {
        return Some(dir.to_path_buf());
    }
    if depth > 32 {
        return None;
    }
    let mut tries: Vec<(usize, PathBuf)> = Vec::new();
    for e in fs::read_dir(dir).ok()?.flatten().take(20_000) {
        let name = e.file_name().to_string_lossy().into_owned();
        let d = dashed(&name);
        if d.is_empty() || !rest.starts_with(&d) || !e.path().is_dir() {
            continue;
        }
        if rest.len() == d.len() {
            return Some(e.path());
        }
        if rest.as_bytes()[d.len()] == b'-' {
            tries.push((d.len(), e.path()));
        }
    }
    // the longest name first: `custom-tmux` before `custom`
    tries.sort_by_key(|t| std::cmp::Reverse(t.0));
    tries.into_iter().find_map(|(n, p)| find_slug(&p, &rest[n + 1..], depth + 1, left))
}

// ---------- checking a path ----------

/// Where a path really is and which root holds it, or why it can't be touched. The path's last
/// part is never followed: a link is judged where it sits.
pub fn locate<'a>(roots: &'a [Root], p: &str) -> Result<(PathBuf, &'a Root), String> {
    if p.trim().is_empty() {
        return Err(NO_PATH.into());
    }
    let path = Path::new(p);
    if !path.is_absolute() {
        return Err(NOT_ABSOLUTE.into());
    }
    if path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) || p.split(['/', '\\']).any(|s| s == "." || s == "..") {
        return Err(DOTS.into());
    }
    let real_roots: Vec<Option<PathBuf>> = roots.iter().map(|r| fs::canonicalize(&r.path).ok()).collect();
    // nowhere near a root: say so before looking whether it's there at all
    let near = roots.iter().zip(&real_roots).any(|(r, real)| path.starts_with(&r.path) || real.as_ref().is_some_and(|real| path.starts_with(real)));
    if !near {
        return Err(OUTSIDE.into());
    }
    if let Some(r) = roots.iter().find(|r| path == r.path) {
        return Err(whole(r));
    }
    if fs::symlink_metadata(path).is_err() {
        return Err(NOT_FOUND.into());
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return Err(OUTSIDE.into()) };
    let real = fs::canonicalize(parent).map_err(|_| NOT_FOUND.to_string())?.join(name);
    for (r, real_root) in roots.iter().zip(&real_roots) {
        let Some(real_root) = real_root else { continue };
        if real == *real_root {
            return Err(whole(r));
        }
        if let Ok(rel) = real.strip_prefix(real_root) {
            if r.id == "gemini-tmp" && gemini_guarded(real_root, rel) {
                return Err(GEMINI_CHATS.into());
            }
            return Ok((real, r));
        }
    }
    // inside a root as written, outside once links are resolved
    Err(LINK_OUTSIDE.into())
}

fn whole(r: &Root) -> String {
    format!("That's the whole {} folder. Delete what's inside it instead", r.label)
}

/// A Gemini project folder holding chats or history, or anything in those.
fn gemini_guarded(root: &Path, rel: &Path) -> bool {
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    match parts.len() {
        0 => false,
        1 => entries(&root.join(&parts[0])).iter().any(|(n, _, _)| gemini_kept(n)),
        _ => gemini_kept(&parts[1]),
    }
}

// ---------- inside an item ----------

/// Every file inside one item, newest first, at most FILES_MAX.
pub fn files(item_path: &str) -> Result<Vec<FileEntry>, String> {
    files_in(&roots(), item_path).map(|(f, _)| f)
}

/// `files` over the roots given, and whether the list was cut at FILES_MAX.
pub fn files_in(roots: &[Root], item_path: &str) -> Result<(Vec<FileEntry>, bool), String> {
    locate(roots, item_path)?;
    let base = Path::new(item_path);
    let meta = fs::symlink_metadata(base).map_err(|_| NOT_FOUND.to_string())?;
    let mut out = Vec::new();
    let entry = |p: &Path, meta: &fs::Metadata, rel: String| FileEntry { path: text(p), rel, size: meta.len(), modified: mtime_ms(meta), kind: kind_of(p, meta).into() };
    if !meta.is_dir() {
        out.push(entry(base, &meta, base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    } else {
        walk(base, &mut |p, meta| {
            if !meta.is_dir() {
                let rel = p.strip_prefix(base).map(slashed).unwrap_or_default();
                // the cheap fields now; the kind (which may read the file) only for those kept
                out.push(FileEntry { path: text(p), rel, size: meta.len(), modified: mtime_ms(meta), kind: String::new() });
            }
        });
        out.sort_by(|a, b| b.modified.total_cmp(&a.modified).then_with(|| a.rel.cmp(&b.rel)));
    }
    let cut = out.len() > FILES_MAX;
    out.truncate(FILES_MAX);
    for f in out.iter_mut().filter(|f| f.kind.is_empty()) {
        let p = PathBuf::from(&f.path);
        f.kind = fs::symlink_metadata(&p).map(|m| kind_of(&p, &m)).unwrap_or("other").into();
    }
    Ok((out, cut))
}

/// One file's contents for a look: text up to TEXT_MAX (and `max_bytes`), an image as base64 up
/// to `max_bytes`, neither for anything else.
pub fn preview(path: &str, max_bytes: u64) -> Result<Preview, String> {
    preview_in(&roots(), path, max_bytes)
}

pub fn preview_in(roots: &[Root], path: &str, max_bytes: u64) -> Result<Preview, String> {
    locate(roots, path)?;
    let p = Path::new(path);
    let meta = fs::symlink_metadata(p).map_err(|_| NOT_FOUND.to_string())?;
    if meta.file_type().is_symlink() {
        return Err(A_LINK.into());
    }
    if !meta.is_file() {
        return Err(NOT_A_FILE.into());
    }
    let size = meta.len();
    let mime = mime_of(p).to_string();
    let mut out = Preview { path: path.into(), mime, size, text: None, base64: None, truncated: false };
    if is_image(p) {
        if size > max_bytes {
            out.truncated = true;
        } else {
            let b = head(p, max_bytes).map_err(|e| e.to_string())?;
            out.base64 = Some(base64::engine::general_purpose::STANDARD.encode(b));
        }
        return Ok(out);
    }
    let limit = TEXT_MAX.min(max_bytes.max(1));
    let b = head(p, limit).map_err(|e| e.to_string())?;
    if looks_like_text(&b) {
        let text = match std::str::from_utf8(&b) {
            Ok(t) => t.to_string(),
            Err(e) => String::from_utf8_lossy(&b[..e.valid_up_to()]).into_owned(),
        };
        if out.mime == "application/octet-stream" {
            out.mime = "text/plain".into();
        }
        out.truncated = size > b.len() as u64;
        out.text = Some(text);
    }
    Ok(out)
}

// ---------- deleting ----------

/// Deletes each path for good, after checking it is strictly inside a root. A link goes, never
/// what it points at.
pub fn remove(paths: &[String]) -> Removed {
    remove_in(&roots(), paths)
}

pub fn remove_in(roots: &[Root], paths: &[String]) -> Removed {
    let mut out = Removed::default();
    let mut gone: Vec<PathBuf> = Vec::new();
    // parents first, so a path inside one already deleted isn't reported as missing
    let mut sorted: Vec<&String> = paths.iter().collect();
    sorted.sort_by_key(|p| Path::new(p.as_str()).components().count());
    sorted.dedup();
    for p in sorted {
        if gone.iter().any(|g| Path::new(p.as_str()).starts_with(g)) {
            out.removed.push(p.clone());
            continue;
        }
        let real = match locate(roots, p) {
            Ok((real, _)) => real,
            Err(why) => {
                out.failed.push((p.clone(), why));
                continue;
            }
        };
        let before = measure(&real).size;
        match delete(&real) {
            Ok(()) => {
                out.freed += before;
                out.removed.push(p.clone());
                gone.push(PathBuf::from(p));
            }
            Err(e) => {
                // part of a folder may have gone before the error
                out.freed += before.saturating_sub(measure(&real).size);
                out.failed.push((p.clone(), e.to_string()));
            }
        }
    }
    out
}

fn delete(p: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(p)?;
    if meta.file_type().is_symlink() {
        // a link to a folder is a directory entry on Windows
        return fs::remove_file(p).or_else(|e| if cfg!(windows) { fs::remove_dir(p) } else { Err(e) });
    }
    if meta.is_dir() {
        // std doesn't follow links inside the folder either
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

// ---------- the command ----------

/// 1.2 GB, 340 KB, 12 B.
pub fn size_text(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v < 10.0 {
        format!("{v:.1} {}", U[i])
    } else {
        format!("{v:.0} {}", U[i])
    }
}

/// "5 min ago", "3 h ago", "12 d ago".
pub fn age_text(now_ms: f64, at: f64) -> String {
    let s = ((now_ms - at) / 1000.0).max(0.0) as u64;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// `7d`, `24h`, `30m`, `2w` in milliseconds.
pub fn parse_age(s: &str) -> Result<f64, String> {
    let s = s.trim();
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: f64 = n.parse().map_err(|_| BAD_AGE.to_string())?;
    let ms = match unit {
        "m" => 60_000.0,
        "h" => 3_600_000.0,
        "d" => 86_400_000.0,
        "w" => 7.0 * 86_400_000.0,
        _ => return Err(BAD_AGE.into()),
    };
    Ok(n * ms)
}

/// How many of the biggest items the plain list shows.
const SHOWN: usize = 12;

pub fn list_command() -> Outcome {
    let now = crate::state::now_ms();
    let l = list(now);
    let mut v = serde_json::to_value(&l).map_err(|e| e.to_string())?;
    v["message"] = json!(listing_lines(&l, now).join("\n"));
    Ok(v)
}

fn listing_lines(l: &Listing, now: f64) -> Vec<String> {
    let total: u64 = l.roots.iter().map(|r| r.size).sum();
    if l.items.is_empty() {
        return vec![format!("No agent temp files on {}.", l.host)];
    }
    let mut lines = vec![format!("Agent temp files on {}: {} in {} items", l.host, size_text(total), l.items.len())];
    let w = l.roots.iter().map(|r| r.root.label.chars().count()).max().unwrap_or(0);
    for r in l.roots.iter().filter(|r| r.items > 0) {
        lines.push(format!("  {:<w$}  {:>8}  {:>4} items  {}", r.root.label, size_text(r.size), r.items, short(&r.root.path)));
    }
    let mut big: Vec<&Item> = l.items.iter().collect();
    big.sort_by_key(|i| std::cmp::Reverse(i.size));
    lines.push(String::new());
    lines.push(if l.items.len() > SHOWN { format!("The {SHOWN} biggest:") } else { "Items:".into() });
    for i in big.iter().take(SHOWN) {
        let recent = if i.recent { "  (changed in the last 30 min)" } else { "" };
        lines.push(format!("  {:>8}  {:>11}  {}  {}{recent}", size_text(i.size), age_text(now, i.modified), i.name, i.path));
    }
    lines.push(String::new());
    lines.push("Look inside one: inyon temp files <path>. Delete: inyon temp rm <path>…".into());
    lines
}

fn short(p: &Path) -> String {
    let home = paths::home();
    match p.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()).replace('\\', "/"),
        _ => text(p),
    }
}

/// An item's path, or its id from `inyon temp ls --json`.
fn item_path(item: &str) -> Result<String, String> {
    if Path::new(item).is_absolute() {
        return Ok(item.to_string());
    }
    list(crate::state::now_ms()).items.into_iter().find(|i| i.id == item).map(|i| i.path).ok_or_else(|| NOT_ABSOLUTE.to_string())
}

pub fn files_command(item: &str) -> Outcome {
    let path = item_path(item)?;
    let (list, truncated) = files_in(&roots(), &path)?;
    let now = crate::state::now_ms();
    let mut lines: Vec<String> = list.iter().map(|f| format!("  {:>8}  {:>11}  {}", size_text(f.size), age_text(now, f.modified), f.rel)).collect();
    if list.is_empty() {
        lines.push("Nothing inside.".into());
    }
    if truncated {
        lines.push(format!("(the newest {FILES_MAX} only)"));
    }
    Ok(json!({ "path": path, "files": list, "truncated": truncated, "message": lines.join("\n") }))
}

pub fn show_command(path: &str, max: u64) -> Outcome {
    let p = preview(path, max)?;
    let message = match (&p.text, &p.base64) {
        (Some(t), _) if p.truncated => format!("{t}\n(only the first {} of {})", size_text(TEXT_MAX), size_text(p.size)),
        (Some(t), _) => t.clone(),
        _ => format!("{}, {}: open it in the app's Temp files screen to see it", p.mime, size_text(p.size)),
    };
    let mut v = serde_json::to_value(&p).map_err(|e| e.to_string())?;
    v["message"] = json!(message);
    Ok(v)
}

/// The paths to delete: as given, or a JSON array on stdin (how the hubs send many at once, past
/// what a Windows command line holds).
pub fn paths_from(given: Vec<String>, stdin: bool) -> Result<Vec<String>, String> {
    if !stdin {
        return Ok(given);
    }
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
    serde_json::from_str::<Vec<String>>(&s).map_err(|_| "Expected a JSON list of paths on stdin".to_string())
}

/// `inyon temp rm`; the second value is whether anything stayed.
pub fn rm_command(paths: &[String]) -> (Outcome, bool) {
    if paths.is_empty() {
        return (Err(NO_PATH.into()), true);
    }
    let r = remove(paths);
    let partial = !r.failed.is_empty();
    (removed_outcome(&r), partial)
}

fn removed_outcome(r: &Removed) -> Outcome {
    let mut lines = Vec::new();
    if !r.removed.is_empty() {
        lines.push(format!("Deleted {} ({} freed)", count(r.removed.len(), "item"), size_text(r.freed)));
    }
    for (p, why) in &r.failed {
        lines.push(format!("Kept {p}: {why}"));
    }
    let mut v = serde_json::to_value(r).map_err(|e| e.to_string())?;
    if r.removed.is_empty() && !r.failed.is_empty() {
        v["ok"] = json!(false);
    }
    v["message"] = json!(lines.join("\n"));
    Ok(v)
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// `inyon temp clean --older-than <age>`: items untouched for longer than `age`, leaving any
/// changed in the last 30 minutes. Without --yes it asks in a terminal, and only lists otherwise.
pub fn clean_command(older_than: &str, yes: bool, ask: bool) -> Outcome {
    let age = parse_age(older_than)?;
    let now = crate::state::now_ms();
    let old: Vec<Item> = list(now).items.into_iter().filter(|i| !i.recent && now - i.modified > age).collect();
    let size: u64 = old.iter().map(|i| i.size).sum();
    if old.is_empty() {
        return Ok(json!({ "removed": [], "freed": 0, "failed": [], "message": format!("Nothing older than {older_than}.") }));
    }
    let paths: Vec<String> = old.iter().map(|i| i.path.clone()).collect();
    if !yes {
        let mut lines: Vec<String> = old.iter().map(|i| format!("  {:>8}  {:>11}  {}", size_text(i.size), age_text(now, i.modified), i.name)).collect();
        lines.push(format!("{} older than {older_than}, {}.", count(old.len(), "item"), size_text(size)));
        if !ask {
            lines.push("Nothing deleted. Add --yes to delete them.".into());
            return Ok(json!({ "would": paths, "size": size, "message": lines.join("\n") }));
        }
        eprintln!("{}", lines.join("\n"));
        if !crate::ui::ask_yes_no("Delete them for good?", false) {
            return Ok(json!({ "removed": [], "freed": 0, "failed": [], "message": "Nothing deleted." }));
        }
    }
    removed_outcome(&remove(&paths))
}
