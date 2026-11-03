//! The hub's side of features/hub-update.ts: is there an update, and start one from Settings.
//!
//! Run as the inyon program, the hub looks for a new release on the website and `inyon update
//! --background` swaps it in and restarts the hub. A development build (`cargo run` in a checkout)
//! doesn't update itself: Settings says how to update the checkout instead. Nothing here runs by
//! itself: the hub only looks for a new release, at most every half hour, when the app asks.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures_util::future::{FutureExt, LocalBoxFuture, Shared};
use inyon_core::clock::now_ms;
use inyon_protocol::{newer_release, LATEST_URL, VERSION};
use serde_json::{json, Map, Value};

use crate::http::{json as answer_json, Req, Resp};
use crate::server::{raw, Routes};

/// at most this often the hub asks for new tags, unless Settings asks for a check
pub const CHECK_EVERY: u64 = 30 * 60_000;

pub type Exec = Rc<dyn Fn(Vec<String>, PathBuf) -> LocalBoxFuture<'static, (i32, String)>>;
pub type Get = Rc<dyn Fn(&str) -> LocalBoxFuture<'static, Result<(u16, String), String>>>;

/// The checkout this program was built in, when it's a development build (`cargo run`) and the
/// checkout is still there: then the hub runs "from source" (it hands out what cargo built, and
/// git and cargo update it). A release build is always the program, even when the folder it was
/// built in is still on disk.
pub fn checkout() -> Option<PathBuf> {
    checkout_at(Path::new(env!("CARGO_MANIFEST_DIR")), cfg!(debug_assertions))
}

fn checkout_at(manifest_dir: &Path, development: bool) -> Option<PathBuf> {
    if !development {
        return None;
    }
    let repo = manifest_dir.join("../..");
    let repo = std::fs::canonicalize(&repo).unwrap_or(repo);
    repo.join("apps/cli/Cargo.toml").is_file().then_some(repo)
}

/// PATH with ~/.local/bin and /usr/local/bin, which a systemd user unit's short PATH often lacks.
pub fn tool_path() -> String {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| inyon_engine::paths::home().to_string_lossy().into_owned());
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_string_lossy().into_owned())).unwrap_or_default();
    let mut seen: Vec<String> = vec![];
    let path = std::env::var("PATH").unwrap_or_default();
    for p in path.split(':').map(str::to_owned).chain([exe_dir, format!("{home}/.local/bin"), "/usr/local/bin".to_owned()]) {
        if !p.is_empty() && !seen.contains(&p) {
            seen.push(p);
        }
    }
    seen.join(":")
}

/// Runs a command with the tool PATH; its exit code and its output and errors together, trimmed.
pub fn exec() -> Exec {
    Rc::new(|cmd: Vec<String>, cwd: PathBuf| {
        Box::pin(async move {
            let Some((prog, args)) = cmd.split_first() else { return (127, String::new()) };
            let out = tokio::process::Command::new(prog).args(args).current_dir(&cwd).env("PATH", tool_path()).stdin(std::process::Stdio::null()).output().await;
            match out {
                Ok(o) => {
                    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                    (o.status.code().unwrap_or(1), text.trim().to_owned())
                }
                // not installed
                Err(e) => (127, e.to_string()),
            }
        })
    })
}

/// What Settings says on a hub built in a checkout, which updates with git and cargo.
pub fn from_source(repo: &Path) -> String {
    format!("This hub is a development build from {}. To update it, run git pull and cargo build there, then restart the hub.", repo.display())
}

/// The commit the hub runs: built from (the inyon program), or checked out (a checkout's build).
pub fn head_commit(repo: Option<&Path>) -> String {
    match repo {
        Some(repo) => std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(repo)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default(),
        None => option_env!("INYON_BUILD_COMMIT").map(str::to_owned).or_else(|| std::env::var("INYON_BUILD_COMMIT").ok()).unwrap_or_default(),
    }
}

