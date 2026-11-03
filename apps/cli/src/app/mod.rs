//! The Inyon app on Windows: `inyon-app.exe`, a program without a console that owns everything
//! Inyon runs in the background there. It starts the link and the hub connection as its own
//! children (inside a Windows job, so they end with it, crash or not), shows the Inyon icon in the
//! notification area exactly while one of them runs, and opens the Inyon window. A second start
//! hands over to the one already running.
//!
//! What's here works the same everywhere and is tested on every system: the record of what the
//! app runs (~/.inyon/node/app.json), the requests the inyon command leaves for it (a folder it
//! reads twice a second, so a command over SSH reaches the app on the desktop too), when a job that
//! stopped is started again, and the words the tray and `inyon status` use. The Win32 side is in
//! [`win`].

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{bin_dir, node_dir};
use crate::service::Role;
use crate::sys::Proc;

#[cfg(windows)]
pub mod win;

/// The app's program name, next to inyon.exe in ~/.inyon/bin.
pub const APP: &str = "inyon-app";

/// What `inyon status` and the app's own messages say about stopping what it runs.
pub const STOP_HINT: &str = "Quit Inyon in the tray icon's menu, or run `inyon stop`, to stop it.";

/// ~/.inyon/bin/inyon-app.exe.
pub fn installed_app() -> PathBuf {
    bin_dir().join(format!("{APP}{}", crate::paths::exe_suffix()))
}

/// The app next to `program` (inyon.exe), where an install or an update puts it.
pub fn app_beside(program: &Path) -> PathBuf {
    let ext = program.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    program.with_file_name(format!("{APP}{ext}"))
}

/// Whether this program was started as the app: the windowless build, or a copy of inyon made
/// under its name.
pub fn started_as_app(argv0: &str) -> bool {
    let name = argv0.rsplit(['/', '\\']).next().unwrap_or(argv0).to_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name) == APP
}

/// inyon.exe for a program that's the app or the engine's copy: the command the app's children
/// and its own windowless steps run, whose output can be read.
pub fn command_beside(exe: &Path) -> PathBuf {
    exe.with_file_name(format!("{}{}", crate::paths::NAME, crate::paths::exe_suffix()))
}

/// The kite, as the tray, the window, the Start menu and Settings → Apps show it.
pub const ICON: &[u8] = include_bytes!("../../assets/inyon.ico");

/// Every build says which version it is in its bytes, so a file in ~/.inyon/bin can be matched to
/// this program without running it (`build_of`).
#[used]
static BUILD: &str = concat!("inyon-build:", env!("CARGO_PKG_VERSION"), ";");

/// This program's mark, kept in the file by its use here.
pub fn build_mark() -> &'static str {
    std::hint::black_box(BUILD)
}

/// The version a program file says it is: inyon, the app or the engine's copy from 0.20.3 on.
pub fn build_of(file: &Path) -> Option<String> {
    build_in(&fs::read(file).ok()?)
}

fn build_in(bytes: &[u8]) -> Option<String> {
    const MARK: &[u8] = b"inyon-build:";
    let mut from = 0;
    while let Some(i) = bytes[from..].windows(MARK.len()).position(|w| w == MARK) {
        let at = from + i + MARK.len();
        // the needle above is in the file too, followed by whatever the linker put next: only a
        // version ended by ';' is the mark
        if let Some(end) = bytes[at..].iter().take(64).position(|b| *b == b';') {
            let v = &bytes[at..at + end];
            if v.first().is_some_and(u8::is_ascii_digit) && v.iter().all(|b| b.is_ascii_alphanumeric() || b".-+".contains(b)) {
                return String::from_utf8(v.to_vec()).ok();
            }
        }
        from = at;
    }
    None
}

/// Runs the app (`inyon app`, or inyon-app.exe opened as it is). With no jobs named and a window
/// to show, it runs those set to run at sign-in.
pub fn run(sys: &crate::service::Sys, mut roles: Vec<Role>, background: bool, pair: bool, adopt: Option<(Role, u32)>) -> i32 {
    #[cfg(windows)]
    {
        if roles.is_empty() && adopt.is_none() {
            roles = crate::service::app_roles(sys);
        }
        win::run(win::Start { roles, background, pair, adopt })
    }
    #[cfg(not(windows))]
    {
        let _ = (sys, &mut roles, background, pair, adopt);
        eprintln!("The Inyon app is only on Windows. Here, `inyon` shows the same.");
        2
    }
}

