//! Port of features/pages.ts: the sites agents make (Blueprint plans, mockups) listed from
//! ~/.inyon/pages.json and served from this machine's disk under `/v/<token>/<path>`.
//!
//! inyon-core's `features::pages_core` holds the rules both hubs share (tokens, paths, headers);
//! this file adds what only a running hub has: the list file, the epochs that revoke links,
//! progress summaries, the API the app calls, and the `/v/` answer. The list file is the source of
//! truth. `inyon pages add` and Blueprint's publish script write it with no hub running, and the
//! hub reads it again whenever it changes.
//!
//! Threading. The core thread checks the token and resolves the path (with `tokio::fs`, whose
//! reads run on the blocking pool); the answer for a file leaves the core carrying only its path,
//! and the axum handler streams it (`file_body`), so a multi-MB image never holds up the core.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bytes::Bytes;

use crate::http::Body;
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use inyon_core::features::pages_core::{
    decide_view, gone_after_open, is_slug, mint_token, pages_key, progress_summary, site_path_problem, slugify, split_view_target, too_big, unique_slug, verify_token, view_origin,
    view_segments, within, FileStat, VerifyContext, ViewContext, ViewDecision, ViewFs, ViewRequest, BAD_SLUG, CANT_WRITE_LIST, GIT_ENV, GIT_TIMEOUT_MS, LOCAL_HOST, NO_INDEX,
    PAGES_LIST_MAX, TOKEN_TTL_MS,
};
use inyon_core::hosts::http_error::{http, HubError, HubResult};
use inyon_core::js::text::JsStr;
use inyon_core::platform::store::kv;
use inyon_protocol::PageProgress;
use serde_json::{json, Map, Number, Value};

use crate::http::{ok, ok_json, Req, Resp};
use crate::server::{guarded, h, Routes};
use inyon_core::features::pages_remote::{self, slashed, Listing, RemoteLists, RemotePage};
pub use inyon_core::features::pages_remote::{as_record, is_note, is_session, Record, NOTES_DIR, NOTES_MAX, NOT_CONNECTED, UNKNOWN_PAGE};

/// The store key (a file in the hub's data folder) that keeps each site's epoch, `{"<host>/<slug>": n}`.
pub const EPOCHS_KEY: &str = "page-epochs";
/// The store key that keeps what pages.json listed at the last read, `{"<host>/<slug>": added}`.
pub const LISTED_KEY: &str = "page-listed";
/// A lock older than this was left by a writer that died, and is taken over.
pub const LOCK_STALE_MS: u64 = 10_000;
/// How long a progress summary is kept before progress.json is read again.
pub const PROGRESS_TTL_MS: u64 = 30_000;
/// Bigger progress files aren't read for the list.
const PROGRESS_MAX: u64 = 1024 * 1024;
/// A bigger `.inyon-page.json` isn't read for a page's name and slug.
const MARKER_MAX: u64 = 64 * 1024;
/// A lock holds a process id and a tag; anything longer isn't one of ours.
const LOCK_MAX: u64 = 256;

pub const BROKEN_FILE: &str = "~/.inyon/pages.json isn't a page list Inyon can read. Fix it or delete it, then try again.";
pub const LOCKED: &str = "Another program is changing ~/.inyon/pages.json. Try again in a moment.";
pub const NOT_SAVED: &str = "The hub couldn't write to its data folder, so the old links stay closed only until it restarts. Check the hub's disk, then revoke again.";

/// `~/.inyon/pages.json`: always under the home folder (not INYON_DATA_DIR), where the CLI and Blueprint write it.
pub fn pages_file(home: &Path) -> PathBuf {
    home.join(".inyon").join("pages.json")
}

fn record_json(r: &Record) -> Value {
    let mut v = json!({ "slug": r.slug, "name": r.name, "path": r.path, "project": r.project, "by": r.by, "added": Value::Number(r.added.clone()) });
    if let Some(s) = &r.session {
        v["session"] = json!(s);
    }
    v
}

/// The session a page belongs to: the one that added it, or the one it was found under (kept by
/// the hub, since a machine's older inyon can't write it). With the name Inyon last gave it.
fn with_session(v: &mut Value, host: &str, r: &Record) {
    let session = r.session.clone().or_else(|| crate::finding::session_of(host, &r.slug));
    if let Some(s) = session {
        if let Some(name) = inyon_core::hosts::names::last_name(host, &s) {
            v["sessionName"] = json!(name);
        }
        v["session"] = json!(s);
    }
}

fn real_or(p: &str) -> String {
    std::fs::canonicalize(p).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_owned())
}

/// The file's text as `{v: 1, pages: [...]}`, or None when it isn't one.
fn parse(text: &str) -> Option<Map<String, Value>> {
    let v = inyon_core::js::json::parse(text)?;
    let o = v.as_object()?;
    (o.get("v").and_then(Value::as_f64) == Some(1.0) && o.get("pages").is_some_and(Value::is_array)).then(|| o.clone())
}

fn now_wall() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

// ---------- the list file ----------

/// What a `Registry` calls with the list each time it reads it again.
pub type OnList = Box<dyn Fn(&[Record])>;

/// ~/.inyon/pages.json, read when its size, mtime or inode changed since the last read. Only the
/// pages that may be served come out of `records()`: well-formed entries whose folder passes the
/// path rules, written and real, with the first of any repeated slug. A missing file is an empty
/// list. Writes go through `update()`, under `pages.json.lock` (created exclusively, taken over
/// after LOCK_STALE_MS), to a temp file renamed over the old one, keeping entries this hub can't
/// read as they were.
///
/// `on_list` hears the list each time it is read again (`Listed` keeps track of what went away).
pub struct Registry {
    pub file: PathBuf,
    home: String,
    real_home: String,
    seen: RefCell<String>,
    cached: RefCell<Vec<Record>>,
    broken: RefCell<bool>,
    on_list: OnList,
}

impl Registry {
    pub fn new(file: PathBuf, home: &Path) -> Registry {
        Registry::watched(file, home, Box::new(|_| {}))
    }

    pub fn watched(file: PathBuf, home: &Path, on_list: OnList) -> Registry {
        let home = home.to_string_lossy().into_owned();
        Registry { file, real_home: real_or(&home), home, seen: RefCell::new(String::new()), cached: RefCell::new(vec![]), broken: RefCell::new(false), on_list }
    }

    fn settle(&self, next: Vec<Record>) {
        *self.cached.borrow_mut() = next;
        (self.on_list)(&self.cached.borrow());
    }

