//! "Can't connect?" (components/ConnectSheet.tsx, lib/connectCheck.ts, lib/connectReport.ts,
//! lib/connlog.ts, lib/redact.ts): the checks this app runs for the hub it came from and for a
//! machine the hub carries, each saying in one sentence what it found and what to do; the log of
//! when the hub's connection dropped and why; and the report "Copy report" puts on the clipboard,
//! with every secret taken out.
//!
//! The log is the web app's own (`inyon.connlog.v1` in this site's storage), so drops either app
//! saw show in both. A phone's kinds of target (a paired hub, a computer, Android's battery) come
//! with the phone build; a browser checks its own hub.

use serde::{Deserialize, Serialize};

// ---------- the checks ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Pass,
    Fail,
    Skip,
}

/// A button the sheet offers next to a step's advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAction {
    SignIn,
}

impl StepAction {
    pub fn label(self) -> &'static str {
        match self {
            StepAction::SignIn => "Sign in",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    /// while it runs: "Checking the hub answers…"
    pub doing: String,
    pub state: StepState,
    /// what it found, in a sentence
    pub say: Option<String>,
    /// what to do about it
    pub fix: Option<String>,
    /// the raw answer, shown only under Details
    pub detail: Option<String>,
    pub action: Option<StepAction>,
    /// a failure here means the steps after it can't tell anything
    pub blocks: bool,
}

/// What one step found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub state: StepState,
    pub say: String,
    pub fix: Option<String>,
    pub detail: Option<String>,
    pub action: Option<StepAction>,
}

impl Found {
    pub fn pass(say: impl Into<String>) -> Found {
        Found { state: StepState::Pass, say: say.into(), fix: None, detail: None, action: None }
    }

    pub fn skip(say: impl Into<String>) -> Found {
        Found { state: StepState::Skip, say: say.into(), fix: None, detail: None, action: None }
    }

    pub fn fail(say: impl Into<String>, fix: impl Into<String>) -> Found {
        Found { state: StepState::Fail, say: say.into(), fix: Some(fix.into()), detail: None, action: None }
    }

    pub fn detail(mut self, d: impl Into<String>) -> Found {
        self.detail = Some(d.into());
        self
    }

    fn action(mut self, a: StepAction) -> Found {
        self.action = Some(a);
        self
    }
}

/// What's being checked: the hub this page came from, or a machine it carries.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Hub { origin: String },
    Machine { id: String, name: String, origin: String },
}

impl Target {
    pub fn origin(&self) -> &str {
        match self {
            Target::Hub { origin } | Target::Machine { origin, .. } => origin,
        }
    }

    /// The hub's address without its scheme: what the sheet names a hub by.
    pub fn host(&self) -> String {
        host_of(self.origin())
    }

    pub fn name(&self) -> String {
        match self {
            Target::Hub { .. } => self.host(),
            Target::Machine { name, .. } => name.clone(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Target::Hub { .. } => "hub",
            Target::Machine { .. } => "machine",
        }
    }
}

/// How long a request or the live connection gets.
pub const TIMEOUT_MS: u32 = 8000;
pub const SKIPPED: &str = "Skipped until the step above passes.";

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest).to_string()
}

/// The host name without a port (and an IPv6 address with its brackets).
fn name_of(url: &str) -> String {
    let host = host_of(url);
    if host.starts_with('[') {
        return host.split_once(']').map_or(host.clone(), |(a, _)| format!("{a}]"));
    }
    host.rsplit_once(':').map_or(host.clone(), |(a, _)| a.to_string())
}

fn is_ip(name: &str) -> bool {
    name.starts_with('[') || name == "localhost" || name.parse::<std::net::Ipv4Addr>().is_ok()
}

/// An address plain http may go to: this computer, or a Tailscale or NetBird address (100.64/10 and
/// Tailscale's IPv6 range), as hubs.ts `plainHttpIp`.
pub fn plain_http_ip(address: &str) -> bool {
    let s = address.trim_start_matches('[').trim_end_matches(']');
    let s = s.split('%').next().unwrap_or(s);
    if let Ok(v4) = s.parse::<std::net::Ipv4Addr>() {
        let o = v4.octets();
        return o[0] == 127 || (o[0] == 100 && (64..=127).contains(&o[1]));
    }
    let Ok(v6) = s.parse::<std::net::Ipv6Addr>() else { return false };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return plain_http_ip(&v4.to_string());
    }
    let g = v6.segments();
    v6.is_loopback() || (g[0] == 0xfd7a && g[1] == 0x115c && g[2] == 0xa1e0)
}

const PLAIN_WHY: &str =
    "anyone on that network could read what's sent, or answer in the hub's place. Use the hub's https address, or its Tailscale or NetBird address (100.x.y.z).";

/// Why the app won't use plain http with an IP address, when it won't.
fn plain_refusal(url: &str) -> Option<String> {
    if !url.starts_with("http:") {
        return None;
    }
    let name = name_of(url);
    (name != "localhost" && is_ip(&name) && !plain_http_ip(&name)).then(|| format!("Inyon won't use plain http with {name}: {PLAIN_WHY}"))
}