/// `inyon window <role>` (an older tray icon's Open): the app's window, in the app.
pub fn open_window(role: Role, pair: bool) -> i32 {
    #[cfg(windows)]
    {
        let _ = role;
        win::run(win::Start { roles: Vec::new(), background: false, pair, adopt: None })
    }
    #[cfg(not(windows))]
    {
        let _ = (role, pair);
        eprintln!("The Inyon window is only on Windows.");
        2
    }
}

/// An engine started the way 0.20.2 and earlier did (its own logon task, then `inyon tray`):
/// the machine moves to the app (its file, the Start menu, Settings → Apps, the tasks), and the app
/// takes the engine over. None when that can't be done here, and the old tray icon shows instead.
pub fn take_over(sys: &crate::service::Sys, role: Role, pid: Option<u32>) -> Option<i32> {
    if sys.platform != crate::sys::Platform::Windows || role == Role::Hub {
        return None;
    }
    let pid = pid?;
    let app = match crate::service::adopt_app(sys) {
        Ok(a) => a,
        Err(e) => {
            trace(&format!("! couldn't move to the app: {e}"));
            return None;
        }
    };
    trace(&format!("moving the {role} (pid {pid}) to the app"));
    let args = vec![app.to_string_lossy().into_owned(), "app".into(), "--background".into(), "--adopt".into(), format!("{}:{pid}", role.name())];
    match crate::sys::spawn_detached(&args, &node_dir().join("app.log")) {
        Ok(()) => Some(0),
        Err(e) => {
            trace(&format!("! the app didn't start: {e}"));
            None
        }
    }
}

/// What `inyon link` or `inyon connect` says instead of starting a second copy, when the app runs
/// that job already. None otherwise (and always for the app's own engines).
pub fn already_runs(proc: &dyn Proc, command: &str) -> Option<String> {
    let role = match command {
        "link" => Role::Link,
        "connect" => Role::Node,
        _ => return None,
    };
    if crate::paths::env_var("INYON_APP").is_some() {
        return None;
    }
    let s = running(proc).filter(|s| s.runs(role))?;
    Some(already_message(role, s.pid))
}

fn already_message(role: Role, pid: u32) -> String {
    let what = if role == Role::Link { "phone link" } else { "hub connection" };
    format!("The Inyon app runs the {what} already (pid {pid}); `inyon logs -f` shows what it does. To run it here instead, quit Inyon in the tray icon's menu or run `inyon stop` first.")
}

/// Uninstall from Settings → Apps: asked and answered in message boxes, since the app has no
/// console. `yes` (QuietUninstallString) asks nothing.
pub fn uninstall_from_settings(sys: &crate::service::Sys, purge: bool, yes: bool) -> i32 {
    #[cfg(windows)]
    {
        if !yes && !win::confirm("Uninstall Inyon", "Remove Inyon from this computer? What it runs in the background stops, and its programs, pairings and logs are deleted.") {
            return 1;
        }
        let r = crate::commands::uninstall_all(sys, purge, false);
        let (ok, text) = match &r {
            Ok(v) => (true, v.get("message").and_then(serde_json::Value::as_str).unwrap_or("Inyon is uninstalled.").to_string()),
            Err(e) => (false, e.clone()),
        };
        if !yes {
            win::tell("Uninstall Inyon", &text, !ok);
        }
        i32::from(!ok)
    }
    #[cfg(not(windows))]
    {
        let _ = (sys, purge, yes);
        2
    }
}

// ---------- what the app runs ----------

/// One job the app runs, and its process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// app.json: written by the app while it runs, removed when it quits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppState {
    pub pid: u32,
    pub version: String,
    /// when it started and when it last wrote this (it does every few seconds), in ms
    pub started: f64,
    pub seen: f64,
    /// what runs, as the tray's status line names it: link, tunnel, hub connection, pairing
    #[serde(default)]
    pub running: Vec<String>,
    #[serde(default)]
    pub jobs: Vec<Job>,
}