    /// What is wrong with a folder as a page, by the path as written and by where it really leads.
    pub fn folder_problem(&self, p: &str) -> Option<&'static str> {
        site_path_problem(p, &self.home).or_else(|| site_path_problem(&real_or(p), &self.real_home))
    }

    fn load(&self) {
        let Ok(st) = std::fs::metadata(&self.file) else {
            self.seen.borrow_mut().clear();
            *self.broken.borrow_mut() = false;
            self.settle(vec![]);
            return;
        };
        let stamp = format!("{}:{}:{:?}", ino(&st), st.len(), st.modified().ok());
        if *self.seen.borrow() == stamp {
            return;
        }
        *self.seen.borrow_mut() = stamp;
        // a list that can't be read as a plain file of at most PAGES_LIST_MAX bytes is a broken one
        let text = read_list(&self.file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let parsed = parse(&text);
        *self.broken.borrow_mut() = parsed.is_none();
        let mut out: Vec<Record> = vec![];
        let pages = parsed.as_ref().and_then(|p| p.get("pages")).and_then(Value::as_array).cloned().unwrap_or_default();
        for raw in &pages {
            let Some(r) = as_record(raw) else { continue };
            if out.iter().any(|x| x.slug == r.slug) || self.folder_problem(&r.path).is_some() {
                continue;
            }
            out.push(r);
        }
        self.settle(out);
    }

    pub fn records(&self) -> Vec<Record> {
        self.load();
        self.cached.borrow().clone()
    }

    pub fn find(&self, slug: &str) -> Option<Record> {
        self.records().into_iter().find(|r| r.slug == slug)
    }

    /// Whether the file is there but isn't a list this hub can read (writes are refused then).
    pub fn broken(&self) -> bool {
        self.load();
        *self.broken.borrow()
    }

    /// Changes the file under its lock. `change` gets every entry as written (readable or not) and
    /// returns the new list, or None to leave the file alone, with its result.
    pub async fn update<T>(&self, change: impl FnOnce(&[Value]) -> HubResult<(Option<Vec<Value>>, T)>) -> HubResult<T> {
        self.update_with(Removed::Keep, change).await
    }

    /// `update`, also changing the list's `removed` folders when the file is written.
    pub async fn update_with<T>(&self, removed: Removed, change: impl FnOnce(&[Value]) -> HubResult<(Option<Vec<Value>>, T)>) -> HubResult<T> {
        let file = self.file.clone();
        with_lock(&lock_of(&self.file), 3000, move || {
            let current = match read_list(&file) {
                Ok(b) => parse(&String::from_utf8_lossy(&b)).ok_or_else(|| http(500, BROKEN_FILE))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({ "v": 1, "pages": [] }).as_object().cloned().unwrap_or_default(),
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Err(http(500, BROKEN_FILE)),
                Err(e) => return Err(HubError::plain(e.to_string())),
            };
            let pages = current.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
            let (next, result) = change(&pages)?;
            if let Some(next) = next {
                let mut out = current;
                out.insert("v".into(), json!(1));
                out.insert("pages".into(), Value::Array(next));
                removed.apply(&mut out);
                write_atomic(&file, &format!("{}\n", inyon_core::js::json::stringify_pretty(&Value::Object(out)))).map_err(|e| HubError::plain(e.to_string()))?;
            }
            Ok(result)
        })
        .await
    }
}

/// The most folders kept in pages.json's `removed`, the newest last (as the CLI keeps them).
pub const REMOVED_MAX: usize = 200;

/// What a change to the list does to its `removed` folders: those taken off the list, which
/// `inyon pages find` leaves alone so a page you removed doesn't come back by itself.
pub enum Removed {
    Keep,
    /// taken off the list
    Remember(Vec<String>),
    /// added again on purpose
    Forget(Vec<String>),
}

impl Removed {
    fn apply(self, list: &mut Map<String, Value>) {
        let mut removed: Vec<String> = list.get("removed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        let before = removed.clone();
        match self {
            Removed::Keep => return,
            Removed::Remember(paths) => {
                removed.retain(|p| !paths.contains(p));
                removed.extend(paths);
                let extra = removed.len().saturating_sub(REMOVED_MAX);
                removed.drain(..extra);
            }
            Removed::Forget(paths) => removed.retain(|p| !paths.contains(p)),
        }
        if removed != before {
            list.insert("removed".into(), json!(removed));
        }
    }
}

#[cfg(unix)]
fn ino(m: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(m)
}
#[cfg(not(unix))]
fn ino(_: &std::fs::Metadata) -> u64 {
    0
}

fn lock_of(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".lock");
    PathBuf::from(s)
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

fn create_private(p: &Path, exclusive: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    if exclusive {
        o.create_new(true);
    } else {
        o.create(true).truncate(true);
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(p)
}

/// A file's bytes, read without ever waiting on it: opened with O_NONBLOCK and without following a
/// symlink at its end, then read only when the open file is a plain file of at most `max` bytes. A
/// FIFO an agent leaves in a page folder would otherwise hold the read, and with it the whole hub,
/// until something wrote to it. A plain file reads the same with O_NONBLOCK set.
pub fn read_plain(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    read_capped(path, max, false)
}

/// ~/.inyon/pages.json's bytes, read as `read_plain` reads, at most PAGES_LIST_MAX of them. The
/// list may be a symlink (a dotfile setup), so the one at the end of its path is followed.
pub fn read_list(path: &Path) -> std::io::Result<Vec<u8>> {
    read_capped(path, PAGES_LIST_MAX, true)
}

fn read_capped(path: &Path, max: u64, follow: bool) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let not_plain = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a plain file of a size the hub reads");
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut o, if follow { libc::O_NONBLOCK } else { libc::O_NOFOLLOW | libc::O_NONBLOCK });
    #[cfg(not(unix))]
    let _ = follow;
    let file = o.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > max {
        return Err(not_plain());
    }
    // read to the end (it may have grown since), refusing it once it passes `max`; the buffer grows
    // as it reads, never sized from the file, which can claim terabytes it doesn't have
    let mut out = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(not_plain());
    }
    Ok(out)
}

