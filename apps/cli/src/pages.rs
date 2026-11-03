//! `inyon pages`: the sites agents make (Blueprint plans, mockups), listed in the Inyon app under
//! Pages. The list is `~/.inyon/pages.json`, which this command, Blueprint's publish script and the
//! hub all write, and which the hub reads again whenever it changes, so adding a page needs no hub
//! running. Each site also gets `.inyon-page.json` in its own folder, so it keeps its slug and name.
//!
//!   ~/.inyon/pages.json     {"v":1,"pages":[{"slug","name","path","project","by","added","session"?}]}
//!   <site>/.inyon-page.json {"v":1,"slug","name","kind":"blueprint","created"}
//!
//! The rules (slugs, which folders may be pages, the messages) are the hubs' own, from
//! crates/inyon-core/src/features/pages_core.rs; tests/pages.rs checks them against fixtures/pages/rules.json.
//! pages.json is changed while holding `pages.json.lock` (created exclusively, taken over once it is
//! 10 s old), by writing a temp file and renaming it over the old one. Entries this program can't
//! read are kept as they are.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::commands::Outcome;
use crate::paths;

/// The host id of the hub's own machine: app links are /pages/<host>/<slug>.
pub const LOCAL_HOST: &str = "local";
pub const BAD_SLUG: &str = "A slug is lowercase letters, digits and dashes, up to 63 characters, starting with a letter or digit";
pub const NOT_ABSOLUTE: &str = "The folder must be an absolute path";
pub const TOO_WIDE: &str = "Pick the site's own folder, not / or your home folder";
pub const PRIVATE_FOLDER: &str = "Folders under ~/.ssh and ~/.inyon can't be pages";
pub const NO_INDEX: &str = "That folder has no index.html";
pub const BROKEN_FILE: &str = "~/.inyon/pages.json isn't a page list Inyon can read. Fix it or delete it, then try again.";
pub const LOCKED: &str = "Another program is changing ~/.inyon/pages.json. Try again in a moment.";
/// A lock older than this was left by a writer that died, and is taken over.
pub const LOCK_STALE: Duration = Duration::from_secs(10);
/// How long to wait for another writer before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// Names are cut to this many characters, as the hub cuts them.
const NAME_MAX: usize = 200;
/// How much of index.html is read for its <title>.
const TITLE_READ: usize = 256 * 1024;
/// A bigger `.inyon-page.json` isn't read for a page's name and slug.
const MARKER_MAX: u64 = 64 * 1024;
/// The most of pages.json this reads, as both hubs and Blueprint (`PAGES_LIST_MAX` in
/// pages-core.ts): a bigger file, a sparse one claiming terabytes too, is a list it can't read.
pub const LIST_MAX: u64 = 4 * 1024 * 1024;
/// A lock holds a process id and a tag; anything longer isn't one of ours.
const LOCK_MAX: u64 = 256;
/// How long the git calls that find a page's project may take, together, before git is stopped.
pub const GIT_TIMEOUT: Duration = Duration::from_millis(3000);
/// Set for those calls: no machine-wide git config, and no lock files taken just to look.
const GIT_ENV: [(&str, &str); 2] = [("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_OPTIONAL_LOCKS", "0")];

// ---------- the shared rules ----------

/// `^[a-z0-9][a-z0-9-]{0,62}$`
pub fn is_slug(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 63 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// ASCII letters lowercased, every other run one dash, no dash at either end, at most 63
/// characters; "page" when nothing is left.
pub fn slugify(text: &str) -> String {
    let mut s = String::new();
    let mut dash = false;
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !s.is_empty() {
                s.push('-');
            }
            dash = false;
            s.push(c);
        } else {
            dash = true;
        }
    }
    s.truncate(63);
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        "page".into()
    } else {
        s
    }
}

/// `base`, or `base-2`, `base-3`… (cut so the whole stays 63 characters), whichever is free first.
pub fn unique_slug(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    for i in 2u64.. {
        let suffix = format!("-{i}");
        let keep: String = base.chars().take(63usize.saturating_sub(suffix.len())).collect();
        let s = format!("{}{suffix}", keep.trim_end_matches('-'));
        if !taken(&s) {
            return s;
        }
    }
    unreachable!("an endless range ran out")
}

/// What is wrong with a folder as a page, before the disk is asked (the caller still checks for
/// index.html). None when it may be one. Paths are compared as written, trailing slashes dropped;
/// a Windows path (`C:\…`) counts as absolute.
pub fn site_path_problem(p: &str, home: &str) -> Option<&'static str> {
    let b = p.as_bytes();
    let win = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if !(p.starts_with('/') || win) || p.contains('\0') {
        return Some(NOT_ABSOLUTE);
    }
    let parts = |s: &str| -> Vec<String> {
        if win {
            s.split(['\\', '/']).filter(|x| !x.is_empty()).map(String::from).collect()
        } else {
            s.split('/').filter(|x| !x.is_empty()).map(String::from).collect()
        }
    };
    let segs = parts(p);
    if segs.iter().any(|s| s == "." || s == "..") {
        return Some(NOT_ABSOLUTE);
    }
    let home_segs = parts(home);
    if segs.is_empty() || (win && segs.len() == 1) || (!home_segs.is_empty() && segs == home_segs) {
        return Some(TOO_WIDE);
    }
    let under = |dir: &str| {
        let mut d = home_segs.clone();
        d.push(dir.to_string());
        segs.len() >= d.len() && segs[..d.len()] == d[..]
    };
    if !home_segs.is_empty() && (under(".ssh") || under(".inyon")) {
        return Some(PRIVATE_FOLDER);
    }
    None
}

/// The rules above, and on Windows (where names don't care about case) the same again lowercased.
fn folder_problem(p: &str, home: &str) -> Option<&'static str> {
    site_path_problem(p, home).or_else(|| if cfg!(windows) { site_path_problem(&p.to_lowercase(), &home.to_lowercase()) } else { None })
}

// ---------- paths ----------

/// A path with `.` and `..` worked out from the text, as a shell does (symlinks stay).
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The folder someone typed, as an absolute path.
fn absolute(dir: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        // a path copied from somewhere Windows printed it as `\\?\C:\…` is taken in the form people type
        let typed = dir.to_str().map_or_else(|| dir.to_path_buf(), |t| PathBuf::from(strip_verbatim(t)));
        std::path::absolute(&typed).map(|p| lexical(&p)).map_err(|e| format!("{}: {e}", dir.display()))
    }
    #[cfg(not(windows))]
    {
        let cwd = std::env::current_dir().map_err(|e| format!("Can't tell which folder this is: {e}"))?;
        Ok(lexical(&cwd.join(dir)))
    }
}

/// A path Windows gave back as `\\?\C:\…` (or `\\?\UNC\server\…`), in the form people type.
pub fn strip_verbatim(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

fn text_of(p: &Path) -> Result<String, String> {
    p.to_str().map(String::from).ok_or_else(|| format!("{} isn't a name Inyon can store (it isn't UTF-8)", p.display()))
}

/// Where a path really leads, symlinks followed, as text.
fn real(p: &Path) -> Option<String> {
    fs::canonicalize(p).ok().and_then(|r| r.to_str().map(strip_verbatim))
}

/// What is wrong with where a folder really leads. A drive letter that is a mapped network drive
/// really leads to `\\server\share\…`, which the rules don't take as a path typed by hand. There the
/// share stands in for the drive (`Z:\…`), and so does the home folder when it is on the same share:
/// the share itself is too wide, and its home folder's `.ssh` stays private.
fn real_problem(path: &str, real_path: &str, real_home: &str) -> Option<&'static str> {
    let b = path.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if let (true, Some((share, rest))) = (drive, unc_parts(real_path)) {
        let as_drive = |rest: &str| format!("{}:\\{rest}", b[0] as char);
        let home = match unc_parts(real_home) {
            Some((home_share, home_rest)) if home_share.eq_ignore_ascii_case(&share) => as_drive(&home_rest),
            _ => real_home.to_string(),
        };
        return folder_problem(&as_drive(&rest), &home);
    }
    folder_problem(real_path, real_home)
}

/// `\\server\share\rest` as (`\\server\share`, `rest`), for a UNC path.
fn unc_parts(p: &str) -> Option<(String, String)> {
    let body = p.strip_prefix(r"\\")?;
    let mut it = body.splitn(3, ['\\', '/']);
    let (server, share) = (it.next().filter(|s| !s.is_empty())?, it.next().filter(|s| !s.is_empty())?);
    Some((format!(r"\\{server}\{share}"), it.next().unwrap_or("").to_string()))
}

