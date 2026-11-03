//! Port of hosts/node-update.ts.
//!
//! Updating a machine's inyon from the app, one tap, never on its own. The machine runs its own
//! `inyon update --background`: one added to this hub takes the build the hub serves, a laptop
//! paired with the phone the newest release. It runs detached, since restarting inyon ends the
//! command that started it, checks the download, and puts the old copy back if the new one won't
//! start. Progress is read from its update-status file; success is the machine coming back with the
//! new version.
//!
//! Before the rename to Inyon the program was tether-node. A laptop paired with the phone ran
//! `tether-node update` the same way. A Windows machine added to a hub gets a script instead
//! (crates/inyon-hub/assets/hub-update.ps1), through the plain exec command every tether-node understands; once
//! the new version runs, it moves its services to inyon's own names.

use crate::re;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use inyon_protocol::{newer_release, parse_release, test_build, HostConfig, HostKind, HostState, HostStatus, NodeUpdate, NodeUpdateState, VERSION};

use super::host::{ExecOptions, Host};
use super::node_link;
use crate::clock::now_ms;
use crate::platform::rt::{self, set_timeout, Timer};

/// The first version with `tether-node update`; a phone-paired laptop before it reinstalls instead.
const LAST_WITHOUT_UPDATE: &str = "0.6.3";
/// The first version that's the inyon program, which updates itself from its hub too.
const FIRST_INYON: &str = "0.7.0";
fn is_inyon(version: &str) -> bool {
    !newer_release(FIRST_INYON, version)
}

/// Whether `target` is newer than a machine's `version`. The target is the newest release or this
/// hub's own version, which can be a test build ("0.19.0-rc.1"): that counts as the release it
/// leads to, so a hub on a test build still offers it to machines on the release before.
pub fn newer_target(target: &str, version: &str) -> bool {
    if !test_build(target) {
        return newer_release(target, version);
    }
    let base = |v: &str| re!(r"[-+].*$").replace(crate::js::JsStr::jtrim(v), "").into_owned();
    let rc = |v: &str| re!(r"-rc\.(\d+)$").captures(crate::js::JsStr::jtrim(v)).and_then(|m| m[1].parse::<f64>().ok()).unwrap_or(f64::NAN);
    if newer_release(&base(target), &base(version)) {
        return true;
    }
    // the same release: only an earlier test build of it is older
    let same = !newer_release(&base(version), &base(target)) && parse_release(&base(version)).is_some();
    same && test_build(crate::js::JsStr::jtrim(version)) && rc(target) > rc(version)
}

/// For a Windows machine still on tether-node: writes the script the hub sent and starts it as a
/// task of its own; the script's output goes to update.log.
pub const WINDOWS_LAUNCHER: &str = r#"d="$HOME/.tether-node"
mkdir -p "$d" && cat > "$d/update.ps1" && printf 'running\n' > "$d/update-status" || exit 1
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$d/update.ps1" -Launch < /dev/null"#;

/// The machine's own `inyon update`, started on its own.
pub const SELF_UPDATE: &str = r#""$INYON_EXE" ${INYON_SCRIPT:+"$INYON_SCRIPT"} update --background"#;
/// The same for a laptop paired with the phone that still runs tether-node.
pub const LINK_UPDATE: &str = r#""$TETHER_NODE_EXE" ${TETHER_NODE_SCRIPT:+"$TETHER_NODE_SCRIPT"} update --background"#;
/// inyon's folder, or tether-node's where it hasn't moved yet
pub const READ_STATUS: &str = r#"cat "$HOME/.inyon/node/update-status" 2>/dev/null || cat "$HOME/.tether-node/update-status" 2>/dev/null; true"#;

/// Fills in the checksum, version and architecture; refuses anything that isn't plainly one.
pub fn fill_update_script(script: &str, sha256: &str, version: &str, arch: &str) -> Result<String, String> {
    if !re!(r"^[0-9a-f]{64}$").is_match(sha256) {
        return Err("Bad checksum for the update script".into());
    }
    if !re!(r"^[0-9A-Za-z.+-]+$").is_match(version) {
        return Err("Bad version for the update script".into());
    }
    if arch != "x64" && arch != "arm64" {
        return Err("Bad architecture for the update script".into());
    }
    let out = script.replace("@@SHA256@@", sha256).replace("@@VERSION@@", version).replace("@@ARCH@@", arch);
    if out.contains("@@") {
        return Err("The update script has a placeholder left".into());
    }
    Ok(out)
}