/// The steps for a target, in the order they run.
pub fn plan(t: &Target) -> Vec<Step> {
    let name = name_of(t.origin());
    let step = |id, title, doing: String, blocks| Step { id, title, doing, state: StepState::Pending, say: None, fix: None, detail: None, action: None, blocks };
    let mut v = vec![
        step("network", "Network", "Checking this device is online…".into(), true),
        step("address", "Address", format!("Looking up {name}…"), true),
        step("answers", "Hub answers", "Checking the hub answers…".into(), true),
        step("host", "Right address", "Checking the hub answers to this address…".into(), true),
        step("signed-in", "Signed in", "Checking you are signed in…".into(), true),
        step("socket", "Live connection", "Opening the live connection…".into(), true),
    ];
    if let Target::Machine { name, .. } = t {
        v.push(step("machine", "Hub reaches it", format!("Checking the hub reaches {name}…"), false));
    }
    v
}

pub fn network(online: bool) -> Found {
    if online {
        Found::pass("This device is online.")
    } else {
        Found::fail("This device is offline.", "Turn on Wi-Fi or mobile data, then try again.")
    }
}

/// A browser can't look a name up, so only an IP address gets checked here.
pub fn address(t: &Target) -> Found {
    let url = t.origin();
    let name = name_of(url);
    if !is_ip(&name) {
        return Found::skip("A browser can't look names up. The next step covers it.");
    }
    match plain_refusal(url) {
        Some(why) => Found::fail(why, "Use the hub's https address, or its Tailscale or NetBird address."),
        None => Found::pass(format!("{name} is an IP address, so there's nothing to look up.")),
    }
}

/// How `GET /api/session` went: its status and whether it says this device is signed in, or the
/// error when nothing came back.
#[derive(Debug, Clone, PartialEq)]
pub enum Session {
    Answered { status: u16, authed: Option<bool> },
    TimedOut,
    Failed(String),
}

fn secs() -> String {
    let n = TIMEOUT_MS / 1000;
    format!("{n} second{}", if n == 1 { "" } else { "s" })
}

pub fn answers(t: &Target, s: &Session) -> Found {
    match s {
        Session::Answered { status, .. } => Found::pass("The hub answered.").detail(format!("GET /api/session answered {status}")),
        Session::TimedOut => Found::fail(
            format!("The hub didn't answer within {}.", secs()),
            "Check this device is on the hub's network or VPN, and that the hub is running (inyon service status on its machine).",
        )
        .detail("TimeoutError: The operation timed out."),
        Session::Failed(e) => {
            let https = if t.origin().starts_with("https:") { " On https, this device must also trust the hub's certificate." } else { "" };
            Found::fail(
                format!("Couldn't connect to {}.", t.host()),
                format!("Check the hub is running and the address is right.{https} What you see now may be a saved copy of the app."),
            )
            .detail(e.clone())
        }
    }
}

pub fn right_address(t: &Target, s: &Session) -> Found {
    let name = name_of(t.origin());
    if matches!(s, Session::Answered { status: 421, .. }) {
        return Found::fail(format!("The hub doesn't answer to {name}."), format!("Add {name} to INYON_ALLOWED_HOSTS on the hub, or use the address the hub shows."))
            .detail("HTTP 421");
    }
    Found::pass(format!("The hub answers to {name}."))
}

pub fn signed_in(s: &Session) -> Found {
    match s {
        Session::Answered { authed: Some(true), .. } => Found::pass("You're signed in."),
        Session::Answered { authed: Some(false), .. } => Found::fail("You are signed out of this hub.", "Sign in again.").action(StepAction::SignIn),
        Session::Answered { status, .. } => {
            Found::fail(format!("The hub answered {status} where it says who's signed in."), "Something in front of the hub (a proxy) may be answering for it.")
                .detail(format!("GET /api/session answered {status}"))
        }
        _ => Found::fail("The hub answered nothing where it says who's signed in.", "Something in front of the hub (a proxy) may be answering for it."),
    }
}

/// How opening the live connection went.
#[derive(Debug, Clone, PartialEq)]
pub enum Opened {
    Open,
    TimedOut,
    Closed { code: u16, reason: String },
}

pub fn socket(o: &Opened) -> Found {
    let proxy = "If a proxy sits in front of the hub, it has to pass WebSockets through (the Upgrade and Connection headers).";
    match o {
        Opened::Open => Found::pass("The live connection opens."),
        Opened::TimedOut => Found::fail(format!("The live connection didn't open within {}.", secs()), proxy),
        Opened::Closed { code: 4001, .. } => {
            Found::fail("The hub refused the live connection: this device is signed out.", "Sign in again.").action(StepAction::SignIn).detail("close 4001")
        }
        Opened::Closed { code, reason } => Found::fail(format!("The live connection was refused (code {code})."), proxy).detail(if reason.is_empty() {
            format!("close {code}")
        } else {
            format!("close {code} {reason}")
        }),
    }
}

/// Whether the hub reaches a machine, from what the hub says about it now.
pub fn machine(name: &str, h: Option<&inyon_protocol::HostState>) -> Found {
    use inyon_protocol::{HostKind, HostStatus};
    let Some(h) = h else { return Found::fail(format!("The hub can't reach {name}."), "It may have been removed from the hub.") };
    if h.status == HostStatus::Online {
        return Found::pass(format!("{name} is connected to the hub."));
    }
    let fix = match h.config.kind {
        HostKind::Ssh => format!("Check the hub's machine can ssh to {} without a password prompt.", h.config.target.as_deref().unwrap_or(name)),
        HostKind::Local => "Check tmux is installed on the hub's machine.".into(),
        _ => format!("On {name}, run inyon service status, and inyon logs --role node to see why it can't connect."),
    };
    let say = match &h.error {
        Some(e) => format!("The hub can't reach {name}: {e}"),
        None => format!("The hub can't reach {name}."),
    };
    let mut f = Found::fail(say, fix);
    f.detail = h.error.clone();
    f
}