/// Writes beside the file and renames over it, so a reader never sees half a list. A symlinked list
/// (a dotfile setup) is written where the link leads: the rename replaces that file, and the link
/// stays.
fn write_atomic(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let file = &link_target(file);
    if let Some(dir) = file.parent() {
        private_dir(dir)?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let r = (|| {
        create_private(&tmp, false)?.write_all(text.as_bytes())?;
        std::fs::rename(&tmp, file)
    })();
    let _ = std::fs::remove_file(&tmp);
    r
}

/// The path a chain of symlinks ends at, read link by link so a link to a file not made yet still
/// leads somewhere; the path itself when it isn't a link. Gives up after 40 links (a loop).
pub fn link_target(file: &Path) -> PathBuf {
    let mut p = file.to_owned();
    for _ in 0..40 {
        let Ok(to) = std::fs::read_link(&p) else { return p };
        p = lexical(&p.parent().unwrap_or(Path::new("")).join(to));
    }
    p
}

/// Runs `f` holding `lock`: an exclusive create, waiting up to `wait` ms, taking over a lock older
/// than LOCK_STALE_MS. The lock holds this process's id and a random tag, and is removed afterwards
/// only while it still holds them. A failure other than "someone else has it" (a read-only folder,
/// or a stale lock that can't be removed) is returned at once as CANT_WRITE_LIST, never waited on.
pub async fn with_lock<T>(lock: &Path, wait: u64, f: impl FnOnce() -> HubResult<T>) -> HubResult<T> {
    use std::io::Write;
    let plain = |_: std::io::Error| http(500, CANT_WRITE_LIST);
    if let Some(dir) = lock.parent() {
        private_dir(dir).map_err(plain)?;
    }
    let until = now_wall() + wait;
    let mine = format!("{} {}\n", std::process::id(), crate::http::random_hex(8));
    loop {
        if make_lock(lock, &mine, |f, b| f.write_all(b)).map_err(plain)? {
            break;
        }
        if take_over_stale(lock).map_err(plain)? {
            continue;
        }
        if now_wall() >= until {
            return Err(http(503, LOCKED));
        }
        inyon_core::platform::rt::sleep(25).await;
    }
    let r = f();
    release_lock(lock, &mine);
    r
}

/// Makes the lock with this writer's tag in it; false when another writer has it. A lock whose tag
/// couldn't be written (a full disk) could never be released, so it is taken away again, while the
/// path still leads to the file this writer made.
pub fn make_lock(lock: &Path, mine: &str, write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>) -> std::io::Result<bool> {
    let mut f = match create_private(lock, true) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    if let Err(e) = write(&mut f, mine.as_bytes()) {
        if f.metadata().ok().zip(std::fs::metadata(lock).ok()).is_some_and(|(made, now)| same_file(&made, &now)) {
            let _ = std::fs::remove_file(lock);
        }
        return Err(e);
    }
    Ok(true)
}

/// The file's identity, to tell a lock judged stale from a new one made in its place.
fn identity(m: &std::fs::Metadata) -> (u64, Option<std::time::SystemTime>) {
    (ino(m), m.modified().ok())
}

/// Removes a lock left by a writer that died (older than LOCK_STALE_MS); true when the caller
/// should try to take it again at once. The lock is first renamed aside, and removed only if what
/// was renamed is the file that was judged stale: two writers that both saw it stale can't remove
/// the fresh lock one of them made in between (that one is put back).
pub fn take_over_stale(lock: &Path) -> std::io::Result<bool> {
    use std::io::ErrorKind::NotFound;
    let old = match std::fs::metadata(lock) {
        Ok(m) => m,
        Err(e) if e.kind() == NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    let age = old.modified().ok().and_then(|t| t.elapsed().ok()).map_or(0, |d| d.as_millis() as u64);
    if age <= LOCK_STALE_MS {
        return Ok(false);
    }
    remove_judged(lock, &old)?;
    Ok(true)
}

/// Removes the lock that was judged stale as `old`, and only that one: renamed aside, then put back
/// if it turns out to be a new lock another writer made since.
pub fn remove_judged(lock: &Path, old: &std::fs::Metadata) -> std::io::Result<()> {
    use std::io::ErrorKind::NotFound;
    let mut aside = lock.as_os_str().to_owned();
    aside.push(format!(".{}.{}.stale", std::process::id(), crate::http::random_hex(4)));
    let aside = PathBuf::from(aside);
    match std::fs::rename(lock, &aside) {
        Ok(()) => {}
        Err(e) if e.kind() == NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    if std::fs::metadata(&aside).map(|m| identity(&m)).ok() != Some(identity(old)) {
        // another writer's new lock: back where it was, unless a third has made one since
        let _ = std::fs::hard_link(&aside, lock);
    }
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

/// Removes the lock if it is still this writer's (read as the list is, so a FIFO put there isn't
/// waited on).
pub fn release_lock(lock: &Path, mine: &str) {
    if read_plain(lock, LOCK_MAX).is_ok_and(|t| t == mine.as_bytes()) {
        let _ = std::fs::remove_file(lock);
    }
}

// ---------- epochs ----------

/// Each site's epoch, kept in the hub's data folder. A token carries the epoch it was made at, so
/// moving it on (Revoke, or removing the page) ends every link made before.
pub struct Epochs(RefCell<Map<String, Value>>);

impl Epochs {
    pub fn load() -> Epochs {
        let mut m = Map::new();
        if let Some(Value::Object(raw)) = kv::get_value(EPOCHS_KEY) {
            for (k, v) in raw {
                if let Some(n) = safe_int(&v) {
                    m.insert(k, json!(n));
                }
            }
        }
        Epochs(RefCell::new(m))
    }

    pub fn of(&self, host: &str, slug: &str) -> u64 {
        self.0.borrow().get(&format!("{host}/{slug}")).and_then(Value::as_u64).unwrap_or(0)
    }

    /// Moves a site's epoch on. The running hub refuses the old links at once; when the new epoch
    /// can't be saved (a full disk) it returns NOT_SAVED, since a restart would open them again.
    pub fn bump(&self, host: &str, slug: &str) -> HubResult<u64> {
        let n = self.of(host, slug) + 1;
        let all = {
            let mut m = self.0.borrow_mut();
            m.insert(format!("{host}/{slug}"), json!(n));
            Value::Object(m.clone())
        };
        if let Err(e) = kv::set_value(EPOCHS_KEY, &all) {
            inyon_core::platform::console::error(&format!("couldn't save the page epochs: {e}"));
            return Err(http(500, NOT_SAVED));
        }
        Ok(n)
    }
}

/// `Number.isSafeInteger(v) && v >= 0`
fn safe_int(v: &Value) -> Option<u64> {
    let n = v.as_f64()?;
    (n.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&n)).then_some(n as u64)
}

/// What the lists held when the hub last read them, `{"<host>/<slug>": added}`, kept in the hub's
/// data folder: its own pages.json and each other machine's. The CLI and Blueprint take pages off a
/// list without the hub, even while it is stopped; when a slug has gone, or now names a page added
/// at another time, `gone` hears of it (host, slug) so its old links end before a page added again
/// under the same slug could open them.
pub struct Listed {
    last: RefCell<Map<String, Value>>,
    gone: OnGone,
}

/// What `Listed` calls with the host and slug of a page that went.
pub type OnGone = Box<dyn Fn(&str, &str)>;

impl Listed {
    pub fn load(gone: OnGone) -> Listed {
        let mut last = Map::new();
        if let Some(Value::Object(raw)) = kv::get_value(LISTED_KEY) {
            for (k, v) in raw {
                if v.is_number() {
                    last.insert(k, v);
                }
            }
        }
        Listed { last: RefCell::new(last), gone }
    }

    /// One machine's list, just read (`host` is LOCAL_HOST for the hub's own).
    pub fn saw(&self, host: &str, records: &[Record]) {
        let prefix = format!("{host}/");
        let mut next: Map<String, Value> = self.last.borrow().iter().filter(|(k, _)| !k.starts_with(&prefix)).map(|(k, v)| (k.clone(), v.clone())).collect();
        for r in records {
            next.insert(format!("{prefix}{}", r.slug), Value::Number(r.added.clone()));
        }
        let gone: Vec<String> = {
            let last = self.last.borrow();
            let mut changed = next.len() != last.len();
            let mut gone = vec![];
            for (k, added) in last.iter() {
                let Some(slug) = k.strip_prefix(&prefix) else { continue };
                if next.get(k).is_some_and(|n| same_number(n, added)) {
                    continue;
                }
                changed = true;
                gone.push(slug.to_owned());
            }
            changed |= next.iter().any(|(k, _)| k.starts_with(&prefix) && !last.contains_key(k));
            if !changed {
                return;
            }
            gone
        };
        for slug in gone {
            (self.gone)(host, &slug);
        }
        if let Err(e) = kv::set_value(LISTED_KEY, &Value::Object(next.clone())) {
            inyon_core::platform::console::error(&format!("couldn't save the pages listed: {e}"));
        }
        *self.last.borrow_mut() = next;
    }
}

/// `===` on two JSON numbers, as JavaScript compares them.
fn same_number(a: &Value, b: &Value) -> bool {
    a.as_f64().is_some_and(|x| b.as_f64() == Some(x))
}

// ---------- progress ----------

/// progress.json summaries by folder, each read at most once every PROGRESS_TTL_MS.
#[derive(Default)]
pub struct ProgressCache(RefCell<HashMap<String, (u64, Option<PageProgress>)>>);

impl ProgressCache {
    pub fn of(&self, folder: &str, now: u64) -> Option<PageProgress> {
        if let Some((at, v)) = self.0.borrow().get(folder) {
            if now.saturating_sub(*at) < PROGRESS_TTL_MS {
                return v.clone();
            }
        }
        let value = read_plain(&Path::new(folder).join("progress.json"), PROGRESS_MAX)
            .ok()
            .and_then(|b| inyon_core::js::json::parse(&String::from_utf8_lossy(&b)))
            .and_then(|d| progress_summary(&d));
        self.0.borrow_mut().insert(folder.to_owned(), (now, value.clone()));
        value
    }

    pub fn drop_folder(&self, folder: &str) {
        self.0.borrow_mut().remove(folder);
    }
}

// ---------- disk ----------

/// decide_view's disk through tokio::fs (its calls run on the blocking pool); both follow symlinks.
pub struct DiskFs;

/// Node's `mtimeMs`: seconds times 1000 plus nanoseconds over a million, as a double.
pub fn mtime_ms(m: &std::fs::Metadata) -> f64 {
    match m.modified().ok().map(|t| t.duration_since(std::time::UNIX_EPOCH)) {
        Some(Ok(d)) => d.as_secs() as f64 * 1e3 + f64::from(d.subsec_nanos()) / 1e6,
        Some(Err(e)) => -(e.duration().as_secs_f64() * 1e3),
        None => 0.0,
    }
}

impl ViewFs for DiskFs {
    fn stat<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<FileStat>> {
        Box::pin(async move {
            let m = tokio::fs::metadata(p).await.ok()?;
            Some(FileStat { file: m.is_file(), dir: m.is_dir(), size: m.len(), mtime_ms: mtime_ms(&m) })
        })
    }
    fn realpath<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<PathBuf>> {
        Box::pin(async move { tokio::fs::canonicalize(p).await.ok() })
    }
}

/// The line git prints for `git -C <folder> rev-parse <option>`, or None. A line starting with '-'
/// is refused: a git too old to know the option prints it back and still exits 0 (git before 2.31
/// does that with --path-format, before 2.5 with --git-common-dir). Git is killed at `until`: a
/// named pipe in a folder's `.git` would otherwise keep it waiting for good.
async fn rev_parse(git: &str, folder: &str, option: &str, until: tokio::time::Instant) -> Option<String> {
    let child = tokio::process::Command::new(git)
        .args(["-C", folder, "rev-parse", option])
        .envs(GIT_ENV)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    // past `until` the wait is dropped, and with it the child, which kill_on_drop kills
    let out = tokio::time::timeout_at(until, child.wait_with_output()).await.ok()?.ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.split('\n').next().unwrap_or("").jtrim();
    (!line.is_empty() && !line.starts_with('-')).then(|| line.to_owned())
}

/// The project a folder belongs to: its git repo's main working tree, or the folder itself. In a
/// linked worktree (an agent's `.claude/worktrees/<name>`, removed once its branch merges) git's
/// common dir is the main tree's `.git`, so the page is grouped under the repo, as the CLI and the
/// Blueprint skill group it. A submodule's common dir (`.git/modules/<name>`) or a bare repo's
/// (`<name>.git`) keeps the top folder. Two plain calls work on any git; the top folder git prints is
/// kept when the common dir's parent is the same folder. Both calls together get GIT_TIMEOUT_MS.
pub(crate) async fn project_of(folder: &str, git: &str) -> String {
    let until = tokio::time::Instant::now() + std::time::Duration::from_millis(GIT_TIMEOUT_MS);
    let Some(top) = rev_parse(git, folder, "--show-toplevel", until).await else {
        return folder.to_owned();
    };
    let Some(common) = rev_parse(git, folder, "--git-common-dir", until).await.map(|c| lexical(&Path::new(folder).join(c))) else {
        return top;
    };
    if common.file_name().is_none_or(|n| n != ".git") || !common.is_dir() {
        return top;
    }
    let Some(parent) = common.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return top;
    };
    match (std::fs::canonicalize(parent), std::fs::canonicalize(&top)) {
        (Ok(a), Ok(b)) if a != b => parent.to_str().map_or(top, str::to_owned),
        _ => top,
    }
}

/// A path with `.` and `..` worked out from the text, as path.resolve does.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// The Markdown notes in a site's `research/` folder, by name, sorted: plain files only, and only
/// when `research` is a real folder, not a symlink. `inyon pages --json` lists them the same way
/// (apps/cli/src/pages.rs) for a page on another machine.
pub fn notes_of(site: &Path) -> Vec<String> {
    let dir = site.join(NOTES_DIR);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut out: Vec<String> =
        entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).filter_map(|e| e.file_name().into_string().ok()).filter(|n| is_note(n)).collect();
    out.sort_by_key(|n| n.to_lowercase());
    out.truncate(NOTES_MAX);
    out
}

/// The name and slug in a site's `.inyon-page.json`, when it has usable ones.
fn marker_of(folder: &str) -> (Option<String>, Option<String>) {
    let Some(Value::Object(m)) = read_plain(&Path::new(folder).join(".inyon-page.json"), MARKER_MAX).ok().and_then(|b| inyon_core::js::json::parse(&String::from_utf8_lossy(&b)))
    else {
        return (None, None);
    };
    let name = m.get("name").and_then(Value::as_str).map(|n| n.jtrim()).filter(|n| !n.is_empty()).map(|n| n.to16(200).to_owned());
    let slug = m.get("slug").and_then(Value::as_str).filter(|s| is_slug(s)).map(str::to_owned);
    (name, slug)
}

// ---------- the feature ----------

/// What the feature keeps, on the core thread.
pub struct Pages {
    pub home: PathBuf,
    pub registry: Registry,
    pub epochs: Rc<Epochs>,
    pub progress: ProgressCache,
    pub now: Rc<dyn Fn() -> u64>,
    /// the disk decide_view looks at (a test swaps folders under it)
    pub fs: Rc<dyn ViewFs>,
    /// what the lists held at the last read, every machine's
    pub listed: Rc<Listed>,
    /// the other machines' pages (pages_remote)
    pub remote: RemoteLists,
}

impl Pages {
    pub fn new(home: PathBuf, now: Rc<dyn Fn() -> u64>) -> Pages {
        let epochs = Rc::new(Epochs::load());
        let bump = epochs.clone();
        // a page taken off the list by anyone (the CLI, Blueprint, a hand edit) ends its links
        // a failed save is logged by bump; the running hub refuses the links all the same
        let listed = Rc::new(Listed::load(Box::new(move |host, slug| {
            let _ = bump.bump(host, slug);
        })));
        let seen = listed.clone();
        let registry = Registry::watched(pages_file(&home), &home, Box::new(move |records| seen.saw(LOCAL_HOST, records)));
        Pages { registry, home, epochs, progress: ProgressCache::default(), now, fs: Rc::new(DiskFs), listed, remote: RemoteLists::default() }
    }

    fn entry(&self, r: &Record) -> Value {
        let mut v = json!({ "host": LOCAL_HOST, "slug": r.slug, "name": r.name, "project": r.project, "by": r.by, "added": Value::Number(r.added.clone()), "folder": r.path });
        if let Some(p) = self.progress.of(&r.path, (self.now)()) {
            v["progress"] = serde_json::to_value(p).unwrap_or(Value::Null);
        }
        let notes = notes_of(Path::new(&r.path));
        if !notes.is_empty() {
            v["notes"] = json!(notes);
        }
        with_session(&mut v, LOCAL_HOST, r);
        v
    }

    /// A page on another machine, as the API lists it.
    fn remote_entry(host: &str, p: &RemotePage) -> Value {
        let r = &p.record;
        let mut v = json!({ "host": host, "slug": r.slug, "name": r.name, "project": r.project, "by": r.by, "added": Value::Number(r.added.clone()), "folder": r.path });
        if let Some(progress) = &p.progress {
            v["progress"] = progress.clone();
        }
        if !p.notes.is_empty() {
            v["notes"] = json!(p.notes);
        }
        with_session(&mut v, host, r);
        v
    }

    fn saw(&self) -> impl Fn(&str, &[Record]) + '_ {
        |host, records| self.listed.saw(host, records)
    }

    async fn page(&self, host: &str, slug: &str) -> HubResult<Record> {
        let r = if !is_slug(slug) {
            None
        } else if host == LOCAL_HOST {
            self.registry.find(slug)
        } else {
            self.remote.find(host, slug, (self.now)(), &self.saw()).await.map(|p| p.record)
        };
        r.ok_or_else(|| http(404, UNKNOWN_PAGE))
    }
}