/// Whether this machine's inyon (or tether-node) can be updated from the app.
pub fn can_update(c: &HostConfig, version: &str, platform: Option<&str>) -> bool {
    is_inyon(version) || if c.kind == HostKind::Node { platform.is_none_or(|p| p.is_empty() || p == "win32") } else { newer_release(version, LAST_WITHOUT_UPDATE) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub poll_ms: u64,
    pub timeout_ms: u64,
    pub keep_ms: u64,
    pub failed_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { poll_ms: 3000, timeout_ms: 15 * 60_000, keep_ms: 60_000, failed_ms: 30 * 60_000 }
    }
}

struct Job {
    state: NodeUpdateState,
    target: String,
    error: Option<String>,
    at: u64,
    timer: Option<Timer>,
}

type JobRef = Rc<RefCell<Job>>;
/// The hub's script for an old Windows machine (platform and arch as it reports them), checksum filled in.
pub type Scripts = Rc<dyn Fn(&str, &str) -> LocalBoxFuture<'static, Result<String, String>>>;

struct Inner {
    o: Options,
    jobs: HashMap<String, JobRef>,
    listeners: Vec<(u64, Rc<dyn Fn()>)>,
    next_listener: u64,
    script: Option<Scripts>,
    /// the newest release on the website, for phone-paired laptops
    latest_release: Option<String>,
}

/// The updates under way (`NodeUpdates`); clones share them.
#[derive(Clone)]
pub struct NodeUpdates(Rc<RefCell<Inner>>);

thread_local! {
    static NODE_UPDATES: NodeUpdates = NodeUpdates::new(Options::default());
}

/// The hub's own (`nodeUpdates`).
pub fn node_updates() -> NodeUpdates {
    NODE_UPDATES.with(Clone::clone)
}

fn first_line_or_last(stderr: &str, stdout: &str, code: i32) -> String {
    let text = if stderr.is_empty() { stdout } else { stderr };
    let last = crate::js::JsStr::jtrim(text).rsplit('\n').next().unwrap_or("").to_owned();
    if last.is_empty() {
        format!("The update didn't start (exit code {code}).")
    } else {
        last
    }
}

impl NodeUpdates {
    pub fn new(o: Options) -> Self {
        NodeUpdates(Rc::new(RefCell::new(Inner { o, jobs: HashMap::new(), listeners: vec![], next_listener: 0, script: None, latest_release: None })))
    }

    pub fn use_scripts(&self, f: impl Fn(&str, &str) -> LocalBoxFuture<'static, Result<String, String>> + 'static) {
        self.0.borrow_mut().script = Some(Rc::new(f));
    }

    /// Calls `f` on every change; the returned function stops that.
    pub fn on_change(&self, f: impl Fn() + 'static) -> impl Fn() {
        let id = {
            let mut i = self.0.borrow_mut();
            i.next_listener += 1;
            let id = i.next_listener;
            i.listeners.push((id, Rc::new(f)));
            id
        };
        let me = self.clone();
        move || me.0.borrow_mut().listeners.retain(|(i, _)| *i != id)
    }

    fn changed(&self) {
        let ls: Vec<Rc<dyn Fn()>> = self.0.borrow().listeners.iter().map(|(_, f)| f.clone()).collect();
        for f in ls {
            f();
        }
    }

    pub fn latest_release(&self) -> Option<String> {
        self.0.borrow().latest_release.clone()
    }

    pub fn set_latest_release(&self, v: Option<String>) {
        self.0.borrow_mut().latest_release = v;
    }

    /// The version a machine's inyon can update to.
    pub fn latest(&self, c: &HostConfig) -> Option<String> {
        match c.kind {
            HostKind::Node => Some(VERSION.to_owned()),
            HostKind::Link => self.latest_release(),
            _ => None,
        }
    }

    /// What the app shows for a machine: nothing while its inyon is current.
    pub fn view(&self, s: &HostState) -> Option<NodeUpdate> {
        let job = self.0.borrow().jobs.get(&s.config.id).cloned();
        if let Some(job) = job {
            let j = job.borrow();
            // updated by other means meanwhile: the old failure no longer matters
            let outdated = j.state == NodeUpdateState::Failed && s.node_version.as_deref().is_some_and(|v| !v.is_empty() && !newer_target(&j.target, v));
            if !outdated {
                return Some(NodeUpdate { latest: j.target.clone(), supported: true, state: Some(j.state), error: j.error.clone() });
            }
        }
        let latest = self.latest(&s.config)?;
        let version = s.node_version.as_deref().filter(|v| !v.is_empty())?;
        if !newer_target(&latest, version) {
            return None;
        }
        let os = node_link::info(&s.config.id).and_then(|i| i.os);
        let platform = os.as_deref().map(|o| o.split(' ').next().unwrap_or(""));
        Some(NodeUpdate { latest, supported: can_update(&s.config, version, platform), state: None, error: None })
    }