/// The jobs app.json lists: each role once, whether the app runs it or adopted the engine an
/// older version started (an adopted job is wanted too), with the process doing it.
pub fn job_list(roles: impl IntoIterator<Item = Role>, pid_of: impl Fn(Role) -> Option<u32>) -> Vec<Job> {
    roles.into_iter().collect::<BTreeSet<Role>>().into_iter().map(|r| Job { role: r.name().to_string(), pid: pid_of(r) }).collect()
}

impl AppState {
    /// The process doing `role` for the app, if it runs one.
    pub fn job(&self, role: Role) -> Option<&Job> {
        self.jobs.iter().find(|j| j.role == role.name())
    }

    pub fn runs(&self, role: Role) -> bool {
        self.job(role).is_some()
    }
}

/// How old app.json may be and still count: the app writes it every few seconds.
const FRESH_MS: f64 = 20_000.0;

pub fn state_file() -> PathBuf {
    node_dir().join("app.json")
}

pub fn read_state() -> Option<AppState> {
    read_state_at(&state_file())
}

fn read_state_at(file: &Path) -> Option<AppState> {
    serde_json::from_str(&fs::read_to_string(file).ok()?).ok()
}

pub fn write_state(s: &AppState) {
    let _ = crate::paths::ensure_node_dir();
    write_state_at(&state_file(), s);
}

fn write_state_at(file: &Path, s: &AppState) {
    let tmp = file.with_extension("json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(s).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, file);
    }
}

/// Removes app.json, if it's this app's (`pid`).
pub fn clear_state(pid: u32) {
    clear_state_at(&state_file(), pid);
}

fn clear_state_at(file: &Path, pid: u32) {
    if read_state_at(file).is_some_and(|s| s.pid == pid) {
        let _ = fs::remove_file(file);
    }
}

/// Whether `s` says an app is running now: written lately by a process that's still there.
pub fn alive(s: &AppState, now: f64, proc: &dyn Proc) -> bool {
    now - s.seen < FRESH_MS && s.pid > 1 && proc.alive(s.pid)
}

/// The app that's running, if one is.
pub fn running(proc: &dyn Proc) -> Option<AppState> {
    running_at(&state_file(), proc)
}

fn running_at(file: &Path, proc: &dyn Proc) -> Option<AppState> {
    read_state_at(file).filter(|s| alive(s, crate::state::now_ms(), proc))
}

/// The version of the app's file on disk (inyon-app.exe next to the installed inyon), which an
/// update replaces while the old one keeps running. None before 0.20.3, whose files carry no mark.
pub fn app_on_disk() -> Option<String> {
    build_of(&app_beside(&crate::paths::installed_binary()))
}

/// Whether the app running `running` should start its new version: its file on disk is another
/// version, or, with no mark to read, this program is.
pub fn needs_relaunch(running: &str, on_disk: Option<&str>, mine: &str) -> bool {
    running != on_disk.unwrap_or(mine)
}

/// The running app, started again first when it's an older version than its file on disk (an
/// update replaced it), and whether it was. None when no app runs, or the new one didn't come up.
/// The file, not this program: an update's restarts run in the updater, the version it replaces.
pub fn current_relaunched(proc: &dyn Proc) -> Option<(AppState, bool)> {
    let s = running(proc)?;
    if !needs_relaunch(&s.version, app_on_disk().as_deref(), crate::ui::VERSION) {
        return Some((s, false));
    }
    let old = s.pid;
    let _ = ask(proc, Do::Relaunch, None, None, 5000);
    for _ in 0..60 {
        if let Some(n) = running(proc).filter(|n| n.pid != old && (n.jobs.iter().all(|j| j.pid.is_some()) || n.jobs.is_empty())) {
            return Some((n, true));
        }
        proc.sleep_ms(250);
    }
    None
}