/// What a hub needs to check for and start its update.
pub struct Options {
    /// the checkout, or None when run as the inyon program
    pub repo: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub version: String,
    pub commit: String,
    pub run: Exec,
    pub now: Rc<dyn Fn() -> u64>,
    /// the inyon program's update status file (see apps/cli/src/update.rs)
    pub status_file: PathBuf,
    /// the program whose `update --background` is run
    pub program: String,
    pub get: Get,
}

impl Options {
    pub fn new(repo: Option<PathBuf>, data_dir: PathBuf, commit: String) -> Self {
        Options {
            repo,
            data_dir,
            version: VERSION.to_owned(),
            commit,
            run: exec(),
            now: Rc::new(now_ms),
            status_file: inyon_engine::paths::node_dir().join("update-status"),
            // the inyon command owns updates
            program: inyon_engine::paths::inyon_command().or_else(|| std::env::current_exe().ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "inyon".into()),
            get: Rc::new(|url: &str| {
                let url = url.to_owned();
                Box::pin(async move {
                    let r = inyon_engine::http::fetch("GET", url, vec![], None, std::time::Duration::from_secs(20)).await?;
                    Ok((r.status, String::from_utf8_lossy(&r.body).into_owned()))
                })
            }),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Check {
    latest: Option<String>,
    can_update: bool,
    reason: Option<String>,
    checked_at: u64,
}

/// What `start` refused, with the status the route answers with.
#[derive(Debug, PartialEq)]
pub struct Refused(pub u16, pub String);

pub struct HubUpdater {
    o: Options,
    pub job_file: PathBuf,
    pub log_file: PathBuf,
    cache: RefCell<Option<Check>>,
    checking: RefCell<Option<Shared<LocalBoxFuture<'static, Check>>>>,
}

fn read_json(p: &Path) -> Option<Map<String, Value>> {
    std::fs::read_to_string(p).ok().and_then(|t| inyon_core::js::json::parse(&t)).and_then(|v| v.as_object().cloned())
}

fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)?;
    f.write_all(text.as_bytes())
}

/// Adds to what the job file says about the update started from Settings.
fn write_job(file: &Path, patch: Value) {
    let mut job = read_json(file).unwrap_or_else(|| {
        let mut m = Map::new();
        m.insert("started".into(), json!(now_ms()));
        m
    });
    if let Value::Object(p) = patch {
        for (k, v) in p {
            job.insert(k, v);
        }
    }
    let _ = write_private(file, &inyon_core::js::stringify(&Value::Object(job)));
}

impl HubUpdater {
    pub fn new(o: Options) -> Rc<Self> {
        let job_file = o.data_dir.join("hub-update.json");
        let log_file = o.status_file.parent().unwrap_or(Path::new(".")).join("update.log");
        Rc::new(HubUpdater { o, job_file, log_file, cache: RefCell::new(None), checking: RefCell::new(None) })
    }

    fn read_job(&self) -> Option<Map<String, Value>> {
        read_json(&self.job_file)
    }

    /// the newest release, from latest.json on the website
    async fn check_release(o: (Get, String, u64)) -> Check {
        let (get, version, now) = o;
        let latest = match get(LATEST_URL).await {
            Ok((status, body)) => inyon_core::hosts::node_update::latest_release_from(status, &body).ok().filter(|n| newer_release(n, &version)),
            Err(_) => None,
        };
        Check { latest, can_update: true, reason: None, checked_at: now }
    }

    fn check(self: &Rc<Self>) -> LocalBoxFuture<'static, Check> {
        let me = self.clone();
        Box::pin(async move {
            let now = (me.o.now)();
            let Some(repo) = me.o.repo.clone() else { return Self::check_release((me.o.get.clone(), me.o.version.clone(), now)).await };
            Check { latest: None, can_update: false, reason: Some(from_source(&repo)), checked_at: now }
        })
    }