    /// Starts an update; says why not when it can't (the API answers 409 with that).
    pub fn start(&self, host: &Host, s: &HostState) -> Option<String> {
        let c = &s.config;
        if c.kind != HostKind::Node && c.kind != HostKind::Link {
            return Some("Only machines running inyon can be updated from here.".into());
        }
        if s.status != HostStatus::Online || !node_link::is_up(&c.id) {
            return Some(format!("{} isn't connected. Update it once it's back online.", c.name));
        }
        if self.0.borrow().jobs.get(&c.id).is_some_and(|j| j.borrow().state == NodeUpdateState::Updating) {
            return Some(format!("inyon on {} is already updating.", c.name));
        }
        let info = node_link::info(&c.id);
        let version = info.as_ref().and_then(|i| i.version.clone()).or_else(|| s.node_version.clone());
        let latest = self.latest(c);
        let os = info.and_then(|i| i.os).unwrap_or_default();
        let mut parts = os.split(' ');
        let platform = parts.next().unwrap_or("").to_owned();
        let arch = parts.next().unwrap_or("").to_owned();
        let Some(latest) = latest.filter(|l| !l.is_empty()) else { return Some("The newest inyon version isn't known yet. Try again in a minute.".into()) };
        let version = match version.filter(|v| !v.is_empty()) {
            Some(v) if newer_target(&latest, &v) => v,
            _ => return Some(format!("inyon on {} is already up to date.", c.name)),
        };
        if !can_update(c, &version, Some(&platform)) {
            return Some(if c.kind == HostKind::Node {
                format!("{} runs tether-node from before the rename to Inyon. Copy the \"Add a machine\" command again and run it there.", c.name)
            } else {
                format!("tether-node {version} can't update itself. Run its setup command on {} again.", c.name)
            });
        }
        let legacy = c.kind == HostKind::Node && !is_inyon(&version);
        if legacy && self.0.borrow().script.is_none() {
            return Some("This hub can't send updates to tether-node.".into());
        }
        let job = Rc::new(RefCell::new(Job { state: NodeUpdateState::Updating, target: latest, error: None, at: now_ms(), timer: None }));
        if let Some(old) = self.0.borrow_mut().jobs.insert(c.id.clone(), job.clone()) {
            rt::clear(&mut old.borrow_mut().timer);
        }
        self.changed();
        let me = self.clone();
        let (host, id) = (host.clone(), c.id.clone());
        rt::spawn(async move {
            if let Err(e) = me.run(&host, &id, &job, legacy, &platform, &arch, &version).await {
                me.finish(&id, &job, NodeUpdateState::Failed, Some(e));
            }
        });
        None
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(&self, host: &Host, id: &str, job: &JobRef, legacy: bool, platform: &str, arch: &str, version: &str) -> Result<(), String> {
        let input = if legacy {
            let script = self.0.borrow().script.clone().ok_or("This hub can't send updates to tether-node.")?;
            Some(script(platform, arch).await?)
        } else {
            None
        };
        let cmd = if legacy {
            WINDOWS_LAUNCHER
        } else if is_inyon(version) {
            SELF_UPDATE
        } else {
            LINK_UPDATE
        };
        if !self.is_current(id, job) {
            return Ok(());
        }
        let r = host.exec(cmd, ExecOptions { input: input.map(String::into_bytes), timeout_ms: Some(90_000) }).await.map_err(|e| e.message)?;
        if r.code != 0 {
            self.finish(id, job, NodeUpdateState::Failed, Some(first_line_or_last(&r.stderr, &r.stdout, r.code)));
            return Ok(());
        }
        self.watch(host.clone(), id.to_owned(), job.clone());
        Ok(())
    }

    fn is_current(&self, id: &str, job: &JobRef) -> bool {
        self.0.borrow().jobs.get(id).is_some_and(|j| Rc::ptr_eq(j, job))
    }

    /// Follows the update until the machine is back with the new version, it fails, or it takes too long.
    fn watch(&self, host: Host, id: String, job: JobRef) {
        let poll = self.0.borrow().o.poll_ms;
        let me = self.clone();
        let j = job.clone();
        let timer = set_timeout(poll, move || rt::spawn(async move { me.tick(host, id, j).await }));
        job.borrow_mut().timer = Some(timer);
    }

    async fn tick(&self, host: Host, id: String, job: JobRef) {
        if !self.is_current(&id, &job) || job.borrow().state != NodeUpdateState::Updating {
            return;
        }
        let up = node_link::is_up(&id);
        let version = if up { node_link::info(&id).and_then(|i| i.version) } else { None };
        let target = job.borrow().target.clone();
        if let Some(v) = version.filter(|v| !v.is_empty()) {
            if !newer_target(&target, &v) {
                return self.finish(&id, &job, NodeUpdateState::Updated, None);
            }
        }
        if up {
            let line = host.exec(READ_STATUS, ExecOptions::timeout(10_000)).await.map(|r| crate::js::JsStr::jtrim(r.stdout.as_str()).to_owned()).unwrap_or_default();
            if !self.is_current(&id, &job) {
                return;
            }
            if let Some(why) = line.strip_prefix("failed:") {
                return self.finish(&id, &job, NodeUpdateState::Failed, Some(crate::js::text::capitalize(crate::js::JsStr::jtrim(why))));
            }
            if line.starts_with("current") {
                return self.finish(&id, &job, NodeUpdateState::Failed, Some("inyon found no newer version to install.".into()));
            }
        }
        let (at, o) = (job.borrow().at, self.0.borrow().o);
        if now_ms().saturating_sub(at) > o.timeout_ms {
            return self.finish(&id, &job, NodeUpdateState::Failed, Some("The update is taking too long. See ~/.inyon/node/update.log on the machine.".into()));
        }
        self.watch(host, id, job);
    }

    fn finish(&self, id: &str, job: &JobRef, state: NodeUpdateState, error: Option<String>) {
        if !self.is_current(id, job) {
            return;
        }
        {
            let mut j = job.borrow_mut();
            rt::clear(&mut j.timer);
            j.state = state;
            j.error = error;
        }
        self.changed();
        // "updated" shows for a minute; a failure stays until the next try
        let o = self.0.borrow().o;
        let (me, idc, jb) = (self.clone(), id.to_owned(), job.clone());
        let timer = set_timeout(if state == NodeUpdateState::Updated { o.keep_ms } else { o.failed_ms }, move || {
            if !me.is_current(&idc, &jb) {
                return;
            }
            me.0.borrow_mut().jobs.remove(&idc);
            me.changed();
        });
        job.borrow_mut().timer = Some(timer);
    }

    /// The phone: looks up the newest release now and every `every_ms` (six hours). `get` is the
    /// platform's fetch of latest.json's version (`fetchLatestRelease`). Clear the returned timer
    /// to stop.
    pub fn watch_releases(&self, get: impl Fn() -> LocalBoxFuture<'static, Result<String, String>> + 'static, every_ms: u64) -> Rc<RefCell<Option<Timer>>> {
        let get: Rc<dyn Fn() -> LocalBoxFuture<'static, Result<String, String>>> = Rc::new(get);
        let slot: Rc<RefCell<Option<Timer>>> = Rc::default();
        fn check(me: NodeUpdates, get: Rc<dyn Fn() -> LocalBoxFuture<'static, Result<String, String>>>) {
            rt::spawn(async move {
                if let Ok(version) = get().await {
                    if Some(&version) != me.latest_release().as_ref() {
                        me.set_latest_release(Some(version));
                        me.changed();
                    }
                }
            });
        }
        fn every(me: NodeUpdates, get: Rc<dyn Fn() -> LocalBoxFuture<'static, Result<String, String>>>, ms: u64, slot: Rc<RefCell<Option<Timer>>>) {
            let s = slot.clone();
            let t = set_timeout(ms, move || {
                check(me.clone(), get.clone());
                every(me, get, ms, s);
            });
            *slot.borrow_mut() = Some(t);
        }
        check(self.clone(), get.clone());
        every(self.clone(), get, every_ms, slot.clone());
        slot
    }
}

/// `fetchLatestRelease`'s check of latest.json: its plain release number, or why not.
pub fn latest_release_from(status: u16, body: &str) -> Result<String, String> {
    use crate::js::json::Js;
    const SITE: &str = "inyon.tech";
    if !(200..300).contains(&status) {
        return Err(format!("{SITE} answered {status}"));
    }
    let v = crate::js::json::parse(body);
    // .trim().replace(/^v/, '')
    let version = v.as_ref().and_then(|v| {
        Some(v).at("version").str().map(|s| {
            let t = crate::js::JsStr::jtrim(s);
            t.strip_prefix('v').unwrap_or(t).to_owned()
        })
    });
    match version {
        Some(ver) if parse_release(&ver).is_some() => Ok(ver),
        _ => Err(format!("{SITE} didn't name a release")),
    }
}

#[cfg(test)]
#[path = "node_update_tests.rs"]
mod tests;