/// Run in an engine the app started (Windows): when the app is older than this program and its
/// file on disk is this version, asks it to start its new version. An updater before 0.20.5 asked
/// the old app only to restart the job, so the old app kept running until the next sign-in. The
/// new app starts this job again with an engine of its own version, which then asks nothing.
pub fn relaunch_if_older(proc: &dyn Proc) {
    let Some(s) = running(proc) else { return };
    let on_disk = app_on_disk();
    if on_disk.as_deref() != Some(crate::ui::VERSION) || !needs_relaunch(&s.version, on_disk.as_deref(), crate::ui::VERSION) {
        return;
    }
    trace(&format!("the app runs {} and its file is {}: asking it to start the new one", s.version, crate::ui::VERSION));
    if send(&Request::new(Do::Relaunch, None, None)).is_ok() {
        #[cfg(windows)]
        win::wake();
    }
}

/// The running app, as [`current_relaunched`] leaves it.
pub fn current(proc: &dyn Proc) -> Option<AppState> {
    current_relaunched(proc).map(|(s, _)| s)
}

/// What `inyon status` says about the app: what it runs and how to stop it. None when it runs
/// nothing (or isn't running).
pub fn status_line(s: &AppState) -> Option<String> {
    if s.running.is_empty() {
        return None;
    }
    Some(format!("Running in the background: {}, by the Inyon app (pid {}). {STOP_HINT}", s.running.join(", "), s.pid))
}

/// The tray's first line: what runs, or that nothing does.
pub fn tray_line(running: &[String]) -> String {
    if running.is_empty() {
        "Nothing is running".to_string()
    } else {
        format!("Running: {}", running.join(", "))
    }
}

/// What runs, in the words the tray uses: the link (and its tunnel), the hub connection, and a
/// pairing that's still being made.
pub fn labels(link: bool, tunnel: bool, node: bool, pairing: bool) -> Vec<String> {
    let mut v = Vec::new();
    if link {
        v.push("link".to_string());
        if tunnel {
            v.push("tunnel".to_string());
        }
    }
    if node {
        v.push("hub connection".to_string());
    }
    if pairing {
        v.push("pairing".to_string());
    }
    v
}

/// What closing the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// nothing runs: the app ends with its window
    Quit,
    /// something runs: the window goes and the icon stays; `note` says so the first time
    ToTray { note: bool },
}

pub fn on_close(anything_running: bool, note_shown: bool) -> Close {
    if anything_running {
        Close::ToTray { note: !note_shown }
    } else {
        Close::Quit
    }
}

/// Whether the app should end: no window open or being made, and nothing to run (after it has had
/// a moment to start what it was asked to).
pub fn should_end(window_open: bool, opening: bool, anything: bool, settling: bool) -> bool {
    !window_open && !opening && !anything && !settling
}

/// The one-time note's marker: once it's shown, closing the window says nothing.
pub fn note_file() -> PathBuf {
    node_dir().join("app-close-note")
}

// ---------- requests ----------

/// What a command asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Do {
    /// run `role` from now on
    Start,
    /// stop `role` until it's asked for again (or the next sign-in)
    Stop,
    /// start `role` again (a new version, new settings)
    Restart,
    /// open the window, or bring it to the front
    Show,
    /// open the window with a pairing code
    Pair,
    /// stop everything and end
    Quit,
    /// start the app again from its file (an update replaced it)
    Relaunch,
    /// a job an older version started outside the app (`pid`): the app takes it over
    Adopt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(rename = "do")]
    pub what: Do,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// when it was asked, in ms: one nobody took within a minute is dropped
    pub at: f64,
}

impl Request {
    pub fn new(what: Do, role: Option<Role>, pid: Option<u32>) -> Request {
        let at = crate::state::now_ms();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Request { id: format!("{}-{}-{n}", at as u64, std::process::id()), what, role: role.map(|r| r.name().to_string()), pid, at }
    }

    pub fn role(&self) -> Option<Role> {
        self.role.as_deref().and_then(Role::parse)
    }
}

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    pub message: String,
}

/// How long a request may wait for the app.
const STALE_MS: f64 = 60_000.0;

pub fn requests_dir() -> PathBuf {
    node_dir().join("app-requests")
}

/// Leaves `r` for the app (written whole, then renamed, so the app never reads half of it).
pub fn send(r: &Request) -> std::io::Result<()> {
    send_in(&requests_dir(), r)
}