/// `~/.inyon/pages.json`: always under the home folder (not INYON_DATA_DIR), where the hub reads it.
pub fn pages_file() -> PathBuf {
    paths::home().join(".inyon").join("pages.json")
}

/// A path under the home folder as `~/…`.
fn short(p: &str) -> String {
    let home = paths::home();
    match Path::new(p).strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()).replace('\\', "/"),
        _ => p.to_string(),
    }
}

// ---------- what a site says about itself ----------

/// The agent running this command, from the variables each one sets for its shell; "cli" otherwise.
pub fn agent(env: impl Fn(&str) -> Option<String>) -> &'static str {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if set("CLAUDECODE") {
        "claude"
    } else if set("CODEX_SANDBOX") || set("CODEX_SANDBOX_NETWORK_DISABLED") || set("CODEX_THREAD_ID") {
        "codex"
    } else if set("GEMINI_CLI") {
        "gemini"
    } else if set("OPENCODE") {
        "opencode"
    } else {
        "cli"
    }
}

/// The conversation the agent adding a page is in, so the app can show that session's name over
/// the page and follow it when the session is renamed: Claude Code's session id, Codex's thread id.
pub fn agent_session(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID"].iter().filter_map(|k| env(k)).find(|v| is_session(v))
}

/// A conversation id as pages.json keeps it: letters, digits and `._:-`, at most 128 of them.
pub fn is_session(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

/// The text of the first `<title>` in an HTML page, entities decoded and spaces collapsed.
pub fn title_of(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    let open = loop {
        let at = from + lower[from..].find("<title")?;
        let next = lower.as_bytes().get(at + 6).copied();
        if matches!(next, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')) {
            break at;
        }
        from = at + 6;
    };
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let text = decode_entities(&html[start..end]);
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!clean.is_empty()).then(|| clean.chars().take(NAME_MAX).collect())
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // by bytes: ';' is ASCII, so where it is found is always a character boundary
        let Some(semi) = rest.as_bytes()[..rest.len().min(12)].iter().position(|b| *b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn page_title(site: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = Vec::new();
    open_plain(&site.join("index.html"), false).ok()?.0.take(TITLE_READ as u64).read_to_end(&mut head).ok()?;
    title_of(&String::from_utf8_lossy(&head))
}

/// Opens a file to read without ever waiting on it: with O_NONBLOCK, and only when the open file is
/// a plain file. A FIFO left in a site's folder (or as the list) would otherwise hold the read until
/// something wrote to it. A plain file reads the same with O_NONBLOCK set. A symlink at the end of
/// the path is followed only when `follow` says so: the list may be one (a dotfile setup), a file in
/// a site's folder may not. The file and its size.
fn open_plain(path: &Path, follow: bool) -> std::io::Result<(fs::File, u64)> {
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut o, if follow { libc::O_NONBLOCK } else { libc::O_NOFOLLOW | libc::O_NONBLOCK });
    #[cfg(not(unix))]
    let _ = follow;
    let file = o.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(ErrorKind::InvalidData, "not a plain file"));
    }
    Ok((file, meta.len()))
}

/// A plain file's bytes (see open_plain), refused when there are more than `max`.
fn read_plain(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    read_capped(path, max, false)
}

fn read_capped(path: &Path, max: u64, follow: bool) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let too_big = || std::io::Error::new(ErrorKind::InvalidData, "too big");
    let (file, len) = open_plain(path, follow)?;
    if len > max {
        return Err(too_big());
    }
    // a buffer that grows as it reads, never sized from the file, which can claim terabytes it doesn't have
    let mut out = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(too_big());
    }
    Ok(out)
}

/// A name as given: trimmed, at most 200 characters, None when empty.
fn clean_name(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.chars().take(NAME_MAX).collect())
}

struct Marker {
    name: Option<String>,
    slug: Option<String>,
    raw: Option<Map<String, Value>>,
}

fn read_marker(site: &Path) -> Marker {
    let raw = read_plain(&site.join(".inyon-page.json"), MARKER_MAX).ok().and_then(|t| serde_json::from_slice::<Value>(&t).ok()).and_then(|v| match v {
        Value::Object(m) => Some(m),
        _ => None,
    });
    let name = raw.as_ref().and_then(|m| m.get("name")).and_then(Value::as_str).and_then(clean_name);
    let slug = raw.as_ref().and_then(|m| m.get("slug")).and_then(Value::as_str).filter(|s| is_slug(s)).map(String::from);
    Marker { name, slug, raw }
}

/// Writes `<site>/.inyon-page.json`, keeping its first `created`. False when the folder can't be
/// written; the page is listed anyway.
fn write_marker(site: &Path, old: &Marker, slug: &str, name: &str, now: u64) -> bool {
    let created = old.raw.as_ref().and_then(|m| m.get("created")).and_then(Value::as_u64).filter(|n| *n < (1u64 << 53)).unwrap_or(now);
    let want = json!({ "v": 1, "slug": slug, "name": name, "kind": "blueprint", "created": created });
    if old.raw.as_ref().is_some_and(|m| Value::Object(m.clone()) == want) {
        return true;
    }
    write_atomic(&site.join(".inyon-page.json"), &format!("{want}\n"), 0o644).is_ok()
}

/// The git top level of a folder (for a linked worktree, the repo's main checkout), or the folder itself.
fn project_of(folder: &str) -> String {
    project_with(folder, "git")
}

fn project_with(folder: &str, git: &str) -> String {
    match main_tree(folder, git, Instant::now() + GIT_TIMEOUT) {
        None => folder.to_string(),
        Some(top) if cfg!(windows) => top.replace('/', "\\"),
        Some(top) => top,
    }
}

/// The line git prints for `git -C <folder> rev-parse <option>`, or None. A line starting with '-'
/// is refused: a git too old to know the option prints it back and still exits 0 (git before 2.31
/// does that with --path-format, before 2.5 with --git-common-dir). Git is killed at `until`: a
/// named pipe in a folder's `.git` would otherwise keep it waiting for good.
fn rev_parse(git: &str, folder: &str, option: &str, until: Instant) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut c = std::process::Command::new(git);
    c.args(["-C", folder, "rev-parse", option]).envs(GIT_ENV).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
    let mut child = crate::sys::quiet(&mut c).spawn().ok()?;
    // the one line rev-parse prints fits in the pipe, so git never waits on it before exiting
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
            Ok(Some(_)) => return None,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = Vec::new();
    child.stdout.take()?.read_to_end(&mut out).ok()?;
    let line = String::from_utf8_lossy(&out).lines().next().unwrap_or("").trim().to_string();
    (!line.is_empty() && !line.starts_with('-')).then_some(line)
}

/// The repo's main working tree, or None outside a repo. In a linked worktree (an agent's
/// `.claude/worktrees/<name>`, removed once its branch merges) git's common dir is the main tree's
/// `.git`, so the page is grouped under the repo, not the worktree. A submodule's common dir
/// (`.git/modules/<name>`) or a bare repo's (`<name>.git`) keeps the top level. Two plain calls work
/// on any git; the top level git prints is kept when the common dir's parent is the same folder.
/// Both calls stop at `until`.
fn main_tree(folder: &str, git: &str, until: Instant) -> Option<String> {
    let top = rev_parse(git, folder, "--show-toplevel", until)?;
    let Some(common) = rev_parse(git, folder, "--git-common-dir", until).map(|c| lexical(&Path::new(folder).join(c))) else {
        return Some(top);
    };
    if common.file_name().is_none_or(|n| n != ".git") || !common.is_dir() {
        return Some(top);
    }
    let Some(parent) = common.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Some(top);
    };
    match (fs::canonicalize(parent), fs::canonicalize(&top)) {
        (Ok(a), Ok(b)) if a != b => Some(parent.to_str().map_or(top, String::from)),
        _ => Some(top),
    }
}

// ---------- the list file ----------