/// The steps as a run goes: start one, record what it found, and a failure that blocks skips the
/// rest.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub steps: Vec<Step>,
    blocked: bool,
}

impl Run {
    pub fn new(t: &Target) -> Run {
        Run { steps: plan(t), blocked: false }
    }

    /// The next step to run, after skipping the ones a blocking failure made pointless.
    pub fn next(&mut self) -> Option<&'static str> {
        loop {
            let i = self.steps.iter().position(|s| s.state == StepState::Pending)?;
            if self.blocked {
                let s = &mut self.steps[i];
                s.state = StepState::Skip;
                s.say = Some(SKIPPED.into());
                continue;
            }
            self.steps[i].state = StepState::Running;
            return Some(self.steps[i].id);
        }
    }

    pub fn found(&mut self, id: &str, f: Found) {
        let Some(s) = self.steps.iter_mut().find(|s| s.id == id) else { return };
        if f.state == StepState::Fail && s.blocks {
            self.blocked = true;
        }
        s.state = f.state;
        s.say = Some(f.say);
        s.fix = f.fix;
        s.detail = f.detail;
        s.action = f.action;
    }

    pub fn done(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| !matches!(s.state, StepState::Pending | StepState::Running))
    }

    pub fn all_good(&self) -> bool {
        self.done() && self.steps.iter().all(|s| matches!(s.state, StepState::Pass | StepState::Skip))
    }
}

// ---------- the connection log ----------

pub const LOG_KEY: &str = "inyon.connlog.v1";
const MAX_SUBJECTS: usize = 40;
const MAX_DROPS: usize = 12;
const STORED_DROPS: usize = 8;
const DETAIL_CHARS: usize = 300;
/// The hub this page came from, as the log keeps it.
pub const SELF: &str = "self";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drop {
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
    pub cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, rename = "while")]
    pub during: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<f64>,
    #[serde(default)]
    pub drops: Vec<Drop>,
    /// unknown until this run of the app has seen it either way; never stored
    #[serde(skip)]
    pub up: Option<bool>,
}

fn clip(s: &str) -> String {
    let r = redact(s);
    r.chars().take(DETAIL_CHARS).collect()
}

fn last_at(s: &Subject) -> f64 {
    s.since.unwrap_or(0.0).max(s.drops.last().map_or(0.0, |d| d.at))
}

/// When each connection came up, dropped and why, as the web app keeps it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConnLog {
    pub subjects: Vec<Subject>,
}

impl ConnLog {
    /// The log as stored; a drop that was still open when the app closed is marked cut.
    pub fn load(text: Option<&str>) -> ConnLog {
        let list: Vec<Subject> = text.and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default();
        let subjects = list
            .into_iter()
            .map(|mut s| {
                for d in &mut s.drops {
                    if d.back.is_none() {
                        d.cut = true;
                    }
                }
                s
            })
            .collect();
        ConnLog { subjects }
    }

    pub fn save(&self) -> String {
        let mut list: Vec<Subject> = self.subjects.clone();
        list.sort_by(|a, b| last_at(b).total_cmp(&last_at(a)));
        list.truncate(MAX_SUBJECTS);
        for s in &mut list {
            let n = s.drops.len();
            s.drops.drain(..n.saturating_sub(STORED_DROPS));
        }
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
    }

    fn subject(&mut self, id: &str, kind: &str, name: &str) -> &mut Subject {
        if !self.subjects.iter().any(|s| s.id == id) {
            if self.subjects.len() >= MAX_SUBJECTS {
                if let Some((i, _)) = self.subjects.iter().enumerate().min_by(|a, b| last_at(a.1).total_cmp(&last_at(b.1))) {
                    self.subjects.remove(i);
                }
            }
            self.subjects.push(Subject { id: id.into(), kind: kind.into(), name: name.into(), since: None, drops: vec![], up: None });
        }
        let s = self.subjects.iter_mut().find(|s| s.id == id).expect("just added");
        s.kind = kind.into();
        s.name = name.into();
        s
    }

    /// It's connected (again); false when it already was.
    pub fn up(&mut self, id: &str, kind: &str, name: &str, at: f64) -> bool {
        let s = self.subject(id, kind, name);
        if s.up == Some(true) {
            return false;
        }
        if let Some(open) = s.drops.last_mut() {
            if open.back.is_none() && !open.cut {
                open.back = Some(at);
            }
        }
        s.up = Some(true);
        s.since = Some(at);
        true
    }

    /// It dropped, or couldn't connect. While it's already down this only counts another failed
    /// try and keeps what it said; a sign-out is kept as the cause, since it explains the rest.
    #[allow(clippy::too_many_arguments)]
    pub fn down(&mut self, id: &str, kind: &str, name: &str, cause: &str, code: Option<u16>, detail: Option<&str>, during: Vec<String>, at: f64) {
        let s = self.subject(id, kind, name);
        if s.up == Some(false) {
            if let Some(open) = s.drops.last_mut().filter(|d| d.back.is_none() && !d.cut) {
                open.tries = Some(open.tries.unwrap_or(0) + 1);
                if let Some(d) = detail.filter(|d| !d.is_empty()) {
                    open.last = Some(clip(d));
                }
                if cause == "signed-out" || cause == "refused" {
                    open.cause = cause.into();
                }
                return;
            }
        }
        s.up = Some(false);
        s.drops.push(Drop { at, back: None, cut: false, cause: cause.into(), code, detail: detail.filter(|d| !d.is_empty()).map(clip), during, tries: None, last: None });
        let n = s.drops.len();
        s.drops.drain(..n.saturating_sub(MAX_DROPS));
    }