fn send_in(dir: &Path, r: &Request) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{}.tmp", r.id));
    fs::write(&tmp, serde_json::to_vec(r).unwrap_or_default())?;
    fs::rename(&tmp, dir.join(format!("{}.json", r.id)))
}

/// The app's answer to request `id`, once it's there (it's removed when read).
pub fn take_reply(id: &str) -> Option<Reply> {
    take_reply_in(&requests_dir(), id)
}

fn take_reply_in(dir: &Path, id: &str) -> Option<Reply> {
    let file = dir.join(format!("{id}.reply"));
    let r = serde_json::from_str(&fs::read_to_string(&file).ok()?).ok();
    let _ = fs::remove_file(&file);
    r
}

/// The requests waiting, oldest first; each is removed as it's read, and stale ones are dropped.
pub fn take_requests(now: f64) -> Vec<Request> {
    take_requests_in(&requests_dir(), now)
}

fn take_requests_in(dir: &Path, now: f64) -> Vec<Request> {
    let mut found: Vec<Request> = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.ends_with(".json") {
            let r = fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Request>(&t).ok());
            let _ = fs::remove_file(&p);
            if let Some(r) = r.filter(|r| now - r.at < STALE_MS) {
                found.push(r);
            }
        } else if name.ends_with(".reply") || name.ends_with(".tmp") {
            // nobody came for it
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_millis() as f64 > STALE_MS);
            if old {
                let _ = fs::remove_file(&p);
            }
        }
    }
    found.sort_by(|a, b| a.at.total_cmp(&b.at));
    found
}

/// The app's answer to `id`.
pub fn reply(id: &str, ok: bool, message: &str) {
    reply_in(&requests_dir(), id, ok, message);
}

fn reply_in(dir: &Path, id: &str, ok: bool, message: &str) {
    let _ = fs::create_dir_all(dir);
    let tmp = dir.join(format!("{id}.reply.tmp"));
    if fs::write(&tmp, serde_json::to_vec(&Reply { ok, message: message.to_string() }).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, dir.join(format!("{id}.reply")));
    }
}

/// Asks the running app to do `what` and waits up to `wait_ms` for its answer. An error when no
/// app runs, or it didn't answer in time.
pub fn ask(proc: &dyn Proc, what: Do, role: Option<Role>, pid: Option<u32>, wait_ms: u64) -> Result<Reply, String> {
    if running(proc).is_none() {
        return Err("The Inyon app isn't running.".into());
    }
    let r = Request::new(what, role, pid);
    send(&r).map_err(|e| format!("Couldn't reach the Inyon app ({e})."))?;
    #[cfg(windows)]
    win::wake();
    let mut waited = 0;
    loop {
        if let Some(answer) = take_reply(&r.id) {
            return Ok(answer);
        }
        if waited >= wait_ms {
            let _ = fs::remove_file(requests_dir().join(format!("{}.json", r.id)));
            return Err("The Inyon app didn't answer.".into());
        }
        proc.sleep_ms(100);
        waited += 100;
    }
}

// ---------- starting jobs again ----------

/// Which jobs the app runs and when one that stopped is started again: at once the first time,
/// then after 2, 4, 8… seconds up to a minute while it keeps stopping. A job that ran for a minute
/// starts counting from the beginning again.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Supervisor {
    wanted: BTreeSet<Role>,
    failures: BTreeMap<Role, u32>,
    retry_at: BTreeMap<Role, f64>,
}

/// A job that ran this long counts as having started well.
const STEADY_MS: f64 = 60_000.0;
const MAX_DELAY_MS: f64 = 60_000.0;

impl Supervisor {
    pub fn want(&mut self, role: Role) {
        if self.wanted.insert(role) {
            self.failures.remove(&role);
            self.retry_at.remove(&role);
        }
    }

    pub fn unwant(&mut self, role: Role) {
        self.wanted.remove(&role);
        self.failures.remove(&role);
        self.retry_at.remove(&role);
    }

    pub fn wanted(&self) -> Vec<Role> {
        self.wanted.iter().copied().collect()
    }

    pub fn wants(&self, role: Role) -> bool {
        self.wanted.contains(&role)
    }