/// Runs `f` holding `<file>.lock`, made with an exclusive create; a lock older than LOCK_STALE is
/// taken over. The lock holds this process's id and a random tag, and is removed afterwards only
/// while it still holds them. A failure other than "someone else has it" (a folder an agent's
/// sandbox keeps read-only) is reported at once with the sandbox hint, never waited on.
fn with_lock<T>(file: &Path, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    struct Held(PathBuf, String);
    impl Drop for Held {
        fn drop(&mut self) {
            // read as the list is, so a FIFO put in its place isn't waited on
            if read_plain(&self.0, LOCK_MAX).is_ok_and(|t| t == self.1.as_bytes()) {
                let _ = fs::remove_file(&self.0);
            }
        }
    }
    let lock = lock_of(file);
    let until = Instant::now() + LOCK_WAIT;
    let mine = format!("{} {}\n", std::process::id(), random_tag());
    let _held = loop {
        if make_lock(&lock, &mine, |fh, b| fh.write_all(b)).map_err(|e| cant_write(file, &e))? {
            break Held(lock, mine);
        }
        if take_over_stale(&lock).map_err(|e| cant_write(file, &e))? {
            continue;
        }
        if Instant::now() >= until {
            return Err(LOCKED.into());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    f()
}

/// Makes the lock with this writer's tag in it; false when another writer has it. A lock whose tag
/// couldn't be written (a full disk) could never be released, so it is taken away again, while the
/// path still leads to the file this writer made.
fn make_lock(lock: &Path, mine: &str, write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>) -> std::io::Result<bool> {
    let mut fh = match OpenOptions::new().write(true).create_new(true).open(lock) {
        Ok(fh) => fh,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    if let Err(e) = write(&mut fh, mine.as_bytes()) {
        if fh.metadata().ok().zip(fs::metadata(lock).ok()).is_some_and(|(made, now)| same_file(&made, &now)) {
            let _ = fs::remove_file(lock);
        }
        return Err(e);
    }
    Ok(true)
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

// No file id here without another dependency; a lock this young (seconds, not the ten before
// another writer takes one over) is this writer's.
#[cfg(not(unix))]
fn same_file(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    true
}

fn lock_of(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// 16 hex digits, different for every lock this program takes.
fn random_tag() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{:016x}", h.finish())
}

/// The file's identity, to tell a lock judged stale from a new one made in its place.
fn identity(m: &fs::Metadata) -> (u64, Option<std::time::SystemTime>) {
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(m);
    #[cfg(not(unix))]
    let ino = m.len();
    (ino, m.modified().ok())
}

/// Removes a lock left by a writer that died (older than LOCK_STALE); true when the caller should
/// try to take it again at once.
fn take_over_stale(lock: &Path) -> std::io::Result<bool> {
    let old = match fs::metadata(lock) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    if !old.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > LOCK_STALE) {
        return Ok(false);
    }
    remove_judged(lock, &old)?;
    Ok(true)
}

/// Removes the lock that was judged stale as `old`, and only that one: renamed aside, then put back
/// if it turns out to be a new lock another writer made since (two writers can judge one lock stale
/// at once, and the first makes its own before the second acts).
pub fn remove_judged(lock: &Path, old: &fs::Metadata) -> std::io::Result<()> {
    let mut aside = lock.as_os_str().to_owned();
    aside.push(format!(".{}.{}.stale", std::process::id(), &random_tag()[..8]));
    let aside = PathBuf::from(aside);
    match fs::rename(lock, &aside) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    if fs::metadata(&aside).map(|m| identity(&m)).ok() != Some(identity(old)) {
        // another writer's new lock: back where it was, unless a third has made one since
        let _ = fs::hard_link(&aside, lock);
    }
    let _ = fs::remove_file(&aside);
    Ok(())
}

fn cant_write(file: &Path, e: &std::io::Error) -> String {
    format!("Can't write {}: {e}. If an agent's sandbox stopped it, run the same command in your own terminal.", file.display())
}

/// Writes beside the file, flushes, and renames over it, so a reader never sees half a file.
fn write_atomic(file: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    let mut name = file.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(name);
    let mut o = OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let result = (|| {
        let mut fh = o.open(&tmp)?;
        fh.write_all(text.as_bytes())?;
        fh.sync_all()?;
        drop(fh);
        fs::rename(&tmp, file)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The file as `{v: 1, pages: [...]}` (other keys kept), an empty list when it isn't there. The list
/// may be a symlink (a dotfile setup); one that isn't a plain file of at most LIST_MAX bytes is a
/// broken list.
pub fn read_list(file: &Path) -> Result<Map<String, Value>, String> {
    let text = match read_capped(file, LIST_MAX, true).map(String::from_utf8) {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(format!("Can't read {}: {e}", file.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let mut m = Map::new();
            m.insert("v".into(), json!(1));
            m.insert("pages".into(), json!([]));
            return Ok(m);
        }
        Err(e) if e.kind() == ErrorKind::InvalidData => return Err(BROKEN_FILE.into()),
        Err(e) => return Err(format!("Can't read {}: {e}", file.display())),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) if m.get("v").and_then(Value::as_f64) == Some(1.0) && m.get("pages").is_some_and(Value::is_array) => Ok(m),
        _ => Err(BROKEN_FILE.into()),
    }
}

/// Writes the list; a symlinked one (a dotfile setup) where the link leads, so the rename replaces
/// that file and the link stays.
fn write_list(file: &Path, list: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(list.clone())).unwrap_or_default() + "\n";
    write_atomic(&link_target(file), &text, 0o600).map_err(|e| cant_write(file, &e))
}

/// The path a chain of symlinks ends at, read link by link so a link to a file not made yet still
/// leads somewhere; the path itself when it isn't a link. Gives up after 40 links (a loop).
pub fn link_target(file: &Path) -> PathBuf {
    let mut p = file.to_owned();
    for _ in 0..40 {
        let Ok(to) = fs::read_link(&p) else { return p };
        p = lexical(&p.parent().unwrap_or(Path::new("")).join(to));
    }
    p
}

/// Makes ~/.inyon when it isn't there, readable only by this user.
fn ensure_dir(file: &Path) -> Result<(), String> {
    let Some(dir) = file.parent() else { return Ok(()) };
    if dir.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| cant_write(file, &e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// One entry as the hub reads it: every field with its type and the slug a slug.
fn readable(v: &Value) -> bool {
    let Some(m) = v.as_object() else { return false };
    let s = |k: &str| m.get(k).is_some_and(Value::is_string);
    m.get("slug").and_then(Value::as_str).is_some_and(is_slug) && s("name") && s("path") && s("project") && s("by") && m.get("added").is_some_and(Value::is_number)
}

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ---------- the commands ----------

pub struct Added {
    pub page: Value,
    /// false when the folder was listed already, just as asked
    pub changed: bool,
    /// whether `.inyon-page.json` in the site's folder is up to date
    pub marker: bool,
}

/// Adds a site's folder to `file`, or updates its entry when the folder is there already, so
/// adding again is safe. `home` is the user's home folder; `by` who is adding it; `session` the
/// agent's conversation, which the latest add sets; `now` ms since 1970.
#[allow(clippy::too_many_arguments)]
pub fn add(file: &Path, home: &Path, dir: &Path, name: Option<&str>, slug: Option<&str>, by: &str, session: Option<&str>, now: u64) -> Result<Added, String> {
    let folder = absolute(dir)?;
    let path = text_of(&folder)?;
    let home_text = text_of(home)?;
    if let Some(p) = folder_problem(&path, &home_text) {
        return Err(p.into());
    }
    let meta = fs::metadata(&folder).map_err(|_| format!("{path} doesn't exist"))?;
    if !meta.is_dir() {
        return Err(format!("{path} isn't a folder"));
    }
    let real_path = real(&folder).unwrap_or_else(|| path.clone());
    let real_home = real(home).unwrap_or_else(|| home_text.clone());
    if let Some(p) = real_problem(&path, &real_path, &real_home) {
        return Err(p.into());
    }
    if !folder.join("index.html").is_file() {
        return Err(format!("{NO_INDEX}: {path}"));
    }
    if let Some(s) = slug {
        if !is_slug(s) {
            return Err(BAD_SLUG.into());
        }
    }
    let asked = name.and_then(clean_name);
    let marker = read_marker(&folder);
    ensure_dir(file)?;

    let (page, changed) = with_lock(file, || {
        let mut list = read_list(file)?;
        let pages = list.get_mut("pages").and_then(Value::as_array_mut).ok_or(BROKEN_FILE)?;
        let at = pages.iter().position(|p| {
            let p = field(p, "path");
            !p.is_empty() && (p == path || real(Path::new(p)).is_some_and(|r| r == real_path))
        });
        let taken = |s: &str| pages.iter().enumerate().any(|(i, p)| Some(i) != at && field(p, "slug") == s);
        let old = at.map(|i| pages[i].clone());
        let old_slug = old.as_ref().map(|o| field(o, "slug")).filter(|s| is_slug(s));
        let slug = match (slug, old_slug) {
            (Some(s), _) => {
                if taken(s) {
                    let other = pages.iter().find(|p| field(p, "slug") == s).map(|p| field(p, "path")).unwrap_or("");
                    return Err(format!("The slug \"{s}\" is taken by {other}. Pick another with --slug"));
                }
                s.to_string()
            }
            (None, Some(s)) => s.to_string(),
            (None, None) => {
                let base = marker.slug.clone().unwrap_or_else(|| slugify(crate::paths::basename(&path)));
                unique_slug(&base, taken)
            }
        };
        let old_name = old.as_ref().map(|o| field(o, "name")).and_then(clean_name);
        let name = asked.clone().or(old_name).or_else(|| marker.name.clone()).or_else(|| page_title(&folder)).unwrap_or_else(|| crate::paths::basename(&path).to_string());
        let mut entry = match &old {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        entry.insert("slug".into(), json!(slug));
        entry.insert("name".into(), json!(name));
        if !entry.get("path").is_some_and(Value::is_string) {
            entry.insert("path".into(), json!(path));
        }
        if !entry.get("project").and_then(Value::as_str).is_some_and(|p| !p.is_empty()) {
            entry.insert("project".into(), json!(project_of(&path)));
        }
        if !entry.get("by").and_then(Value::as_str).is_some_and(|b| !b.is_empty()) {
            entry.insert("by".into(), json!(by));
        }
        if !entry.get("added").is_some_and(Value::is_number) {
            entry.insert("added".into(), json!(now));
        }
        if let Some(s) = session.filter(|s| is_session(s)) {
            entry.insert("session".into(), json!(s));
        }
        let page = Value::Object(entry);
        let changed = old.as_ref() != Some(&page);
        if changed {
            match at {
                Some(i) => pages[i] = page.clone(),
                None => pages.push(page.clone()),
            }
        }
        // added on purpose: `pages find` may add it again after a later rm
        let was_removed = forget_removed(&mut list, &path, &real_path);
        if changed || was_removed {
            write_list(file, &list)?;
        }
        Ok((page, changed))
    })?;
    let marker = write_marker(&folder, &marker, field(&page, "slug"), field(&page, "name"), now);
    Ok(Added { page, changed, marker })
}

/// Takes every entry with this slug out of `file`, and returns them.
pub fn remove(file: &Path, slug: &str) -> Result<Vec<Value>, String> {
    let missing = || format!("There's no page called \"{slug}\" on this machine. See them with: inyon pages ls");
    if !file.exists() {
        return Err(missing());
    }
    with_lock(file, || {
        let mut list = read_list(file)?;
        let pages = list.get_mut("pages").and_then(Value::as_array_mut).ok_or(BROKEN_FILE)?;
        let (gone, keep): (Vec<Value>, Vec<Value>) = pages.drain(..).partition(|p| field(p, "slug") == slug);
        *pages = keep;
        if gone.is_empty() {
            return Err(missing());
        }
        for p in &gone {
            let path = field(p, "path");
            remember_removed(&mut list, &real(Path::new(path)).unwrap_or_else(|| path.to_string()));
        }
        write_list(file, &list)?;
        Ok(gone)
    })
}

// ---------- finding pages ----------

/// The most folders kept in pages.json's `removed`, the newest last.
pub const REMOVED_MAX: usize = 200;
/// How deep `pages find` looks under a folder, and how many folders it looks at in all.
const FIND_DEPTH: usize = 4;
const FIND_FOLDERS: usize = 4000;
/// Folders `pages find` doesn't look inside: dependencies and build output.
// test data holds sites made for tests (Inyon's own fixtures/pages/site was found this way)
const FIND_SKIP: [&str; 11] = ["node_modules", "target", "dist", "build", "vendor", "__pycache__", "fixtures", "__fixtures__", "testdata", "test", "tests"];

/// The folders taken off the list, which `pages find` leaves alone (the hub keeps the same list).
fn removed_of(list: &Map<String, Value>) -> Vec<String> {
    list.get("removed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect()
}

fn remember_removed(list: &mut Map<String, Value>, folder: &str) {
    let mut removed = removed_of(list);
    removed.retain(|p| p != folder);
    removed.push(folder.to_string());
    let extra = removed.len().saturating_sub(REMOVED_MAX);
    removed.drain(..extra);
    list.insert("removed".into(), json!(removed));
}

/// Takes a folder out of `removed`; whether it was there.
fn forget_removed(list: &mut Map<String, Value>, path: &str, real_path: &str) -> bool {
    let removed = removed_of(list);
    let keep: Vec<String> = removed.iter().filter(|p| *p != path && *p != real_path).cloned().collect();
    if keep.len() == removed.len() {
        return false;
    }
    list.insert("removed".into(), json!(keep));
    true
}

/// The Blueprint sites under `dir`, up to `depth` folders down: a folder with a `.inyon-page.json`
/// and an `index.html`, both plain files. Symlinks aren't followed, dot folders and FIND_SKIP aren't
/// looked in, and nothing inside a site is. `budget` counts down the folders looked at.
fn sites_under(dir: &Path, depth: usize, budget: &mut usize, out: &mut Vec<PathBuf>) {
    let mut todo = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, level)) = todo.pop() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let plain = |name: &str| fs::symlink_metadata(d.join(name)).is_ok_and(|m| m.is_file());
        if plain(".inyon-page.json") && plain("index.html") {
            out.push(d);
            continue;
        }
        if level == depth {
            continue;
        }
        let Ok(entries) = fs::read_dir(&d) else { continue };
        let mut subs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && !FIND_SKIP.contains(&n)))
            .map(|e| e.path())
            .collect();
        subs.sort();
        todo.extend(subs.into_iter().rev().map(|p| (p, level + 1)));
    }
}

/// Adds the Blueprint sites found where agents work: under each of `dirs`, and in the
/// `private/plans` of its project. A site already listed, or taken off the list before, is left
/// alone, and so is a folder that can't be a page (`/`, the home folder). The entries added.
pub fn find(file: &Path, home: &Path, dirs: &[PathBuf], by: &str, now: u64) -> Result<Vec<Value>, String> {
    let home_text = text_of(home)?;
    let list = if file.exists() { read_list(file)? } else { Map::new() };
    let mut known: std::collections::HashSet<String> = removed_of(&list).into_iter().collect();
    for p in list.get("pages").and_then(Value::as_array).into_iter().flatten() {
        let path = field(p, "path");
        known.insert(path.to_string());
        known.extend(real(Path::new(path)));
    }
    // each folder gets an even share of what's left, so a big one can't use it all up; what one
    // doesn't use goes to the ones after it
    let mut left = FIND_FOLDERS;
    let mut sites = vec![];
    let roots: Vec<&PathBuf> = dirs.iter().filter(|d| d.is_absolute()).collect();
    for (i, dir) in roots.iter().enumerate() {
        let share = left / (roots.len() - i);
        let mut budget = share;
        let Ok(dir) = absolute(dir) else { continue };
        let Ok(text) = text_of(&dir) else { continue };
        if folder_problem(&text, &home_text).is_some() || !dir.is_dir() {
            continue;
        }
        sites_under(&dir, FIND_DEPTH, &mut budget, &mut sites);
        let plans = Path::new(&project_of(&text)).join("private").join("plans");
        if !plans.starts_with(&dir) {
            sites_under(&plans, 1, &mut budget, &mut sites);
        }
        left -= share - budget;
    }
    let mut added = vec![];
    for site in sites {
        let Ok(path) = text_of(&site) else { continue };
        if known.contains(&path) || real(&site).is_some_and(|r| known.contains(&r)) {
            continue;
        }
        known.insert(path);
        // one that can't be added (a broken list, a site out of a private folder) is skipped
        if let Ok(a) = add(file, home, &site, None, None, by, None, now) {
            added.push(a.page);
        }
    }
    Ok(added)
}

pub fn find_command(dirs: &[PathBuf], by: &str) -> Outcome {
    let by = if !by.is_empty() && by.len() <= 32 && by.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') { by } else { "found" };
    let found = find(&pages_file(), &paths::home(), dirs, by, crate::state::now_ms() as u64)?;
    let message = match found.len() {
        0 => "No new pages.".to_string(),
        n => format!("Added {n} page{}: {}", if n == 1 { "" } else { "s" }, found.iter().map(|p| field(p, "name")).collect::<Vec<_>>().join(", ")),
    };
    Ok(json!({ "found": found, "message": message }))
}

/// What keeps a listed page from opening in the app, when something does.
fn trouble(page: &Value, home: &str, real_home: &str) -> Option<String> {
    let path = field(page, "path");
    if let Some(p) = folder_problem(path, home) {
        return Some(p.into());
    }
    if !Path::new(path).is_dir() {
        return Some("the folder is gone".into());
    }
    if let Some(p) = real(Path::new(path)).and_then(|r| real_problem(path, &r, real_home)) {
        return Some(p.into());
    }
    if !Path::new(path).join("index.html").is_file() {
        return Some("it has no index.html".into());
    }
    None
}

/// The page's path in the app. Only the hub's own machine knows it: the hub gives a machine joined
/// to it an id of its own, which that machine never learns.
fn app_link(slug: &str) -> String {
    format!("/pages/{LOCAL_HOST}/{slug}")
}

/// Where a machine without a hub of its own has its pages shown: the hub it's joined to, or (paired
/// with the phone only, say) nowhere yet.
fn not_hub_note(joined: bool) -> &'static str {
    if joined {
        "This machine doesn't run a hub. The hub it's joined to shows its pages (hub 0.22 or later)."
    } else {
        "This machine doesn't run a hub and isn't joined to one, so the app can't open this page. Add it to a hub (Machines > Add a machine) to show its pages there."
    }
}

/// What `add` says. Without a hub here the page opens through the hub this machine is joined to,
/// which gives it an address of its own, so there's no app path to hand over.
fn added_lines(name: &str, machine: &str, project: &str, slug: &str, changed: bool, hub: bool, joined: bool) -> Vec<String> {
    let again = if changed { "" } else { " (already listed)" };
    if hub {
        vec![format!("\"{name}\" is in Inyon: Pages > {machine} > {project}{again}"), format!("Open it in the app at {}", app_link(slug))]
    } else if joined {
        vec![format!("\"{name}\" is in Inyon: Pages > {machine} > {project}{again}"), not_hub_note(true).into()]
    } else {
        vec![format!("\"{name}\" is listed on {machine}, under {project}{again}."), not_hub_note(false).into()]
    }
}

pub fn add_command(dir: &Path, name: Option<&str>, slug: Option<&str>) -> Outcome {
    let by = agent(|k| std::env::var(k).ok());
    let session = agent_session(|k| std::env::var(k).ok());
    let now = crate::state::now_ms() as u64;
    let a = add(&pages_file(), &paths::home(), dir, name, slug, by, session.as_deref(), now)?;
    let machine = inyon_engine::hostname();
    let slug = field(&a.page, "slug").to_string();
    let project = paths::basename(field(&a.page, "project").trim_end_matches(['/', '\\'])).to_string();
    let hub = crate::engine::hub_here();
    let joined = !hub && crate::commands::joined();
    let lines = added_lines(field(&a.page, "name"), &machine, &project, &slug, a.changed, hub, joined);
    let mut out = json!({
        "page": a.page,
        "host": LOCAL_HOST,
        "machine": machine,
        "changed": a.changed,
        "marker": a.marker,
        "hub": hub,
        "joined": joined,
        "message": lines.join("\n"),
    });
    if hub {
        out["app"] = json!(app_link(&slug));
    }
    Ok(out)
}

pub fn remove_command(slug: &str) -> Outcome {
    let gone = remove(&pages_file(), slug)?;
    let first = &gone[0];
    Ok(json!({
        "removed": gone,
        "message": format!("Removed \"{}\" from Inyon. Its folder stays where it is: {}", field(first, "name"), short(field(first, "path"))),
    }))
}

pub fn list_command() -> Outcome {
    let file = pages_file();
    let list = read_list(&file)?;
    let home = paths::home();
    let home_text = home.to_string_lossy().into_owned();
    let real_home = real(&home).unwrap_or_else(|| home_text.clone());
    let all = list.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let hub = crate::engine::hub_here();
    let mut seen = std::collections::HashSet::new();
    let mut pages = Vec::new();
    let mut skipped = 0;
    for p in &all {
        if !readable(p) || !seen.insert(field(p, "slug").to_string()) {
            skipped += 1;
            continue;
        }
        let mut row = p.clone();
        if hub {
            row["app"] = json!(app_link(field(p, "slug")));
        }
        if let Some(t) = trouble(p, &home_text, &real_home) {
            row["problem"] = json!(t);
        } else {
            if let Some(progress) = progress_of(Path::new(field(p, "path"))) {
                row["progress"] = progress;
            }
            let notes = notes_of(Path::new(field(p, "path")));
            if !notes.is_empty() {
                row["notes"] = json!(notes);
            }
        }
        pages.push(row);
    }
    let machine = inyon_engine::hostname();
    let mut lines = Vec::new();
    if pages.is_empty() {
        lines.push("No pages on this machine yet. Add a site's folder with: inyon pages add <folder>".to_string());
    } else {
        lines.push(format!("Pages on {machine}:"));
        let w_slug = pages.iter().map(|p| field(p, "slug").chars().count()).max().unwrap_or(0);
        let w_name = pages.iter().map(|p| field(p, "name").chars().count()).max().unwrap_or(0).min(40);
        for p in &pages {
            let name: String = field(p, "name").chars().take(40).collect();
            let mut line = format!("  {:<w_slug$}  {:<w_name$}  {}", field(p, "slug"), name, short(field(p, "path")));
            if let Some(t) = p.get("problem").and_then(Value::as_str) {
                line.push_str(&format!("  ! {t}"));
            }
            lines.push(line);
        }
        lines.push(if hub { format!("Open one in the app under Pages, or at {}", app_link("<slug>")) } else { format!("Open one in the app under Pages > {machine}") });
    }
    if skipped > 0 {
        lines.push(format!(
            "{skipped} {} in {} can't be read and {} left out.",
            if skipped == 1 { "entry" } else { "entries" },
            short(&file.to_string_lossy()),
            if skipped == 1 { "is" } else { "are" }
        ));
    }
    if !hub && !pages.is_empty() {
        lines.push(not_hub_note(crate::commands::joined()).into());
    }
    // `serves`: this inyon answers `pages get`, so a hub can show these pages (from 0.22)
    Ok(json!({ "pages": pages, "host": LOCAL_HOST, "machine": machine, "hub": hub, "serves": true, "skipped": skipped, "message": lines.join("\n") }))
}

// ---------- serving a page to a hub ----------

/// The most notes listed for one site.
const NOTES_MAX: usize = 200;

/// The Markdown notes in a site's `research/` folder, by name, sorted, as the hubs list them for
/// their own pages (inyon-hub's pages::notes_of): plain files whose name has no dot first and ends
/// in .md, and only when `research` is a real folder, not a symlink.
pub fn notes_of(site: &Path) -> Vec<String> {
    let dir = site.join("research");
    if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return vec![];
    }
    let Ok(entries) = fs::read_dir(&dir) else { return vec![] };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n.len() <= 120 && n.len() > 3 && n.to_ascii_lowercase().ends_with(".md"))
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out.truncate(NOTES_MAX);
    out
}

/// The biggest progress.json read, as the hubs read it.
const PROGRESS_MAX: u64 = 1024 * 1024;
/// The biggest file `pages get` sends; the hub asks for less with --max.
pub const GET_MAX: u64 = 8 * 1024 * 1024;

/// A site's progress.json counted as the hubs count it (pages_core's progress_summary): tasks done
/// of all tasks across its releases, and its `now` and `updated` lines. None without a readable one.
pub fn progress_of(site: &Path) -> Option<Value> {
    let bytes = read_plain(&site.join("progress.json"), PROGRESS_MAX).ok()?;
    let data: Value = serde_json::from_slice(&bytes).ok()?;
    let releases = data.as_object()?.get("releases")?.as_array()?;
    let (mut done, mut total) = (0u32, 0u32);
    for t in releases.iter().filter_map(|r| r.get("tasks")?.as_array()).flatten().filter(|t| t.is_object()) {
        total += 1;
        done += u32::from(t.get("status").and_then(Value::as_str) == Some("done"));
    }
    let mut out = json!({ "done": done, "total": total });
    for k in ["now", "updated"] {
        if let Some(v) = data.get(k).and_then(Value::as_str) {
            out[k] = json!(v);
        }
    }
    Some(out)
}

/// A path as a hub on any system reads it: Windows' `\\?\` gone and its backslashes as slashes.
fn slashed(p: &str) -> String {
    strip_verbatim(p).replace('\\', "/")
}

/// Node's `mtimeMs`, as the hubs make it.
fn mtime_ms(m: &fs::Metadata) -> f64 {
    match m.modified().ok().map(|t| t.duration_since(std::time::UNIX_EPOCH)) {
        Some(Ok(d)) => d.as_secs() as f64 * 1e3 + f64::from(d.subsec_nanos()) / 1e6,
        Some(Err(e)) => -(e.duration().as_secs_f64() * 1e3),
        None => 0.0,
    }
}

/// pages_core's `etag_of`: `"<size in hex>-<whole ms in hex>"`.
fn etag_of(size: u64, mtime: f64) -> String {
    let ms = mtime.floor() as i128;
    if ms < 0 {
        format!("\"{size:x}--{:x}\"", -ms)
    } else {
        format!("\"{size:x}-{ms:x}\"")
    }
}

/// pages_core's `etag_matches`: `*`, or any listed tag equal to it (a `W/` prefix ignored).
fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let inm = if_none_match.trim();
    !inm.is_empty() && (inm == "*" || inm.split(',').any(|t| t.trim().strip_prefix("W/").unwrap_or(t.trim()) == etag))
}