    pub fn last_drop(&self, id: &str) -> Option<&Drop> {
        self.subjects.iter().find(|s| s.id == id)?.drops.last()
    }

    /// The latest drops of every subject, newest first.
    pub fn recent(&self, n: usize) -> Vec<(&Subject, &Drop)> {
        let mut v: Vec<(&Subject, &Drop)> = self.subjects.iter().flat_map(|s| s.drops.iter().map(move |d| (s, d))).collect();
        v.sort_by(|a, b| b.1.at.total_cmp(&a.1.at));
        v.truncate(n);
        v
    }
}

fn close_code(code: u16) -> Option<&'static str> {
    Some(match code {
        1000 => "the connection was closed",
        1001 => "the other side went away (a restart, most likely)",
        1006 => "the connection was cut without a goodbye",
        1011 => "the hub hit an error",
        1012 => "the hub restarted",
        1013 => "the hub was too busy",
        4001 => "the hub signed this device out",
        4408 => "it stopped answering pings",
        _ => return None,
    })
}

/// Why it dropped, in one plain phrase.
pub fn why(d: &Drop) -> String {
    let has = |c: &str| d.during.iter().any(|x| x == c);
    let bg = if has("background") { " while Inyon was in the background" } else { "" };
    match d.cause.as_str() {
        "signed-out" => return d.detail.clone().unwrap_or_else(|| "the hub signed this device out".into()),
        "refused" => return d.detail.clone().unwrap_or_else(|| "the hub turned the address away".into()),
        "hub-down" => return "its hub was out of reach".into(),
        "offline" => return d.detail.clone().unwrap_or_else(|| "the hub lost it".into()),
        _ => {}
    }
    if has("doze") {
        return "Android's Doze paused the phone".into();
    }
    if has("frozen") {
        return "the page was frozen in the background".into();
    }
    if has("offline") {
        return "the network went away".into();
    }
    if has("restricted") && !bg.is_empty() {
        return format!("Android's battery limits stopped Inyon{bg}");
    }
    match d.cause.as_str() {
        "handshake" => return format!("the computer answered but the handshake failed{}", d.detail.as_ref().map(|x| format!(": {x}")).unwrap_or_default()),
        "timeout" => return format!("nothing came back for a minute{bg}"),
        "unreachable" => return format!("nothing answered{}", d.detail.as_ref().map(|x| format!(" ({x})")).unwrap_or_default()),
        _ => {}
    }
    let said = match d.code {
        Some(c) => close_code(c).map_or_else(|| format!("it closed with code {c}"), str::to_string),
        None => "the connection closed".into(),
    };
    format!("{said}{bg}")
}

/// "4m 10s", "1h 5m".
pub fn span(ms: f64) -> String {
    let s = (ms / 1000.0).round().max(0.0) as u64;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return if !s.is_multiple_of(60) { format!("{m}m {}s", s % 60) } else { format!("{m}m") };
    }
    let h = m / 60;
    if !m.is_multiple_of(60) {
        format!("{h}h {}m", m % 60)
    } else {
        format!("{h}h")
    }
}

/// How long it was down: "4m 10s", "still down after …", or that the app closed first.
pub fn down_for(d: &Drop, now: f64) -> String {
    match d.back {
        Some(back) => span(back - d.at),
        None if d.cut => "until the app closed".into(),
        None => format!("still down after {}", span(now - d.at)),
    }
}