    pub fn is_empty(&self) -> bool {
        self.wanted.is_empty()
    }

    /// Asked to start again now (Restart): no waiting.
    pub fn now(&mut self, role: Role) {
        self.retry_at.remove(&role);
    }

    /// `role`'s process ended after `ran_ms`; returns how long until it's started again.
    pub fn ended(&mut self, role: Role, now: f64, ran_ms: f64) -> f64 {
        if !self.wanted.contains(&role) {
            return 0.0;
        }
        let n = if ran_ms >= STEADY_MS { 0 } else { self.failures.get(&role).copied().unwrap_or(0) + 1 };
        self.failures.insert(role, n);
        let delay = if n == 0 { 0.0 } else { (1000.0 * 2f64.powi(n as i32)).min(MAX_DELAY_MS) };
        self.retry_at.insert(role, now + delay);
        delay
    }

    /// The wanted jobs to start now: not running, and not waiting out a delay.
    pub fn due(&self, now: f64, running: &dyn Fn(Role) -> bool) -> Vec<Role> {
        self.wanted.iter().copied().filter(|r| !running(*r) && self.retry_at.get(r).is_none_or(|t| now >= *t)).collect()
    }
}

/// How long a job an older version started keeps running after the app takes it over, so an
/// update from that version sees it come up and stay up before the app starts its own.
pub const ADOPT_GRACE_MS: f64 = 60_000.0;

/// Whether the app should replace an adopted job now: its grace is over and no update runs.
pub fn replace_adopted(since: f64, now: f64, update_running: bool) -> bool {
    now - since >= ADOPT_GRACE_MS && !update_running
}

/// Set in the app's process: the window asks the app for what it does instead of starting things
/// itself.
pub(crate) static IN_APP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn in_app() -> bool {
    IN_APP.load(std::sync::atomic::Ordering::Relaxed)
}

static PAIRINGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether a pairing code is being made or shown from the window right now.
pub fn pairing() -> bool {
    PAIRINGS.load(std::sync::atomic::Ordering::Relaxed) > 0
}

/// Counts a pairing as running while the guard lives: the icon shows it, and the app doesn't end
/// under it.
pub struct Pairing(());

impl Pairing {
    pub fn start() -> Pairing {
        PAIRINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Pairing(())
    }
}