/// `real` is `root` or inside it, with no dot-named folder or file on the way (both slashed).
fn inside(root: &str, real: &str) -> bool {
    let root = root.trim_end_matches('/');
    let rest = if real == root {
        ""
    } else if let Some(r) = real.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        r
    } else {
        return false;
    };
    rest.split('/').filter(|c| !c.is_empty()).all(|c| !c.starts_with('.'))
}

/// One path as `pages get` reports it: where it really leads and what is there.
fn looked_up(p: &Path) -> Value {
    let Some(real) = real(p) else { return Value::Null };
    let Ok(m) = fs::metadata(&real) else { return Value::Null };
    json!({ "real": slashed(&real), "file": m.is_file(), "dir": m.is_dir(), "size": m.len(), "mtime": mtime_ms(&m) })
}

/// What a hub needs to answer one request for a file of a page on this machine, in one go: where
/// the site's folder really is, then the asked path and the index.html inside it (each where it
/// really leads, whether it's a file or a folder, its size and mtime), and the file the request
/// ends at, as base64. The hub decides the answer with its own rules; this side only lists what is
/// there, and sends a file only when it's inside the site, with no dot name and no symlink leading
/// out on the way, of at most `max` bytes, and not the one the browser has (`etag`).
///
/// Only a page on this machine's list can be read, and only through its slug.
pub fn get_command(slug: &str, segments: &[String], slash: bool, max: u64, etag: Option<&str>) -> Outcome {
    let (mut out, send) = get_answer(slug, segments, slash, max, etag)?;
    if let Some(path) = send {
        if let Ok(bytes) = read_plain(&path, max) {
            use base64::Engine;
            out["data"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
        }
    }
    Ok(out)
}

/// The biggest file `pages get --lines` sends.
pub const STREAM_MAX: u64 = 64 * 1024 * 1024;
/// A streamed file's bytes per line: 48 KiB, 64 KiB as base64.
const LINE_BYTES: usize = 48 * 1024;

/// `pages get --lines`: the same answer on one line, without `data`, then the file as base64 lines,
/// for a hub that reads it as a stream. A file of any size up to STREAM_MAX then fits, since no
/// one message has to hold it. The answer says how many bytes follow (`sends`); a file that
/// can't be read after all ends the lines early, and the hub then has fewer than it was told.
pub fn get_lines_command(slug: &str, segments: &[String], slash: bool, max: u64, etag: Option<&str>) -> Outcome {
    use base64::Engine;
    use std::io::{Read, Write};
    let (mut out, send) = get_answer(slug, segments, slash, max, etag)?;
    let opened = send.and_then(|p| open_plain(&p, false).ok()).filter(|(_, len)| *len <= max);
    if let Some((_, len)) = &opened {
        out["sends"] = json!(len);
    }
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", serde_json::to_string(&crate::commands::with_v(out)).unwrap_or_default());
    if let Some((file, len)) = opened {
        let mut file = file.take(len);
        let mut buf = vec![0u8; LINE_BYTES];
        loop {
            // a full line each time, so only the last one is short
            let mut n = 0;
            while n < buf.len() {
                match file.read(&mut buf[n..]) {
                    Ok(0) => break,
                    Ok(k) => n += k,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            if n == 0 || writeln!(stdout, "{}", base64::engine::general_purpose::STANDARD.encode(&buf[..n])).is_err() {
                break;
            }
            if n < buf.len() {
                break;
            }
        }
    }
    let _ = stdout.flush();
    Ok(Value::Null)
}

/// get_command's answer without the file, and the file to send when there is one.
fn get_answer(slug: &str, segments: &[String], slash: bool, max: u64, etag: Option<&str>) -> Result<(Value, Option<PathBuf>), String> {
    let not_found = || Ok((json!({ "ok": true, "root": Value::Null, "items": [] }), None));
    let list = read_list(&pages_file())?;
    let home = paths::home();
    let home_text = home.to_string_lossy().into_owned();
    let real_home = real(&home).unwrap_or_else(|| home_text.clone());
    let pages = list.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(page) = pages.iter().find(|p| readable(p) && field(p, "slug") == slug) else { return not_found() };
    if trouble(page, &home_text, &real_home).is_some() {
        return not_found();
    }
    // as view_segments takes them, and no colon: on Windows `name:stream` reads another stream
    if segments.iter().any(|s| s.is_empty() || s.starts_with('.') || s.contains(['/', '\\', '\0', ':'])) {
        return not_found();
    }
    let folder = PathBuf::from(field(page, "path"));
    let Some(root) = real(&folder).map(|r| slashed(&r)) else { return not_found() };
    let asked = segments.iter().fold(folder, |p, s| p.join(s));
    let first = looked_up(&asked);
    let index = match first.get("real").and_then(Value::as_str) {
        Some(r) if first["dir"] == true => looked_up(&Path::new(r).join("index.html")),
        _ => Value::Null,
    };
    // the file the hub serves: the index.html of a folder asked for with its slash, or the file asked for
    let ends = match (first["dir"] == true, slash) {
        (true, true) => &index,
        (false, false) => &first,
        _ => &Value::Null,
    };
    let mut out = json!({ "ok": true, "root": root, "items": [first.clone(), index.clone()] });
    let mut send = None;
    if let (Some(real_path), true) = (ends.get("real").and_then(Value::as_str), ends["file"] == true) {
        let size = ends["size"].as_u64().unwrap_or(0);
        let fresh = etag.is_some_and(|e| etag_matches(e, &etag_of(size, ends["mtime"].as_f64().unwrap_or(0.0))));
        let name = real_path.rsplit('/').next().unwrap_or("");
        if inside(&root, real_path) && (name == "progress.json" || !fresh) {
            if size > max {
                out["big"] = json!(true);
            } else {
                send = Some(PathBuf::from(real_path));
            }
        }
    }
    Ok((out, send))
}

// ---------- telling a hub when pages change ----------

/// How often `pages watch` looks, in ms, right after something changed.
pub const WATCH_EVERY_MS: u64 = 2000;
/// While nothing changes it looks less and less often, down to once in this many times `every_ms`:
/// walking every site's files every 2 s kept a laptop busy for pages nobody was editing.
const WATCH_SLOWEST: u64 = 5;

/// How long `pages watch` waits before its next look.
pub fn watch_wait(prev: u64, changed: bool, every_ms: u64) -> u64 {
    let every = every_ms.max(200);
    if changed {
        every
    } else {
        (prev.max(every) * 3 / 2).min(every * WATCH_SLOWEST)
    }
}
/// How long `pages watch` goes without printing: an empty line then, so it finds out when nobody
/// reads it any more, even if what stopped the hub's end never reached it.
pub const WATCH_ALIVE_MS: u64 = 30_000;
/// The most files a site's stamp covers; a bigger site has none, and the hub asks for each file.
const STAMP_FILES: usize = 4000;
/// How deep a site's stamp looks.
const STAMP_DEPTH: usize = 8;

/// A short tag for everything a hub could serve from a site: every file and folder without a dot
/// name, with its size and mtime. Another tag means something changed. None when the site has
/// more than STAMP_FILES of them, or more folders deep than STAMP_DEPTH, or can't be read. Symlinks
/// count as themselves, not followed, so a link out of the site can't make it walk the disk; the
/// hub doesn't serve what one leads out to anyway.
pub fn site_stamp(site: &Path) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut seen = 0usize;
    let mut todo = vec![(site.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = todo.pop() {
        let mut names: Vec<(std::ffi::OsString, fs::Metadata)> = vec![];
        for e in fs::read_dir(&dir).ok()? {
            let e = e.ok()?;
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            names.push((name, fs::symlink_metadata(e.path()).ok()?));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, m) in names {
            seen += 1;
            if seen > STAMP_FILES {
                return None;
            }
            let path = dir.join(&name);
            path.strip_prefix(site).unwrap_or(&path).hash(&mut h);
            (m.is_dir(), m.is_symlink(), m.len()).hash(&mut h);
            mtime_ms(&m).to_bits().hash(&mut h);
            if m.is_dir() && !m.is_symlink() {
                if depth + 1 > STAMP_DEPTH {
                    return None;
                }
                todo.push((path, depth + 1));
            }
        }
    }
    Some(format!("{:016x}", h.finish()))
}

/// `pages watch --json`: the list as `pages --json` gives it, each page with its site's `stamp`,
/// printed once and then again whenever it changes (a page added or removed, a board moving on, a
/// file written), so the hub this machine is joined to doesn't have to ask. It runs until the hub
/// stops it, or until nobody reads what it prints (an empty line after `alive_ms` without one checks).
pub fn watch_command(every_ms: u64, alive_ms: u64) -> Outcome {
    use std::io::Write;
    let mut last = String::new();
    let mut printed = std::time::Instant::now();
    let mut wait = every_ms;
    loop {
        let mut v = list_command()?;
        if let Some(pages) = v.get_mut("pages").and_then(Value::as_array_mut) {
            for p in pages.iter_mut().filter(|p| p.get("problem").is_none()) {
                p["stamp"] = site_stamp(Path::new(field(p, "path"))).map_or(Value::Null, Value::from);
            }
        }
        if let Some(m) = v.as_object_mut() {
            m.remove("message");
        }
        let text = serde_json::to_string(&crate::commands::with_v(v)).unwrap_or_default();
        let line = if text != last { Some(text.as_str()) } else { (printed.elapsed().as_millis() as u64 >= alive_ms).then_some("") };
        if let Some(line) = line {
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                return Ok(Value::Null);
            }
            printed = std::time::Instant::now();
        }
        wait = watch_wait(wait, text != last, every_ms);
        last = text;
        std::thread::sleep(std::time::Duration::from_millis(wait));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_watch_looks_less_often_while_nothing_changes() {
        let mut wait = WATCH_EVERY_MS;
        let mut waits = vec![];
        for _ in 0..6 {
            wait = watch_wait(wait, false, WATCH_EVERY_MS);
            waits.push(wait);
        }
        assert_eq!(waits, [3000, 4500, 6750, 10_000, 10_000, 10_000]);
        // a board moved on: back to every 2 s
        assert_eq!(watch_wait(10_000, true, WATCH_EVERY_MS), 2000);
        assert_eq!(watch_wait(0, false, 50), 300);
    }

    #[test]
    fn titles() {
        assert_eq!(title_of("<html><head><title>Plan</title>").as_deref(), Some("Plan"));
        assert_eq!(title_of("<TITLE lang=en>\n  Inyon &amp; co &#8212; plan\n</TITLE>").as_deref(), Some("Inyon & co \u{2014} plan"));
        assert_eq!(title_of("<titlex>no</titlex><title>yes</title>").as_deref(), Some("yes"));
        assert_eq!(title_of("<title>   </title>"), None);
        assert_eq!(title_of("<title>never closed"), None);
        assert_eq!(title_of("no title"), None);
        assert_eq!(title_of("<title>a &bogus; b & c</title>").as_deref(), Some("a &bogus; b & c"));
    }

    #[test]
    fn an_ampersand_before_text_of_more_than_one_byte_a_character_does_not_panic() {
        // from the '&', byte 12 falls inside 'の'
        assert_eq!(title_of("<title>設計 & 日本語の計画</title>").as_deref(), Some("設計 & 日本語の計画"));
        assert_eq!(title_of("<title>Café &amp; crème brûlée &éé</title>").as_deref(), Some("Café & crème brûlée &éé"));
        assert_eq!(title_of("<title>&日本語の計画&#x8a08;</title>").as_deref(), Some("&日本語の計画\u{8a08}"));
    }

    #[test]
    fn a_mapped_network_drive_is_judged_with_its_share_as_the_drive() {
        // Z: is \\server\share: canonicalize leads there, and the rules don't take a UNC path typed by hand
        assert_eq!(real_problem(r"Z:\work\site", r"\\server\share\work\site", r"C:\Users\a"), None);
        assert_eq!(site_path_problem(r"Z:\work\site", r"C:\Users\a"), None);
        // the share itself is as wide as a drive
        assert_eq!(real_problem(r"Z:\", r"\\server\share", r"C:\Users\a"), Some(TOO_WIDE));
        assert_eq!(real_problem(r"Z:\", r"\\server\share\", r"C:\Users\a"), Some(TOO_WIDE));
        // a home folder on the same share (a roaming profile) keeps its private folders and itself
        assert_eq!(real_problem(r"Z:\a\.ssh\x", r"\\server\share\a\.ssh\x", r"\\SERVER\share\a"), Some(PRIVATE_FOLDER));
        assert_eq!(real_problem(r"Z:\a", r"\\server\share\a", r"\\server\share\a"), Some(TOO_WIDE));
        assert_eq!(real_problem(r"Z:\a\.ssh", r"\\server\share\a\.ssh", r"\\server\other\a"), None);
        // a drive letter that leads into the home folder's .ssh is still refused
        assert_eq!(real_problem(r"D:\x", r"C:\Users\a\.ssh\x", r"C:\Users\a"), Some(PRIVATE_FOLDER));
        // a UNC path typed as such stays refused, and so does a Unix path that leads somewhere odd
        assert_eq!(real_problem(r"\\server\share\site", r"\\server\share\site", r"C:\Users\a"), Some(NOT_ABSOLUTE));
        assert_eq!(real_problem("/home/a/site", "/home/a/.ssh", "/home/a"), Some(PRIVATE_FOLDER));
        assert_eq!(real_problem(r"Z:\x", r"\\server", r"C:\Users\a"), Some(NOT_ABSOLUTE));
    }

    /// A FIFO at `p`, and a writer that opens it after two seconds and closes it at once: a read
    /// that opened it as a plain file waits that long instead of hanging the test for good. The
    /// writer is stopped when this is dropped.
    #[cfg(unix)]
    struct Fifo(std::process::Child);

    #[cfg(unix)]
    impl Fifo {
        fn at(p: &Path) -> Fifo {
            Fifo::after(p, 2)
        }
        /// The writer opens it after `secs` seconds.
        fn after(p: &Path, secs: u32) -> Fifo {
            let _ = fs::remove_file(p);
            assert!(std::process::Command::new("mkfifo").arg(p).status().unwrap().success());
            Fifo(std::process::Command::new("sh").args(["-c", &format!("sleep {secs}; : > \"$1\""), "sh"]).arg(p).spawn().unwrap())
        }
    }

    #[cfg(unix)]
    impl Drop for Fifo {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_site_or_as_the_list_is_skipped_at_once_not_read_until_something_writes_to_it() {
        let dir = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("inyon-cli-fifo-{}-{}", std::process::id(), random_tag()));
        let home = dir.join("home");
        let site = home.join("work/piped");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("index.html"), "<title>Piped plan</title>").unwrap();
        let file = home.join(".inyon/pages.json");
        let quick = |at: Instant| assert!(at.elapsed() < Duration::from_secs(1), "took {:?}", at.elapsed());
        {
            // the marker: the slug comes from the folder, the name from the title
            let _w = Fifo::at(&site.join(".inyon-page.json"));
            let at = Instant::now();
            let added = add(&file, &home, &site, None, None, "cli", None, 1).unwrap();
            quick(at);
            assert_eq!((field(&added.page, "slug"), field(&added.page, "name")), ("piped", "Piped plan"));
        }
        {
            // index.html swapped for a FIFO after the check: no title, at once
            let _w = Fifo::at(&site.join("index.html"));
            let at = Instant::now();
            assert_eq!(page_title(&site), None);
            quick(at);
        }
        {
            // the list itself: a list it can't read, at once
            let _w = Fifo::at(&file);
            fs::remove_file(site.join("index.html")).unwrap();
            fs::write(site.join("index.html"), "<title>Piped plan</title>").unwrap();
            let at = Instant::now();
            assert_eq!(add(&file, &home, &site, None, None, "cli", None, 1).err().as_deref(), Some(BROKEN_FILE));
            quick(at);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_put_in_place_of_the_lock_is_not_waited_on_when_it_is_released() {
        let dir = std::env::temp_dir().join(format!("inyon-cli-lock-fifo-{}-{}", std::process::id(), random_tag()));
        fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("pages.json.lock");
        let at = Instant::now();
        let w = with_lock(&dir.join("pages.json"), || Ok(Fifo::at(&lock))).unwrap();
        assert!(at.elapsed() < Duration::from_secs(1), "took {:?}", at.elapsed());
        // it isn't this writer's lock, so it stays
        assert!(std::os::unix::fs::FileTypeExt::is_fifo(&fs::symlink_metadata(&lock).unwrap().file_type()));
        drop(w);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn git_that_never_answers_a_named_pipe_as_git_head_is_stopped_after_three_seconds() {
        let dir = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("inyon-cli-git-stuck-{}-{}", std::process::id(), random_tag()));
        fs::create_dir_all(dir.join(".git/objects")).unwrap();
        fs::create_dir_all(dir.join(".git/refs")).unwrap();
        // a writer late enough that waiting on git fails the test instead of hanging it
        let w = Fifo::after(&dir.join(".git/HEAD"), 8);
        let at = Instant::now();
        let folder = dir.to_str().unwrap();
        assert_eq!(project_of(folder), folder);
        let took = at.elapsed();
        assert!(took > GIT_TIMEOUT - Duration::from_millis(200) && took < GIT_TIMEOUT + Duration::from_millis(1500), "took {took:?}");
        drop(w);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_its_tag_could_not_be_written_to_is_taken_away_again_not_left_for_ten_seconds() {
        let dir = std::env::temp_dir().join(format!("inyon-cli-lock-full-{}-{}", std::process::id(), random_tag()));
        fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("pages.json.lock");
        let full = |_: &mut fs::File, _: &[u8]| Err(std::io::Error::new(ErrorKind::StorageFull, "no space left on device"));
        assert_eq!(make_lock(&lock, "1 tag\n", full).unwrap_err().kind(), ErrorKind::StorageFull);
        assert!(!lock.exists());
        // the next writer gets it at once
        assert_eq!(with_lock(&dir.join("pages.json"), || Ok("in")), Ok("in"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_lock_two_writers_judged_leaves_the_fresh_one_made_in_between_in_its_place() {
        let dir = std::env::temp_dir().join(format!("inyon-cli-lock-{}-{}", std::process::id(), random_tag()));
        fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("pages.json.lock");
        fs::write(&lock, "1 stale\n").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(11);
        OpenOptions::new().write(true).open(&lock).unwrap().set_modified(old).unwrap();
        let judged = fs::metadata(&lock).unwrap();
        fs::remove_file(&lock).unwrap();
        fs::write(&lock, "2 fresh\n").unwrap();
        remove_judged(&lock, &judged).unwrap();
        assert_eq!(fs::read_to_string(&lock).unwrap(), "2 fresh\n");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_lock_that_cannot_be_removed_says_so_at_once_instead_of_waiting() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores the folder's mode
        }
        let dir = std::env::temp_dir().join(format!("inyon-cli-lock-ro-{}-{}", std::process::id(), random_tag()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pages.json");
        let lock = dir.join("pages.json.lock");
        fs::write(&lock, "1\n").unwrap();
        OpenOptions::new().write(true).open(&lock).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(11)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let started = Instant::now();
        let r = with_lock(&file, || Ok(()));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(&dir);
        let err = r.unwrap_err();
        assert!(err.contains("If an agent's sandbox stopped it"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_worktree_belongs_to_the_main_checkout_with_any_git() {
        let base = std::env::temp_dir().join(format!("inyon-cli-git-{}-{}", std::process::id(), random_tag()));
        let repo = base.join("repo");
        fs::create_dir_all(repo.join("sub")).unwrap();
        let repo = fs::canonicalize(&repo).unwrap();
        let s = |p: &Path| p.to_str().unwrap().to_string();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(&repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "first"]);
        let tree = repo.join(".claude/worktrees/x");
        git(&["worktree", "add", "-q", "--detach", &s(&tree)]);
        fs::create_dir_all(tree.join("deep")).unwrap();
        let outside = base.join("plain");
        fs::create_dir_all(&outside).unwrap();
        // fixtures/old-git/git is git before 2.31: it prints --path-format=absolute back and exits 0
        let old = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/old-git/");
        for g in ["git".to_string(), format!("{old}git")] {
            assert_eq!(project_with(&s(&repo), &g), s(&repo), "{g}");
            assert_eq!(project_with(&s(&repo.join("sub")), &g), s(&repo), "{g}");
            assert_eq!(project_with(&s(&tree), &g), s(&repo), "{g}");
            assert_eq!(project_with(&s(&tree.join("deep")), &g), s(&repo), "{g}");
            if std::process::Command::new("git").arg("-C").arg(&outside).args(["rev-parse", "--show-toplevel"]).output().is_ok_and(|o| !o.status.success()) {
                assert_eq!(project_with(&s(&outside), &g), s(&outside), "{g}");
            }
        }
        // before 2.5 there's no --git-common-dir: the top level then
        let g24 = format!("{old}git-2.4");
        assert_eq!(project_with(&s(&repo.join("sub")), &g24), s(&repo));
        assert_eq!(project_with(&s(&tree.join("deep")), &g24), s(&tree));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn verbatim_paths_lose_their_prefix() {
        assert_eq!(strip_verbatim(r"\\?\C:\Users\a\site"), r"C:\Users\a\site");
        assert_eq!(strip_verbatim(r"\\?\UNC\server\share\site"), r"\\server\share\site");
        assert_eq!(strip_verbatim("/home/a/site"), "/home/a/site");
    }

    #[cfg(unix)]
    #[test]
    fn dots_are_worked_out_from_the_text() {
        assert_eq!(lexical(Path::new("/a/b/../c/./d/")), PathBuf::from("/a/c/d"));
        assert_eq!(lexical(Path::new("/../..")), PathBuf::from("/"));
    }

    #[test]
    fn agents_from_their_variables() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(agent(env(&[("CLAUDECODE", "1")])), "claude");
        assert_eq!(agent(env(&[("CODEX_THREAD_ID", "x")])), "codex");
        assert_eq!(agent(env(&[("GEMINI_CLI", "1")])), "gemini");
        assert_eq!(agent(env(&[("OPENCODE", "1")])), "opencode");
        assert_eq!(agent(env(&[("CLAUDECODE", "")])), "cli");
        assert_eq!(agent(env(&[])), "cli");
    }

    #[test]
    fn windows_folders_are_judged_too() {
        assert_eq!(site_path_problem(r"C:\Users\a\site", r"C:\Users\a"), None);
        assert_eq!(site_path_problem(r"C:\Users\a", r"C:\Users\a"), Some(TOO_WIDE));
        assert_eq!(site_path_problem(r"C:\Users\a\.inyon\x", r"C:\Users\a"), Some(PRIVATE_FOLDER));
        assert_eq!(site_path_problem("C:/Users/a/.ssh", r"C:\Users\a"), Some(PRIVATE_FOLDER));
        assert_eq!(site_path_problem(r"C:\", r"C:\Users\a"), Some(TOO_WIDE));
        assert_eq!(site_path_problem(r"\\server\share\site", r"C:\Users\a"), Some(NOT_ABSOLUTE));
    }
}