    /// what an update from Settings is doing, or how the last one ended
    async fn job_state(&self) -> Map<String, Value> {
        let mut out = Map::new();
        let now = (self.o.now)();
        let started = |j: &Map<String, Value>| j.get("started").and_then(Value::as_f64).unwrap_or(f64::NAN);
        let s = |j: &Map<String, Value>, k: &str| j.get(k).filter(|v| crate::http::truthy(Some(v))).map(crate::http::js_string);
        let log = Value::String(self.log_file.to_string_lossy().into_owned());
        if self.o.repo.is_some() {
            return out;
        }
        let job = self.read_job();
        let line = std::fs::read_to_string(&self.o.status_file).map(|t| t.trim().to_owned()).unwrap_or_default();
        let Some(job) = job.filter(|j| !crate::http::truthy(j.get("done"))) else { return out };
        if let Some(e) = s(&job, "error") {
            out.insert("error".into(), json!(e));
            out.insert("log".into(), log);
        } else if let Some(why) = line.strip_prefix("failed:") {
            out.insert("error".into(), json!(why.trim()));
            out.insert("log".into(), log);
        } else if line == "running" || (now as f64 - started(&job)) < 10_000.0 {
            // `inyon update` restarts this hub when it's done; until then it says running
            out.insert("running".into(), json!(true));
        }
        out
    }

    pub async fn status(self: &Rc<Self>, refresh: bool) -> Value {
        let stale = self.cache.borrow().as_ref().is_none_or(|c| (self.o.now)().saturating_sub(c.checked_at) > CHECK_EVERY);
        if refresh || stale {
            let shared = {
                let mut checking = self.checking.borrow_mut();
                checking.get_or_insert_with(|| self.check().shared()).clone()
            };
            let c = shared.await;
            *self.checking.borrow_mut() = None;
            *self.cache.borrow_mut() = Some(c);
        }
        let c = self.cache.borrow().clone().unwrap_or_default();
        let mut m = Map::new();
        m.insert("current".into(), json!(self.o.version));
        if let Some(l) = &c.latest {
            m.insert("latest".into(), json!(l));
        }
        m.insert("canUpdate".into(), json!(c.can_update));
        if let Some(r) = &c.reason {
            m.insert("reason".into(), json!(r));
        }
        m.insert("checkedAt".into(), json!(c.checked_at));
        for (k, v) in self.job_state().await {
            m.insert(k, v);
        }
        Value::Object(m)
    }

    pub async fn start(self: &Rc<Self>) -> Result<(), Refused> {
        if self.job_state().await.contains_key("running") {
            return Err(Refused(409, "An update is already running.".into()));
        }
        if let Some(repo) = &self.o.repo {
            return Err(Refused(409, from_source(repo)));
        }
        let _ = std::fs::create_dir_all(&self.o.data_dir);
        let _ = write_private(&self.job_file, &inyon_core::js::stringify(&json!({ "started": (self.o.now)(), "step": "start" })));
        // runs on its own (see detach in apps/cli/src/service.rs), since it restarts this hub
        let r = (self.o.run)(vec![self.o.program.clone(), "update".into(), "--background".into()], self.o.data_dir.clone()).await;
        if r.0 != 0 {
            let why = if r.1.is_empty() { "Couldn't start the update.".to_owned() } else { r.1 };
            write_job(&self.job_file, json!({ "error": why }));
            return Err(Refused(500, why));
        }
        *self.cache.borrow_mut() = None;
        Ok(())
    }

    pub async fn post(self: &Rc<Self>) -> Resp {
        match self.start().await {
            Ok(()) => answer_json(202, &json!({ "started": true })),
            Err(Refused(status, why)) => answer_json(status, &json!({ "error": why })),
        }
    }

    /// The route, behind sign-in.
    pub fn add(self: &Rc<Self>, routes: &mut Routes) {
        let (a, b) = (self.clone(), self.clone());
        let get = raw(move |r: Req| {
            let u = a.clone();
            async move { answer_json(200, &u.status(r.query_has("refresh")).await) }
        });
        let post = raw(move |_r: Req| {
            let u = b.clone();
            async move { u.post().await }
        });
        crate::server::guarded_raw(routes, "/api/hub/update", vec![(axum::http::Method::GET, get), (axum::http::Method::POST, post)]);
    }
}

#[cfg(test)]
#[path = "hub_update_tests.rs"]
mod tests;