impl Drop for Pairing {
    fn drop(&mut self) {
        PAIRINGS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A line in ~/.inyon/node/app.log, with the time.
pub fn trace(what: &str) {
    use std::io::Write;
    let dir = node_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("app.log")) {
        let _ = writeln!(f, "{} {what}", crate::state::format_time(crate::state::now_ms()));
    }
}

/// Puts a process this program started into the app's job, when this program is the app: a
/// pairing the window asked for ends with the app like everything else it runs.
pub fn hold(child: &std::process::Child) {
    #[cfg(windows)]
    win::hold(child);
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Procs(Vec<u32>);
    impl Proc for Procs {
        fn cmdline(&self, _pid: u32) -> Option<Vec<String>> {
            None
        }
        fn signal(&self, _pid: u32, _sig: crate::sys::Signal) {}
        fn alive(&self, pid: u32) -> bool {
            self.0.contains(&pid)
        }
        fn sleep_ms(&self, _ms: u64) {}
    }

    #[test]
    fn a_program_file_says_its_version() {
        // made at run time, so these samples aren't marks in this test program's own file
        let marked = |s: &str| s.replace("MARK", &["inyon", "build:"].join("-")).into_bytes();
        assert_eq!(build_in(&marked("..MZ..MARK0.20.3;...")), Some("0.20.3".into()));
        assert_eq!(build_in(b"no mark here"), None);
        assert_eq!(build_in(&marked("MARK0.20.3")), None, "cut off");
        assert_eq!(build_in(&marked("MARK\x01\x02 bytes;..MARK0.21.0-rc.1;")), Some("0.21.0-rc.1".into()), "the bare needle is passed over");
        // this program carries its own
        let _ = build_mark();
        let me = std::env::current_exe().unwrap();
        assert_eq!(build_of(&me).as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn an_update_relaunches_the_app_when_its_file_is_newer_than_what_runs() {
        // the updater is the old version: what counts is the file it put in place
        assert!(needs_relaunch("0.20.3", Some("0.20.5"), "0.20.3"));
        assert!(!needs_relaunch("0.20.5", Some("0.20.5"), "0.20.3"));
        // a file with no mark: this program decides, as before
        assert!(needs_relaunch("0.20.3", None, "0.20.5"));
        assert!(!needs_relaunch("0.20.5", None, "0.20.5"));
    }

    #[test]
    fn an_adopted_job_is_listed_once() {
        // the link adopted from 0.20.2 is also wanted, so it's run again if that engine stops
        let jobs = job_list([Role::Link, Role::Link, Role::Node], |r| (r == Role::Link).then_some(15028));
        assert_eq!(jobs, vec![Job { role: "link".into(), pid: Some(15028) }, Job { role: "node".into(), pid: None }]);
    }

    #[test]
    fn a_second_link_is_refused_with_how_to_stop_the_first() {
        let m = already_message(Role::Link, 812);
        assert!(m.starts_with("The Inyon app runs the phone link already (pid 812)"), "{m}");
        assert!(m.contains("`inyon stop`") && m.contains("tray icon"), "{m}");
        assert!(already_message(Role::Node, 1).contains("hub connection"));
        assert_eq!(already_runs(&Procs(vec![]), "pair"), None);
    }

    #[test]
    fn the_app_is_known_by_its_name() {
        assert!(started_as_app(r"C:\Users\a\.inyon\bin\inyon-app.exe"));
        assert!(started_as_app("Inyon-App.EXE"));
        assert!(!started_as_app(r"C:\Users\a\.inyon\bin\inyon.exe"));
        assert!(!started_as_app("inyon-engine.exe"));
        assert_eq!(app_beside(Path::new("C:/u/.inyon/bin/inyon.exe")), PathBuf::from("C:/u/.inyon/bin/inyon-app.exe"));
        assert_eq!(command_beside(Path::new("C:/u/.inyon/bin/inyon-app.exe")), PathBuf::from(format!("C:/u/.inyon/bin/inyon{}", crate::paths::exe_suffix())));
    }

    #[test]
    fn an_app_counts_while_its_record_is_fresh_and_its_process_runs() {
        let s = AppState { pid: 4242, version: "0.20.3".into(), started: 0.0, seen: 100_000.0, running: vec!["link".into()], jobs: vec![] };
        assert!(alive(&s, 105_000.0, &Procs(vec![4242])));
        // crashed: the record stays, the process is gone
        assert!(!alive(&s, 105_000.0, &Procs(vec![])));
        // a reused pid with a record nobody has written for a while
        assert!(!alive(&s, 100_000.0 + FRESH_MS + 1.0, &Procs(vec![4242])));
    }

    #[test]
    fn status_says_what_runs_who_runs_it_and_how_to_stop_it() {
        let mut s = AppState { pid: 77, running: labels(true, true, true, false), ..Default::default() };
        let line = status_line(&s).unwrap();
        assert_eq!(
            line,
            "Running in the background: link, tunnel, hub connection, by the Inyon app (pid 77). Quit Inyon in the tray icon's menu, or run `inyon stop`, to stop it."
        );
        s.running.clear();
        assert_eq!(status_line(&s), None);
        assert_eq!(tray_line(&labels(true, false, false, true)), "Running: link, pairing");
        assert_eq!(tray_line(&[]), "Nothing is running");
        // the tunnel belongs to the link
        assert_eq!(labels(false, true, false, false), Vec::<String>::new());
    }

    #[test]
    fn close_quits_when_nothing_runs_and_hides_to_the_tray_with_one_note_when_something_does() {
        assert_eq!(on_close(false, false), Close::Quit);
        assert_eq!(on_close(true, false), Close::ToTray { note: true });
        assert_eq!(on_close(true, true), Close::ToTray { note: false });
        assert!(should_end(false, false, false, false));
        assert!(!should_end(true, false, false, false), "the window keeps it open");
        // the app ended at start on a slow computer: WebView2 took past the 10 s settle to come up
        assert!(!should_end(false, true, false, false), "so does a window still being made");
        assert!(!should_end(false, false, true, false), "so does anything running");
        assert!(!should_end(false, false, false, true), "and the moment it's starting what it was asked to");
    }

    #[test]
    fn a_job_that_keeps_stopping_waits_longer_each_time_up_to_a_minute() {
        let mut s = Supervisor::default();
        let none = |_: Role| false;
        assert!(s.due(0.0, &none).is_empty());
        s.want(Role::Link);
        assert_eq!(s.due(0.0, &none), vec![Role::Link]);
        assert!(s.due(0.0, &|r| r == Role::Link).is_empty(), "running already");
        assert_eq!(s.ended(Role::Link, 1000.0, 500.0), 2000.0);
        assert!(s.due(2000.0, &none).is_empty());
        assert_eq!(s.due(3000.0, &none), vec![Role::Link]);
        assert_eq!(s.ended(Role::Link, 3000.0, 100.0), 4000.0);
        for _ in 0..10 {
            s.ended(Role::Link, 3000.0, 100.0);
        }
        assert_eq!(s.ended(Role::Link, 3000.0, 100.0), MAX_DELAY_MS);
        // a run that lasted counts from the start again
        assert_eq!(s.ended(Role::Link, 9000.0, STEADY_MS), 0.0);
        // Restart doesn't wait
        s.ended(Role::Link, 9000.0, 100.0);
        s.now(Role::Link);
        assert_eq!(s.due(9000.0, &none), vec![Role::Link]);
        // a job that was stopped on purpose isn't started again
        s.unwant(Role::Link);
        assert_eq!(s.ended(Role::Link, 9000.0, 100.0), 0.0);
        assert!(s.due(99_000.0, &none).is_empty());
        assert!(s.is_empty());
    }

    #[test]
    fn an_adopted_job_is_replaced_after_its_grace_and_never_while_an_update_runs() {
        assert!(!replace_adopted(0.0, ADOPT_GRACE_MS - 1.0, false));
        assert!(replace_adopted(0.0, ADOPT_GRACE_MS, false));
        assert!(!replace_adopted(0.0, ADOPT_GRACE_MS * 10.0, true));
    }

    #[test]
    fn requests_go_through_the_folder_and_come_back_answered() {
        let dir = std::env::temp_dir().join(format!("inyon-app-requests-{}-{}", std::process::id(), crate::state::now_ms()));
        let _ = fs::remove_dir_all(&dir);
        let now = crate::state::now_ms();
        let a = Request::new(Do::Start, Some(Role::Link), None);
        let mut b = Request::new(Do::Adopt, Some(Role::Node), Some(31));
        b.at = a.at + 1.0;
        send_in(&dir, &a).unwrap();
        send_in(&dir, &b).unwrap();
        let mut stale = Request::new(Do::Quit, None, None);
        stale.at = now - STALE_MS - 1.0;
        send_in(&dir, &stale).unwrap();
        let got = take_requests_in(&dir, now);
        assert_eq!(got.len(), 2, "the stale one is dropped: {got:?}");
        assert_eq!((got[0].what, got[0].role()), (Do::Start, Some(Role::Link)));
        assert_eq!((got[1].what, got[1].role(), got[1].pid), (Do::Adopt, Some(Role::Node), Some(31)));
        assert!(take_requests_in(&dir, now).is_empty(), "each is taken once");
        reply_in(&dir, &a.id, true, "started the link");
        assert_eq!(take_reply_in(&dir, &a.id), Some(Reply { ok: true, message: "started the link".into() }));
        assert_eq!(take_reply_in(&dir, &a.id), None);

        let file = dir.join("app.json");
        let me = AppState { pid: 9, version: "x".into(), started: now, seen: now, running: vec![], jobs: vec![Job { role: "link".into(), pid: Some(12) }] };
        write_state_at(&file, &me);
        assert_eq!(running_at(&file, &Procs(vec![9])).map(|s| s.runs(Role::Link)), Some(true));
        assert!(running_at(&file, &Procs(vec![])).is_none(), "its process is gone");
        clear_state_at(&file, 10);
        assert!(read_state_at(&file).is_some(), "only its own record is removed");
        clear_state_at(&file, 9);
        assert!(read_state_at(&file).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