pub fn context_words(c: &[String]) -> String {
    c.iter()
        .map(|x| match x.as_str() {
            "offline" => "offline",
            "background" => "in the background",
            "frozen" => "page frozen",
            "doze" => "Doze",
            "restricted" => "battery limits",
            "saver" => "battery saver",
            "no-keepalive" => "Stay connected off",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------- the report ----------

/// What the report is built from: the app, the device, the checks and the last drops.
pub struct ReportInput<'a> {
    pub now: f64,
    pub version: &'a str,
    pub user_agent: Option<&'a str>,
    pub online: bool,
    pub hidden: bool,
    pub target: Option<&'a Target>,
    pub steps: &'a [Step],
    pub drops: Vec<(&'a Subject, &'a Drop)>,
}

/// "2026-10-31 18:02 UTC".
pub fn when(ms: f64) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

fn mark(s: StepState) -> &'static str {
    match s {
        StepState::Pass => "ok",
        StepState::Fail => "FAIL",
        StepState::Skip => "skip",
        StepState::Pending => "-",
        StepState::Running => "...",
    }
}

/// The plain-text report for support or an agent to read, with every secret taken out.
pub fn report(r: &ReportInput) -> String {
    let mut lines = vec![format!("Inyon connection report, {}", when(r.now))];
    lines.push(format!("App: Inyon {}", r.version));
    lines.push(format!("Platform: browser{}", r.user_agent.map(|u| format!(" · {u}")).unwrap_or_default()));
    lines.push(format!("Network: {} · page {}", if r.online { "online" } else { "offline" }, if r.hidden { "hidden" } else { "visible" }));
    if let Some(t) = r.target {
        let at = match t {
            Target::Hub { origin } => origin.clone(),
            // a machine is reached through its hub
            Target::Machine { .. } => t.host(),
        };
        lines.push(String::new());
        lines.push(format!("Checks for {} ({} at {at})", t.name(), t.kind()));
        for s in r.steps {
            let said = s.say.clone().unwrap_or_else(|| if s.state == StepState::Pending { "not run".into() } else { String::new() });
            lines.push(format!("  [{}] {}: {said}", mark(s.state), s.title));
            if let (Some(fix), StepState::Fail) = (&s.fix, s.state) {
                lines.push(format!("         Do: {fix}"));
            }
            if let Some(d) = &s.detail {
                for l in d.split('\n') {
                    lines.push(format!("         {l}"));
                }
            }
        }
    }
    lines.push(String::new());
    lines.push("Last drops".into());
    if r.drops.is_empty() {
        lines.push("  None recorded.".into());
    }
    for (subject, d) in &r.drops {
        let mut bits = vec![why(d), if d.back.is_some() || d.cut { format!("down {}", down_for(d, r.now)) } else { down_for(d, r.now) }];
        if let Some(c) = d.code {
            bits.push(format!("close code {c}"));
        }
        if !d.during.is_empty() {
            bits.push(format!("while {}", context_words(&d.during)));
        }
        if let Some(n) = d.tries.filter(|n| *n > 0) {
            bits.push(format!("{n} failed {}{}", if n == 1 { "try" } else { "tries" }, d.last.as_ref().map(|l| format!(", last: {l}")).unwrap_or_default()));
        }
        lines.push(format!("  {}  {} ({}): {}", when(d.at), subject.name, subject.kind, bits.join("; ")));
    }
    redact(&lines.join("\n"))
}

// ---------- secrets out ----------

const HIDDEN: &str = "[hidden]";

fn ci_starts(s: &str, at: usize, word: &str) -> bool {
    s.len() >= at + word.len() && s.is_char_boundary(at) && s.is_char_boundary(at + word.len()) && s[at..at + word.len()].eq_ignore_ascii_case(word)
}

fn word_start(s: &str, at: usize) -> bool {
    at == 0 || !s[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Applies `rule` at every position from the left: it returns how many bytes it matched and what
/// to put in their place, or nothing to keep the character and go on.
fn pass(s: &str, rule: impl Fn(&str, usize) -> Option<(usize, String)>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some((n, to)) = rule(s, i) {
            out.push_str(&to);
            i += n.max(1);
            while !s.is_char_boundary(i) {
                i += 1;
            }
            continue;
        }
        let c = s[i..].chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn run_while(s: &str, from: usize, ok: impl Fn(char) -> bool) -> usize {
    s[from..].char_indices().find(|(_, c)| !ok(*c)).map_or(s.len(), |(j, _)| from + j)
}

/// Takes secrets out of text that leaves the device: pairing links, a URL's query and fragment,
/// bearer tokens and cookies, secret-sounding keys and their values, hub pairing codes, private
/// keys and anything long and random enough to be a token. Addresses, words and paths stay.
pub fn redact(text: &str) -> String {
    let secret_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/');
    // pairing links carry the code and the key: inyon://pair?…, inyon://hub?…, tether://pair?…
    let s = pass(text, |s, i| {
        if !word_start(s, i) {
            return None;
        }
        for scheme in ["inyon", "tether"] {
            for kind in ["pair", "hub"] {
                let head = format!("{scheme}://{kind}?");
                if ci_starts(s, i, &head) {
                    let end = run_while(s, i + head.len(), |c| !c.is_whitespace());
                    return Some((end - i, format!("{}{HIDDEN}", &s[i..i + head.len()])));
                }
            }
        }
        None
    });
    // a URL's query and fragment (a WebSocket ticket, a page token, a #pair? code)
    let s = pass(&s, |s, i| {
        if !word_start(s, i) {
            return None;
        }
        let scheme = ["https://", "http://", "wss://", "ws://"].into_iter().find(|p| ci_starts(s, i, p))?;
        let stop = |c: char| c.is_whitespace() || matches!(c, '?' | '#' | '"' | '\'' | '<' | '>');
        let base_end = run_while(s, i + scheme.len(), |c| !stop(c));
        if base_end == i + scheme.len() || !s[base_end..].starts_with(['?', '#']) {
            return None;
        }
        let end = run_while(s, base_end + 1, |c| !(c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>')));
        Some((end - i, format!("{}?{HIDDEN}", &s[i..base_end])))
    });
    // bearer tokens, and whole cookie and authorization headers
    let s = pass(&s, |s, i| {
        if !word_start(s, i) {
            return None;
        }
        if ci_starts(s, i, "bearer") {
            let sp = run_while(s, i + 6, char::is_whitespace);
            if sp > i + 6 {
                let end = run_while(s, sp, |c| !(c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';')));
                if end > sp {
                    return Some((end - i, format!("{} {HIDDEN}", &s[i..i + 6])));
                }
            }
        }
        for k in ["set-cookie", "cookie", "authorization"] {
            if ci_starts(s, i, k) && !s[i + k.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                let mut j = i + k.len();
                if s[j..].starts_with(['"', '\'']) {
                    j += 1;
                }
                j = run_while(s, j, |c| c == ' ' || c == '\t');
                if s[j..].starts_with([':', '=']) {
                    let sep_end = run_while(s, j + 1, |c| c == ' ' || c == '\t');
                    let end = s[sep_end..].find('\n').map_or(s.len(), |n| sep_end + n);
                    return Some((end - i, format!("{}{HIDDEN}", &s[i..sep_end])));
                }
            }
        }
        for k in ["__host-inyon_session", "__host-tether_session", "inyon_session", "tether_session"] {
            if ci_starts(s, i, k) && s[i + k.len()..].starts_with('=') {
                let end = run_while(s, i + k.len() + 1, |c| !(c.is_whitespace() || matches!(c, ';' | ',' | '"' | '\'')));
                if end > i + k.len() + 1 {
                    return Some((end - i, format!("{}={HIDDEN}", &s[i..i + k.len()])));
                }
            }
        }
        None
    });
    // key=value and "key": "value" for anything that sounds secret (a quoted value goes whole)
    let s = pass(&s, |s, i| {
        if !word_start(s, i) {
            return None;
        }
        const KEYS: [&str; 12] = ["token", "ticket", "password", "passwd", "secret", "code", "key", "cookie", "session", "auth", "seed", "passphrase"];
        let k = KEYS.iter().find(|k| ci_starts(s, i, k))?;
        let key_end = run_while(s, i + k.len(), |c| c.is_ascii_alphabetic() || c == '_' || c == '-');
        let mut j = key_end;
        if s[j..].starts_with(['"', '\'']) {
            j += 1;
        }
        j = run_while(s, j, char::is_whitespace);
        if !s[j..].starts_with([':', '=']) {
            return None;
        }
        let v = run_while(s, j + 1, char::is_whitespace);
        let end = match s[v..].chars().next() {
            Some(q @ ('"' | '\'')) => s[v + 1..].find([q, '\n']).map_or(s.len(), |n| if s[v + 1 + n..].starts_with(q) { v + 2 + n } else { v + 1 + n }),
            Some(_) => run_while(s, v, |c| !(c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '&' | '}'))),
            None => return None,
        };
        (end > v).then(|| (end - i, format!("{}{HIDDEN}", &s[i..v])))
    });
    // a hub pairing code as typed or shown: K7Q4-MX2T 493812, K7Q4MX2T:493812
    let s = pass(&s, |s, i| {
        if !word_start(s, i) {
            return None;
        }
        let b = s.as_bytes();
        let up = |j: usize| b.get(j).is_some_and(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
        if !(0..4).all(|k| up(i + k)) {
            return None;
        }
        let mut j = i + 4;
        if b.get(j) == Some(&b'-') {
            j += 1;
        }
        if !(0..4).all(|k| up(j + k)) {
            return None;
        }
        j += 4;
        if !matches!(b.get(j), Some(b':' | b' ')) {
            return None;
        }
        j += 1;
        if !(0..6).all(|k| b.get(j + k).is_some_and(u8::is_ascii_digit)) {
            return None;
        }
        j += 6;
        let ends = b.get(j).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'));
        ends.then(|| (j - i, HIDDEN.to_string()))
    });
    // a private key block
    let s = pass(&s, |s, i| {
        let rest = &s[i..];
        let head = rest.strip_prefix("-----BEGIN ")?;
        let line = head.find("-----")?;
        if !head[..line].ends_with("PRIVATE KEY") {
            return None;
        }
        let after = 11 + line + 5;
        let end = rest[after..].find("-----END ").and_then(|e| rest[after + e + 9..].find("-----").map(|f| after + e + 9 + f + 5)).unwrap_or(rest.len());
        Some((end, HIDDEN.to_string()))
    });
    // anything long and random enough to be a token or a key (letters and digits mixed, so a long
    // word or a path isn't one)
    pass(&s, |s, i| {
        if s[..i].chars().next_back().is_some_and(secret_char) {
            return None;
        }
        let end = run_while(s, i, secret_char);
        let run = &s[i..end];
        if run.len() < 24 || !run.contains(|c: char| c.is_ascii_digit()) || !run.contains(|c: char| c.is_ascii_alphabetic()) {
            return None;
        }
        let pad = run_while(s, end, |c| c == '=');
        Some(((end + (pad - end).min(2)) - i, HIDDEN.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hub() -> Target {
        Target::Hub { origin: "https://hub.example:4777".into() }
    }

    #[test]
    fn a_hub_that_works_passes_every_step_one_after_another() {
        let t = hub();
        let mut run = Run::new(&t);
        let ok = Session::Answered { status: 200, authed: Some(true) };
        let mut order = vec![];
        while let Some(id) = run.next() {
            order.push(id);
            let f = match id {
                "network" => network(true),
                "address" => address(&t),
                "answers" => answers(&t, &ok),
                "host" => right_address(&t, &ok),
                "signed-in" => signed_in(&ok),
                "socket" => socket(&Opened::Open),
                _ => unreachable!(),
            };
            run.found(id, f);
        }
        assert_eq!(order, ["network", "address", "answers", "host", "signed-in", "socket"]);
        assert!(run.all_good());
        assert_eq!(run.steps[1].say.as_deref(), Some("A browser can't look names up. The next step covers it."));
        assert_eq!(run.steps[2].detail.as_deref(), Some("GET /api/session answered 200"));
    }

    #[test]
    fn offline_skips_the_rest() {
        let t = hub();
        let mut run = Run::new(&t);
        assert_eq!(run.next(), Some("network"));
        run.found("network", network(false));
        assert_eq!(run.next(), None);
        assert!(run.done());
        assert!(!run.all_good());
        assert_eq!(run.steps[0].fix.as_deref(), Some("Turn on Wi-Fi or mobile data, then try again."));
        assert!(run.steps[1..].iter().all(|s| s.state == StepState::Skip && s.say.as_deref() == Some(SKIPPED)));
    }

    #[test]
    fn plain_http_to_a_lan_address_is_refused_at_the_address_step() {
        let lan = Target::Hub { origin: "http://192.168.1.20:4777".into() };
        let f = address(&lan);
        assert_eq!(f.state, StepState::Fail);
        assert!(f.say.starts_with("Inyon won't use plain http with 192.168.1.20:"), "{}", f.say);
        assert_eq!(address(&Target::Hub { origin: "http://100.97.188.112:4777".into() }).state, StepState::Pass);
        assert_eq!(address(&Target::Hub { origin: "http://127.0.0.1:4777".into() }).say, "127.0.0.1 is an IP address, so there's nothing to look up.");
        assert!(plain_http_ip("[fd7a:115c:a1e0::1]"));
        assert!(plain_http_ip("::ffff:100.64.0.1"));
        assert!(plain_http_ip("::1"));
        assert!(!plain_http_ip("10.0.0.1"));
        assert!(!plain_http_ip("fd91:8fa2::1"));
    }

    #[test]
    fn the_hub_not_answering_says_what_to_check() {
        let t = hub();
        let f = answers(&t, &Session::TimedOut);
        assert_eq!(f.say, "The hub didn't answer within 8 seconds.");
        let f = answers(&t, &Session::Failed("TypeError: Failed to fetch".into()));
        assert_eq!(f.say, "Couldn't connect to hub.example:4777.");
        assert_eq!(
            f.fix.as_deref(),
            Some(
                "Check the hub is running and the address is right. On https, this device must also trust the hub's certificate. What you see now may be a saved copy of the app."
            )
        );
        assert_eq!(f.detail.as_deref(), Some("TypeError: Failed to fetch"));
    }

    #[test]
    fn the_wrong_host_and_a_signed_out_page() {
        let t = hub();
        let wrong = Session::Answered { status: 421, authed: None };
        let f = right_address(&t, &wrong);
        assert_eq!((f.say.as_str(), f.detail.as_deref()), ("The hub doesn't answer to hub.example.", Some("HTTP 421")));
        let out = signed_in(&Session::Answered { status: 200, authed: Some(false) });
        assert_eq!((out.say.as_str(), out.action), ("You are signed out of this hub.", Some(StepAction::SignIn)));
        let proxy = signed_in(&Session::Answered { status: 502, authed: None });
        assert_eq!(proxy.say, "The hub answered 502 where it says who's signed in.");
    }

    #[test]
    fn the_live_connection_refused_signed_out_or_silent() {
        assert_eq!(socket(&Opened::TimedOut).say, "The live connection didn't open within 8 seconds.");
        let out = socket(&Opened::Closed { code: 4001, reason: String::new() });
        assert_eq!((out.action, out.detail.as_deref()), (Some(StepAction::SignIn), Some("close 4001")));
        let other = socket(&Opened::Closed { code: 1006, reason: "bye".into() });
        assert_eq!((other.say.as_str(), other.detail.as_deref()), ("The live connection was refused (code 1006).", Some("close 1006 bye")));
    }

    #[test]
    fn a_machine_adds_whether_the_hub_reaches_it() {
        let t = Target::Machine { id: "srv".into(), name: "srv".into(), origin: "https://hub.example".into() };
        assert_eq!(plan(&t).last().map(|s| s.id), Some("machine"));
        let h: inyon_protocol::HostState =
            serde_json::from_value(json!({ "config": { "id": "srv", "name": "srv", "kind": "ssh", "target": "deploy@srv" }, "status": "error", "error": "Permission denied",
            "integration": "unknown", "tmux": "ok", "agents": [], "sessions": [], "updatedAt": 1 }))
            .unwrap();
        let f = machine("srv", Some(&h));
        assert_eq!(f.say, "The hub can't reach srv: Permission denied");
        assert_eq!(f.fix.as_deref(), Some("Check the hub's machine can ssh to deploy@srv without a password prompt."));
        // a failed hub check skips it too, but the step doesn't block anything after it
        let mut run = Run::new(&t);
        run.next();
        run.found("network", network(false));
        while run.next().is_some() {}
        assert_eq!(run.steps.last().unwrap().state, StepState::Skip);
    }

    fn drop_at(at: f64) -> Drop {
        Drop { at, back: None, cut: false, cause: "closed".into(), code: Some(1006), detail: None, during: vec![], tries: None, last: None }
    }

    #[test]
    fn the_log_counts_failed_tries_and_closes_a_drop_when_it_comes_back() {
        let mut log = ConnLog::default();
        assert!(log.up(SELF, "hub", "hub.example", 1000.0));
        assert!(!log.up(SELF, "hub", "hub.example", 1500.0));
        log.down(SELF, "hub", "hub.example", "closed", Some(1006), None, vec!["background".into()], 2000.0);
        log.down(SELF, "hub", "hub.example", "closed", Some(1006), Some("refused ticket=abc123secretvalue"), vec![], 3000.0);
        log.down(SELF, "hub", "hub.example", "signed-out", Some(4001), None, vec![], 4000.0);
        let d = log.last_drop(SELF).unwrap();
        assert_eq!((d.tries, d.cause.as_str(), d.last.as_deref()), (Some(2), "signed-out", Some("refused ticket=[hidden]")));
        assert!(log.up(SELF, "hub", "hub.example", 62_000.0));
        assert_eq!(down_for(log.last_drop(SELF).unwrap(), 0.0), "1m");
        // stored and read again: an open drop from a closed app is cut
        log.down(SELF, "hub", "hub.example", "timeout", None, None, vec![], 70_000.0);
        let again = ConnLog::load(Some(&log.save()));
        let d = again.last_drop(SELF).unwrap();
        assert!(d.cut);
        assert_eq!(down_for(d, 0.0), "until the app closed");
        assert_eq!(why(d), "nothing came back for a minute");
        // the web app's own entries read too
        let web = ConnLog::load(Some(r#"[{"id":"self","kind":"hub","name":"box:4777","since":5,"drops":[{"at":1,"back":3,"cause":"closed","code":1012,"while":["offline"]}]}]"#));
        assert_eq!(why(web.last_drop(SELF).unwrap()), "the network went away");
    }

    #[test]
    fn drops_say_why_in_words() {
        let mut d = drop_at(0.0);
        assert_eq!(why(&d), "the connection was cut without a goodbye");
        d.during = vec!["background".into()];
        assert_eq!(why(&d), "the connection was cut without a goodbye while Inyon was in the background");
        d.code = Some(4999);
        assert_eq!(why(&d), "it closed with code 4999 while Inyon was in the background");
        assert_eq!(span(4_000.0), "4s");
        assert_eq!(span(250_000.0), "4m 10s");
        assert_eq!(span(3_900_000.0), "1h 5m");
        assert_eq!(down_for(&drop_at(0.0), 30_000.0), "still down after 30s");
        assert_eq!(context_words(&["doze".into(), "background".into()]), "Doze, in the background");
    }

    #[test]
    fn the_report_says_what_runs_what_the_checks_found_and_the_last_drops() {
        let t = hub();
        let mut run = Run::new(&t);
        run.next();
        run.found("network", network(true));
        run.next();
        run.found("address", address(&t));
        run.next();
        run.found("answers", answers(&t, &Session::Failed("TypeError: Failed to fetch".into())));
        while run.next().is_some() {}
        let log = ConnLog::load(Some(
            r#"[{"id":"self","kind":"hub","name":"hub.example:4777","drops":[{"at":1793448000000,"back":1793448250000,"cause":"closed","code":1012,"while":[],"tries":2,"last":"no answer"}]}]"#,
        ));
        let text = report(&ReportInput {
            now: 1_793_448_300_000.0,
            version: "0.23.2",
            user_agent: Some("Mozilla/5.0"),
            online: true,
            hidden: false,
            target: Some(&t),
            steps: &run.steps,
            drops: log.recent(10),
        });
        let want = "Inyon connection report, 2026-10-31 12:05 UTC
App: Inyon 0.23.2
Platform: browser · Mozilla/5.0
Network: online · page visible

Checks for hub.example:4777 (hub at https://hub.example:4777)
  [ok] Network: This device is online.
  [skip] Address: A browser can't look names up. The next step covers it.
  [FAIL] Hub answers: Couldn't connect to hub.example:4777.
         Do: Check the hub is running and the address is right. On https, this device must also trust the hub's certificate. What you see now may be a saved copy of the app.
         TypeError: Failed to fetch
  [skip] Right address: Skipped until the step above passes.
  [skip] Signed in: Skipped until the step above passes.
  [skip] Live connection: Skipped until the step above passes.

Last drops
  2026-10-31 12:00 UTC  hub.example:4777 (hub): the hub restarted; down 4m 10s; close code 1012; 2 failed tries, last: no answer";
        assert_eq!(text, want);
        let none = report(&ReportInput { now: 0.0, version: "1", user_agent: None, online: false, hidden: true, target: None, steps: &[], drops: vec![] });
        assert!(none.ends_with("Last drops\n  None recorded."), "{none}");
        assert!(none.contains("Network: offline · page hidden"));
    }

    // made up, and put together here so no scanner mistakes the source for a real token
    fn token() -> String {
        ["tk", "9f8e7d6c5b4a39281706f5e4d3c2b1a0"].join("_")
    }

    #[test]
    fn no_secret_survives() {
        let text = [
            "wss://hub.example:4777/ws?ticket=abc123ticketvalue&catalog=3".to_string(),
            format!("authorization: {} {}", ["Bear", "er"].concat(), token()),
            "Cookie: inyon_session=s3ss10n-c00kie-value; theme=dark".into(),
            "set the code K7Q4-MX2T 493812 or K7Q4MX2T:493812".into(),
            r#"{"password": "hunter2"}"#.into(),
            "inyon://pair?m=yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o&s=abc".into(),
            "https://inyon.tech/download/#hub?u=https://box&c=K7Q4MX2T:493812".into(),
            "https://relay.pkarr.org/yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o: 404".into(),
        ]
        .join("\n");
        let out = redact(&text);
        for s in [
            token().as_str(),
            "abc123ticketvalue",
            "K7Q4-MX2T 493812",
            "K7Q4MX2T:493812",
            "hunter2",
            "s3ss10n-c00kie-value",
            "yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o",
        ] {
            assert!(!out.contains(s), "{s} in {out}");
        }
    }

    #[test]
    fn addresses_words_and_paths_stay_readable() {
        let text = "Couldn't connect to hub.example:4777. https://hub.example/api/session answered 421. close code 1006 at /home/sam/development/project-folder";
        assert_eq!(redact(text), text);
        assert_eq!(redact("wss://hub.example/ws?ticket=x"), "wss://hub.example/ws?[hidden]");
        assert_eq!(redact("Bearer abc"), "Bearer [hidden]");
        assert_eq!(redact("inyon://hub?u=x&c=y"), "inyon://hub?[hidden]");
        assert_eq!(redact("password: \"correct horse battery staple\" then token='two words' end"), "password: [hidden] then token=[hidden] end");
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact(&format!("key:\n{key}\nafter"));
        assert!(!out.contains("b3BlbnNzaC1rZXktdjEAAAAA"), "{out}");
        assert!(out.ends_with("after"));
        assert_eq!(redact("Lösung ✳ fine"), "Lösung ✳ fine");
    }
}