thread_local! {
    static PAGES: RefCell<Option<Rc<Pages>>> = const { RefCell::new(None) };
}

/// The feature for this thread's hub: pages.json under `home`, epochs from the records.
pub fn init(home: PathBuf) {
    install(Pages::new(home, Rc::new(inyon_core::clock::now_ms)));
}

pub fn install(p: Pages) {
    PAGES.with(|x| *x.borrow_mut() = Some(Rc::new(p)));
}

pub fn pages() -> Rc<Pages> {
    PAGES.with(|x| x.borrow().clone()).expect("pages::init has run on this thread")
}

/// `readBody`: a JSON object of at most 16 KB ("{}" when empty).
async fn read_body(req: &mut Req) -> HubResult<Map<String, Value>> {
    const LIMIT: usize = 16 * 1024;
    // the TS counts the decoded text, in UTF-16 units; four bytes a unit is the most that can be
    let bytes = req.read_limited(LIMIT * 4, "Request too large").await.map_err(|e| if e.status == Some(413) { e } else { http(400, "Invalid JSON") })?;
    if req.header("content-length").is_some_and(|v| inyon_core::js::to_number(&v) > LIMIT as f64) {
        return Err(http(413, "Request too large"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.len16() > LIMIT {
        return Err(http(413, "Request too large"));
    }
    match inyon_core::js::json::parse(if text.is_empty() { "{}" } else { text }) {
        Some(Value::Object(m)) => Ok(m),
        _ => Err(http(400, "Invalid JSON")),
    }
}

/// Every machine's pages with their progress, as GET /api/pages lists them.
pub async fn entries(p: &Pages) -> Vec<Value> {
    let mut all: Vec<Value> = p.registry.records().iter().map(|r| p.entry(r)).collect();
    for (host, list) in p.remote.all((p.now)(), &p.saw()).await {
        all.extend(list.iter().map(|r| Pages::remote_entry(&host, r)));
    }
    all
}

pub async fn list(_req: Req) -> HubResult<Resp> {
    Ok(ok_json(&json!({ "pages": entries(&pages()).await })))
}

pub async fn open(req: Req) -> HubResult<Resp> {
    let p = pages();
    let host = req.param("host");
    let r = p.page(&host, &req.param("slug")).await?;
    let phone = crate::phones::phone_for(&req).map(|ph| ph.id);
    let (token, expires) = mint_token(&pages_key(&crate::password::session_secret()), &host, &r.slug, p.epochs.of(&host, &r.slug), phone.as_deref(), (p.now)());
    Ok(ok_json(&json!({ "url": format!("/v/{token}/"), "expires": expires, "ttl": TOKEN_TTL_MS })))
}

pub async fn revoke(req: Req) -> HubResult<Resp> {
    let p = pages();
    let host = req.param("host");
    let r = p.page(&host, &req.param("slug")).await?;
    p.epochs.bump(&host, &r.slug)?;
    Ok(ok())
}

/// POST /api/pages/:host: adds a folder, and tells the apps that follow the pages.
pub async fn register(req: Req) -> HubResult<Resp> {
    let r = register_page(req).await;
    if r.is_ok() {
        crate::pages_stream::read();
    }
    r
}

async fn register_page(mut req: Req) -> HubResult<Resp> {
    let b = read_body(&mut req).await?;
    let host = req.param("host");
    let p = pages();
    if host != LOCAL_HOST {
        return register_remote(&p, &host, &b).await;
    }
    let given = b.get("path").and_then(Value::as_str).map(|s| s.jtrim().to_owned()).unwrap_or_default();
    let folder = if given.len16() > 1 { given.trim_end_matches(['/', '\\']).to_owned() } else { given };
    if let Some(problem) = p.registry.folder_problem(&folder) {
        return Err(http(400, problem));
    }
    let asked_slug = match b.get("slug") {
        None => None,
        Some(Value::String(s)) if is_slug(s) => Some(s.clone()),
        Some(_) => return Err(http(400, BAD_SLUG)),
    };
    let asked = match b.get("name") {
        None => None,
        Some(Value::String(n)) => Some(n.jtrim().to16(200).to_owned()),
        Some(_) => return Err(http(400, "The name must be text")),
    };
    if !std::fs::metadata(Path::new(&folder).join("index.html")).is_ok_and(|m| m.is_file()) {
        return Err(http(400, NO_INDEX));
    }
    let (marker_name, marker_slug) = marker_of(&folder);
    let asked = asked.filter(|a| !a.is_empty());
    let basename = folder.rsplit('/').next().unwrap_or("").to_owned();
    let name = asked.clone().or(marker_name).unwrap_or_else(|| basename.clone());
    let project = project_of(&folder, "git").await;
    let now = (p.now)();
    let record = p
        .registry
        .update_with(Removed::Forget(vec![folder.clone(), real_or(&folder)]), |pages| {
            let records: Vec<Option<Record>> = pages.iter().map(as_record).collect();
            if let Some(i) = records.iter().position(|r| r.as_ref().is_some_and(|r| r.path == folder)) {
                let same = records[i].clone().expect("found");
                if asked_slug.as_ref().is_some_and(|s| *s != same.slug) {
                    return Err(http(409, format!("That folder is already a page, as \"{}\"", same.slug)));
                }
                let Some(asked) = asked.filter(|a| *a != same.name) else { return Ok((None, same)) };
                let mut next = pages.to_vec();
                if let Some(o) = next[i].as_object_mut() {
                    o.insert("name".into(), json!(asked));
                }
                return Ok((Some(next), Record { name: asked, ..same }));
            }
            let taken = |s: &str| records.iter().flatten().any(|r| r.slug == s) || pages.iter().any(|p| p.get("slug").and_then(Value::as_str) == Some(s));
            let slug = match &asked_slug {
                Some(s) if taken(s) => return Err(http(409, format!("There is already a page called \"{s}\" on this machine"))),
                Some(s) => s.clone(),
                None => unique_slug(&marker_slug.clone().unwrap_or_else(|| slugify(&basename)), taken),
            };
            let added = Record { slug, name: name.clone(), path: folder.clone(), project: project.clone(), by: "app".into(), added: Number::from(now), session: None };
            let mut next = pages.to_vec();
            next.push(record_json(&added));
            Ok((Some(next), added))
        })
        .await?;
    p.progress.drop_folder(&record.path);
    Ok(ok_json(&p.entry(&record)))
}

/// DELETE /api/pages/:host/:slug: takes a page off the list, and tells the apps that follow the pages.
pub async fn remove(req: Req) -> HubResult<Resp> {
    let r = remove_page(req).await;
    if r.is_ok() {
        crate::pages_stream::read();
    }
    r
}

async fn remove_page(req: Req) -> HubResult<Resp> {
    let (host, slug) = (req.param("host"), req.param("slug"));
    if !is_slug(&slug) {
        return Err(http(404, UNKNOWN_PAGE));
    }
    let p = pages();
    if host != LOCAL_HOST {
        p.page(&host, &slug).await?;
        pages_remote::remove(&host, &slug).await?;
        p.remote.forget(&host);
        // its links end now, and stay dead if the slug comes back
        p.epochs.bump(&host, &slug)?;
        return Ok(ok());
    }
    let folders: Vec<String> = p.registry.records().iter().filter(|r| r.slug == slug).map(|r| real_or(&r.path)).collect();
    let gone = p
        .registry
        .update_with(Removed::Remember(folders), |pages| {
            let keep: Vec<Value> = pages.iter().filter(|v| !(v.is_object() && v.get("slug").and_then(Value::as_str) == Some(slug.as_str()))).cloned().collect();
            if keep.len() == pages.len() {
                return Err(http(404, UNKNOWN_PAGE));
            }
            let gone: Vec<Record> = pages.iter().filter_map(as_record).filter(|r| r.slug == slug).collect();
            Ok((Some(keep), gone))
        })
        .await?;
    // links made before stay dead if the slug is used again (reading the list now reports the slug
    // gone, which moves the epoch on)
    p.registry.records();
    for r in gone {
        p.progress.drop_folder(&r.path);
    }
    Ok(ok())
}

/// Adds a folder on another machine, through its own inyon (which checks the folder by the same
/// rules), and answers with the entry as the list will have it.
async fn register_remote(p: &Pages, host: &str, b: &Map<String, Value>) -> HubResult<Resp> {
    if pages_remote::machines().iter().all(|m| m != host) {
        return Err(http(404, NOT_CONNECTED));
    }
    let folder = b.get("path").and_then(Value::as_str).map(|s| s.jtrim().to_owned()).unwrap_or_default();
    if !inyon_core::features::pages_core::is_absolute(&folder) {
        return Err(http(400, inyon_core::features::pages_core::NOT_ABSOLUTE));
    }
    let slug = match b.get("slug") {
        None => None,
        Some(Value::String(s)) if is_slug(s) => Some(s.as_str()),
        Some(_) => return Err(http(400, BAD_SLUG)),
    };
    let name = match b.get("name") {
        None => None,
        Some(Value::String(n)) => Some(n.jtrim().to16(200).to_owned()),
        Some(_) => return Err(http(400, "The name must be text")),
    };
    let added = pages_remote::add(host, &folder, name.as_deref(), slug).await?;
    p.remote.forget(host);
    Ok(ok_json(&Pages::remote_entry(host, &added)))
}

/// The API routes, behind sign-in (a cookie, or a paired phone's bearer).
pub fn add(r: &mut Routes) {
    guarded(r, "/api/pages", vec![(Method::GET, h(list))]);
    guarded(r, "/api/pages/:host", vec![(Method::POST, h(register))]);
    guarded(r, "/api/pages/:host/:slug", vec![(Method::DELETE, h(remove))]);
    guarded(r, "/api/pages/:host/:slug/open", vec![(Method::POST, h(open))]);
    guarded(r, "/api/pages/:host/:slug/revoke", vec![(Method::POST, h(revoke))]);
}

// ---------- /v/ ----------

/// A 200's file, opened and checked, for the axum handler to stream once the answer is off the
/// core thread. (An extension has to be Clone, so the file sits in a shared slot it is taken from.)
#[derive(Debug, Clone)]
pub struct ViewFile {
    pub file: std::sync::Arc<std::sync::Mutex<Option<std::fs::File>>>,
    pub size: u64,
}

/// The answer to a `/v/` request. It never reads the Cookie or Authorization header and never
/// sets a cookie: the token in the path is the only credential. The Host is checked against the
/// names the hub answers to directly, since right_host() would let a signed-in cookie through.
pub async fn view(req: Req) -> Resp {
    let c = crate::server::ctx();
    let host = req.header("host");
    let forwarded = req.header("x-forwarded-host");
    let mut names: Vec<String> = host.iter().cloned().collect();
    if let Some(f) = &forwarded {
        names.extend(f.split(',').map(|h| h.trim().to_owned()));
    }
    let answered = !names.is_empty() && names.iter().all(|h| c.answers_to(h));
    let origin = if answered { view_origin(forwarded.as_deref().or(host.as_deref()), crate::index::secure_request(&req)) } else { None };
    let target = if req.query.is_empty() { req.path.clone() } else { format!("{}?{}", req.path, req.query) };
    let p = pages();
    let key = pages_key(&crate::password::session_secret());
    let epoch_of = |h: &str, s: &str| p.epochs.of(h, s);
    let phone_paired = |id: &str| crate::phones::phone_paired(id);
    // read the list first, so a page removed since the last request has its epoch moved on before
    // its token is checked
    p.registry.records();
    let inm = req.header("if-none-match");
    let method = req.method.as_str().to_owned();
    let verify = VerifyContext { key: &key, now: (p.now)(), epoch_of: &epoch_of, phone_paired: &phone_paired };
    // a page on another machine: what its inyon says about this request, asked once, up front
    let remote = remote_listing(&p, &method, &target, &verify, inm.as_deref()).await;
    let site_root = |h: &str, s: &str| match &remote {
        Some(r) => (r.host == h && r.slug == s).then(|| r.root.clone()),
        None if h == LOCAL_HOST => p.registry.find(s).map(|r| PathBuf::from(r.path)),
        None => None,
    };
    let fs: &dyn ViewFs = match &remote {
        Some(r) => r.listing.as_ref(),
        None => p.fs.as_ref(),
    };
    let ctx = ViewContext { verify, site_root: &site_root, fs };
    let vr = ViewRequest { method: &method, target: &target, origin: origin.as_deref(), if_none_match: inm.as_deref() };
    let d = decide_view(&vr, &ctx).await;
    let head = req.method == Method::HEAD;
    let note = d.file.as_ref().is_some_and(|(path, size)| *size <= crate::notes::NOTE_MAX as u64 && crate::notes::wants_note(&req.query, path));
    if let Some(r) = &remote {
        let Some((path, _)) = &d.file else { return respond(d, head, None) };
        return match r.listing.bytes_for(path) {
            Some(bytes) if note => respond_note(d, head, bytes),
            Some(bytes) => respond_bytes(d, head, bytes.to_vec()),
            None if r.listing.big => respond(too_big(&vr), head, None),
            None => respond(gone_after_open(&vr), head, None),
        };
    }
    match (&d.file, &d.site) {
        (Some((path, _)), Some(site)) => match open_within(path, site).await {
            Some((file, size)) if note => match read_opened(file, size) {
                Some(bytes) => respond_note(d, head, &bytes),
                None => respond(gone_after_open(&vr), head, None),
            },
            Some(opened) => respond(d, head, Some(opened)),
            None => respond(gone_after_open(&vr), head, None),
        },
        _ => respond(d, head, None),
    }
}

/// An opened file read whole, when it is still `size` bytes long.
fn read_opened(mut file: std::fs::File, size: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::with_capacity(size as usize);
    file.by_ref().take(size + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 == size).then_some(bytes)
}

/// A note (`?as=note`) as a page to read, with the headers decide_view gave the file but its own
/// type and tag, so a cached copy of the Markdown and of the page are never taken for each other.
fn respond_note(mut d: ViewDecision, head: bool, md: &[u8]) -> Resp {
    let name = d.file.as_ref().and_then(|(p, _)| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for (k, v) in d.headers.iter_mut() {
        match *k {
            "content-type" => *v = "text/html; charset=utf-8".into(),
            "etag" => *v = format!("{}-note\"", v.trim_end_matches('"')),
            _ => {}
        }
    }
    respond_bytes(d, head, crate::notes::page(&name, &String::from_utf8_lossy(md)).into_bytes())
}

/// A `/v/` request for a page on another machine, with what that machine said about it.
struct RemoteView {
    host: String,
    slug: String,
    /// the site's folder as the hub joins paths (slashed)
    root: PathBuf,
    listing: Rc<Listing>,
}

/// For a request whose token is good and names a page on another machine that is still listed
/// there: asks that machine about it, once. None for anything else, which decide_view then answers
/// as it does (403, 404, 405, 301). While the machine's `pages watch` runs, a file it already sent
/// is answered from the hub's copy until its site's stamp moves on, without asking again.
async fn remote_listing(p: &Pages, method: &str, target: &str, verify: &VerifyContext<'_>, if_none_match: Option<&str>) -> Option<RemoteView> {
    if method != "GET" && method != "HEAD" {
        return None;
    }
    let t = split_view_target(target)?;
    let claims = verify_token(&t.token, verify).ok()?;
    if claims.h == LOCAL_HOST || t.rest.is_empty() {
        return None;
    }
    let (segments, slash) = view_segments(&t.rest)?;
    let page = p.remote.find(&claims.h, &claims.s, (p.now)(), &p.saw()).await?;
    let root = slashed(&page.record.path);
    // a machine that doesn't answer leaves nothing listed, so the request is a 404
    let listing = if p.remote.watching(&claims.h) {
        let key = pages_remote::file_key(&claims.h, &claims.s, &segments, slash);
        let stamp = p.remote.stamp(&claims.h, &claims.s);
        match stamp.as_deref().and_then(|st| p.remote.kept(&key, st)) {
            Some(kept) => kept,
            None => {
                let l = Rc::new(pages_remote::get_lines(&claims.h, &claims.s, &root, &segments, slash, pages_remote::STREAM_FILE_MAX).await.unwrap_or_default());
                if let Some(st) = stamp.filter(|_| l.answered() && !l.short) {
                    p.remote.keep(key, st, l.clone());
                }
                l
            }
        }
    } else {
        Rc::new(pages_remote::get(&claims.h, &claims.s, &root, &segments, slash, if_none_match, pages_remote::REMOTE_FILE_MAX).await.unwrap_or_default())
    };
    Some(RemoteView { host: claims.h, slug: claims.s, root, listing })
}

/// Opens a file decide_view checked, then makes sure the open file is still inside the site: a
/// folder on the way could have been swapped for a symlink since the check (an agent can rename
/// its own folders). The system says where the open file really is (/proc/self/fd on Linux,
/// F_GETPATH on macOS); elsewhere the path must still have no symlink in it and lead to the same
/// file. The file and its size, or None when it can't be opened or isn't the site's.
pub async fn open_within(path: &Path, site: &Path) -> Option<(std::fs::File, u64)> {
    let mut o = tokio::fs::OpenOptions::new();
    o.read(true);
    // O_NONBLOCK keeps a file swapped for a FIFO since the check from holding the open until
    // something writes to it; anything but a plain file is then refused, and a plain file's reads
    // go back to blocking ones
    #[cfg(unix)]
    o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = o.open(path).await.ok()?.into_std().await;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        // SAFETY: fd is the open file this function owns; F_GETFL and F_SETFL only touch its flags
        let ok = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags != -1 && libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) != -1
        };
        if !ok {
            return None;
        }
    }
    let inside = match opened_path(&file) {
        Some(at) => within(site, &at),
        None => {
            let real = tokio::fs::canonicalize(path).await.ok();
            let now = tokio::fs::metadata(path).await.ok();
            real.as_deref() == Some(path) && now.is_some_and(|n| same_file(&n, &meta)) && within(site, path)
        }
    };
    inside.then_some((file, meta.len()))
}

/// Where an open file really is, as the system tracks it.
#[cfg(target_os = "linux")]
fn opened_path(f: &std::fs::File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", f.as_raw_fd())).ok()
}

#[cfg(target_os = "macos")]
fn opened_path(f: &std::fs::File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN (PATH_MAX) bytes and a NUL into buf
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if r == -1 {
        return None;
    }
    let len = buf.iter().position(|b| *b == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn opened_path(_: &std::fs::File) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

// The Rust hub runs on Linux and macOS only. Elsewhere there's no file id to compare without
// another dependency, and a size and a time can match a different file, so nothing is served.
#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    false
}

fn respond(d: ViewDecision, head: bool, opened: Option<(std::fs::File, u64)>) -> Resp {
    let mut res = Resp::new(Body::empty());
    *res.status_mut() = StatusCode::from_u16(d.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (k, v) in &d.headers {
        if let Ok(v) = HeaderValue::from_str(v) {
            res.headers_mut().append(HeaderName::from_static(k), v);
        }
    }
    match opened {
        // the length of the file that was opened, which an atomic replace since the check doesn't change
        Some((file, size)) => {
            res.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(size));
            if !head {
                res.extensions_mut().insert(ViewFile { file: std::sync::Arc::new(std::sync::Mutex::new(Some(file))), size });
            }
        }
        None if !head && !d.body.is_empty() => *res.body_mut() = Body::from(d.body),
        None => {}
    }
    res
}

/// A 200 for a file another machine sent whole.
fn respond_bytes(d: ViewDecision, head: bool, bytes: Vec<u8>) -> Resp {
    let mut res = respond(ViewDecision { body: String::new(), ..d }, true, None);
    res.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    if !head {
        *res.body_mut() = Body::from(bytes);
    }
    res
}

/// An open file's bytes as a body, read with tokio::fs in 64 KB pieces, at most `size` of them. A
/// file cut shorter in place since it was opened ends the connection, as a failed stream does in
/// the Bun hub.
pub fn file_body(file: std::fs::File, size: u64) -> Body {
    use tokio::io::AsyncReadExt;
    let s = futures_util::stream::unfold((Some(tokio::fs::File::from_std(file)), size), |(f, left)| async move {
        let mut f = f?;
        if left == 0 {
            return None;
        }
        let mut buf = vec![0u8; left.min(64 * 1024) as usize];
        match f.read(&mut buf).await {
            Ok(0) => Some((Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the file got shorter")), (None, 0))),
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), (Some(f), left - n as u64)))
            }
            Err(e) => Some((Err(e), (None, 0))),
        }
    });
    Body::from_stream(s)
}

#[cfg(test)]
#[path = "pages_tests.rs"]
mod tests;
