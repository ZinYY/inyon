//! Port of tmux/tmux.ts and tmux/live.ts: the probe script and what it prints, the pane list,
//! captured screens, and the commands that type into a pane. The pure functions take what a
//! command printed and return what it means, or build the command; the async ones run them on a
//! [`Host`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use inyon_protocol::{agents, detect_agent, AgentKind, IntegrationState, MachineKind, PaneInfo, SessionInfo, WindowInfo, AGENT_KINDS};
use serde_json::Value;

use super::machine_kind::machine_kind;
use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host, StreamHandle};
use crate::hosts::http_error::{HubError, HubResult};
use crate::js::json::{self, Js};
use crate::js::num::parse_int;
use crate::js::{to_number, JsStr};
use crate::platform::rt::sleep;
use crate::re;

/// Sessions Inyon creates for terminal views; hidden from listings.
pub const VIEW_PREFIX: &str = "inyon-view-";
/// Sessions running an agent's sign-in; hidden too.
pub const LOGIN_PREFIX: &str = "inyon-login-";
/// Every prefix of a session that isn't yours, those from before the rename to Inyon included
pub const HIDDEN_PREFIXES: [&str; 4] = [VIEW_PREFIX, LOGIN_PREFIX, "tether-view-", "tether-login-"];

pub fn hidden_session(name: &str) -> bool {
    HIDDEN_PREFIXES.iter().any(|p| name.starts_with(p))
}

const SEP: char = '\u{1f}';

/// The `-F` format the probe lists panes with, fields joined by U+001F.
pub fn pane_format() -> String {
    [
        "#{session_id}",
        "#{session_name}",
        "#{session_attached}",
        "#{window_id}",
        "#{window_index}",
        "#{window_name}",
        "#{window_active}",
        "#{window_activity}",
        "#{pane_id}",
        "#{pane_index}",
        "#{pane_active}",
        "#{pane_pid}",
        "#{pane_current_command}",
        "#{pane_current_path}",
        "#{pane_title}",
        "#{pane_width}",
        "#{pane_height}",
        "#{pane_dead}",
    ]
    .join("\u{1f}")
}

/// One line of the pane format's output split into its fields. Older tmux (3.4, as on Ubuntu
/// 24.04) writes the separator escaped, as the four characters \037, and a backslash as \; those
/// lines are unescaped first. 3.6 writes them as they are.
pub fn tmux_fields(line: &str) -> Vec<String> {
    let line = if !line.contains(SEP) && line.contains("\\037") {
        re!(r"\\(\\|[0-7]{3})", "g")
            .replace_all(line, |c: &regex::Captures| {
                let e = &c[1];
                if e == "\\" {
                    "\\".to_owned()
                } else {
                    char::from_u32(parse_int(e, 8) as u32).map(String::from).unwrap_or_default()
                }
            })
            .into_owned()
    } else {
        line.to_owned()
    };
    line.split(SEP).map(str::to_owned).collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Proc {
    pid: f64,
    ppid: f64,
    etimes: f64,
    comm: String,
    args: String,
}

/// A line of ~/.inyon/panes/*.json: the hook's last word on a pane.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HookPaneState {
    pub pane: String,
    pub ts: f64,
    /// session_id, transcript_path, cwd, hook_event_name and whatever else the hook sent
    pub data: Value,
    /// the agent that sent it, when it isn't Claude Code (`hook.sh copilot agentStop`)
    pub agent: Option<AgentKind>,
    /// that agent's own name for the event
    pub event: Option<String>,
}

impl HookPaneState {
    /// Whether it came from this kind of agent: a line without a name is Claude Code's.
    pub fn from_agent(&self, kind: AgentKind) -> bool {
        self.agent.unwrap_or(AgentKind::Claude) == kind
    }

    /// The event in Claude Code's words, whichever agent sent it: a prompt (UserPromptSubmit), the
    /// end of a turn (Stop), a question mid-turn (Notification), a session starting or ending.
    pub fn event_name(&self) -> &str {
        let data = Some(&self.data);
        let Some(agent) = self.agent else { return data.at("hook_event_name").str().unwrap_or("") };
        let event = self.event.as_deref().unwrap_or("");
        match (agent, event) {
            // Copilot starts its session on the first prompt, just after userPromptSubmitted (1.0.95),
            // so its sessionStart says nothing about the turn
            (AgentKind::Copilot, "sessionStart") => "",
            (_, "SessionStart") => "SessionStart",
            (_, "SessionEnd" | "sessionEnd") => "SessionEnd",
            (AgentKind::Copilot, "userPromptSubmitted") | (AgentKind::Gemini, "BeforeAgent") | (AgentKind::Antigravity, "PreInvocation") => "UserPromptSubmit",
            (AgentKind::Copilot, "agentStop") | (AgentKind::Gemini, "AfterAgent") | (AgentKind::Antigravity, "Stop") => "Stop",
            // Copilot says it's idle as a notification; an error it can't recover from ends the turn
            (AgentKind::Copilot, "notification") if data.at("notification_type").str() == Some("agent_idle") => "Stop",
            (AgentKind::Copilot, "errorOccurred") if data.at("recoverable").and_then(Value::as_bool) == Some(false) => "Stop",
            (AgentKind::Copilot, "notification" | "permissionRequest") | (AgentKind::Gemini, "Notification") => "Notification",
            _ => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentProc {
    pub kind: AgentKind,
    pub pid: f64,
    pub started_at: f64,
    pub cwd: Option<String>,
    pub resume: Option<String>,
    pub args: String,
    /// the pane's own process; equal to pid when the agent was started as the pane command
    pub pane_pid: f64,
    /// the name of the custom agent this process is (Settings › Agents)
    pub custom: Option<String>,
}

/// The last status line payload Claude wrote for a pane, with the account it was written under
/// and the one signed in now.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusPane {
    pub ts: f64,
    pub data: Value,
    pub account: Option<String>,
    pub account_now: Option<String>,
}

/// The background session a Claude Code window moved its conversation to, which the window shows
/// and takes messages for from then on, with that session's transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct ParkedClaude {
    pub session_id: String,
    pub transcript: String,
}

/// Antigravity CLI's last status line payload for a pane.
#[derive(Debug, Clone, PartialEq)]
pub struct AgyStatusPane {
    pub ts: f64,
    pub data: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxProbe {
    Ok,
    NoServer,
    Missing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub hostname: String,
    pub home: String,
    pub os: String,
    /// unix seconds
    pub now: f64,
    pub latency_ms: f64,
    pub tmux: TmuxProbe,
    /// never Unknown
    pub integration: IntegrationState,
    pub sessions: Vec<SessionInfo>,
    /// pane id → agent process found inside it
    pub agent_procs: IndexMap<String, AgentProc>,
    pub hook_panes: IndexMap<String, HookPaneState>,
    /// pane id → last status line payload (Claude)
    pub status_panes: IndexMap<String, StatusPane>,
    /// Claude account this machine is signed in to now (oauthAccount.accountUuid)
    pub account: Option<String>,
    /// pane id → unix seconds of the last compaction (Claude)
    pub compact_panes: IndexMap<String, f64>,
    pub installed: Vec<AgentKind>,
    /// agents with a saved sign-in on the machine
    pub signed_in: Vec<AgentKind>,
    /// Antigravity CLI: process id → the conversation it has open (from the presence lock it holds)
    pub agy_conversations: IndexMap<i64, String>,
    /// Antigravity CLI: pane id → last status line payload
    pub agy_status: IndexMap<String, AgyStatusPane>,
    /// Antigravity conversation id → its title
    pub agy_titles: IndexMap<String, String>,
    /// whether Inyon's status line is in Antigravity CLI's settings (None when agy isn't installed)
    pub agy_integration: Option<bool>,
    /// the agents other than Claude Code whose settings run Inyon's hook
    pub hooked: Vec<AgentKind>,
    /// pane id → unix seconds of the last key a person pressed in a tmux client showing it
    pub keys_at: IndexMap<String, f64>,
    /// window id → its size, for each window no person's tmux client is attached to (through any
    /// session holding it, Inyon's own views included)
    pub unwatched: IndexMap<String, (u32, u32)>,
    /// Claude Code process id → the background session its window moved the conversation to
    pub claude_parked: IndexMap<i64, ParkedClaude>,
    pub machine_kind: Option<MachineKind>,
}

#[cfg(any(test, feature = "testing"))]
impl ProbeResult {
    /// A Linux machine named "box" with tmux and nothing else, whose home is `home`: for tests that
    /// need a connected machine without probing one.
    pub fn at_home(home: &str) -> Self {
        ProbeResult {
            hostname: "box".into(),
            home: home.into(),
            os: "Linux".into(),
            now: 0.0,
            latency_ms: 0.0,
            tmux: TmuxProbe::Ok,
            integration: IntegrationState::Missing,
            sessions: vec![],
            agent_procs: IndexMap::new(),
            hook_panes: IndexMap::new(),
            status_panes: IndexMap::new(),
            account: None,
            compact_panes: IndexMap::new(),
            installed: vec![],
            signed_in: vec![],
            agy_conversations: IndexMap::new(),
            agy_status: IndexMap::new(),
            agy_titles: IndexMap::new(),
            agy_integration: None,
            hooked: vec![],
            machine_kind: None,
            keys_at: IndexMap::new(),
            unwatched: Default::default(),
            claude_parked: Default::default(),
        }
    }
}

/// `<pid> <folder>` lines → folder per process. Linux adds " (deleted)" once the folder is
/// removed (a worktree cleaned up while the agent runs); the agent's transcripts still name the
/// folder as it was, so drop the mark or the agent can't be matched to its chat.
pub fn proc_cwds(lines: &[String]) -> HashMap<i64, String> {
    let mut cwds = HashMap::new();
    for line in lines {
        let i = line.find(' ');
        let rest = match i {
            Some(i) => &line[i + 1..],
            None => line.as_str(),
        };
        let dir = re!(r" \(deleted\)$").replacen(rest, 1, "");
        if let Some(i) = i.filter(|&i| i > 0) {
            if !dir.is_empty() {
                cwds.insert(to_number(&line[..i]) as i64, dir.into_owned());
            }
        }
    }
    cwds
}

fn num(s: &str) -> f64 {
    to_number(s)
}

/// What `probe()` makes of the probe script's output. `stderr` and `code` only go into the error
/// when the script didn't run to its end.
pub fn parse_probe(stdout: &str, stderr: &str, code: i32, latency_ms: f64) -> Result<ProbeResult, String> {
    if !stdout.contains("@@END") {
        let text = if !stderr.is_empty() {
            stderr.to_owned()
        } else if !stdout.is_empty() {
            stdout.to_owned()
        } else {
            format!("exit {code}")
        };
        let lines: Vec<&str> = text.jtrim().split('\n').collect();
        return Err(lines[lines.len().saturating_sub(2)..].join(" "));
    }
    let mut sections: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut cur = String::new();
    let mut res = ProbeResult {
        hostname: String::new(),
        home: String::new(),
        os: String::new(),
        now: crate::clock::now_ms() as f64 / 1000.0,
        latency_ms,
        tmux: TmuxProbe::Ok,
        integration: IntegrationState::Missing,
        sessions: vec![],
        agent_procs: IndexMap::new(),
        hook_panes: IndexMap::new(),
        status_panes: IndexMap::new(),
        account: None,
        compact_panes: IndexMap::new(),
        installed: vec![],
        signed_in: vec![],
        agy_conversations: IndexMap::new(),
        agy_status: IndexMap::new(),
        agy_titles: IndexMap::new(),
        agy_integration: None,
        hooked: vec![],
        machine_kind: None,
        keys_at: IndexMap::new(),
        unwatched: IndexMap::new(),
        claude_parked: IndexMap::new(),
    };
    for line in stdout.split('\n') {
        if let Some(v) = line.strip_prefix("@@HOST ") {
            res.hostname = v.jtrim().to_owned();
        } else if let Some(v) = line.strip_prefix("@@HOME ") {
            res.home = v.jtrim().to_owned();
        } else if let Some(v) = line.strip_prefix("@@OS ") {
            // busybox-w32 (inyon on Windows) says Windows_NT
            let os = v.jtrim();
            res.os = if os == "Windows_NT" { "Windows".into() } else { os.to_owned() };
        } else if let Some(v) = line.strip_prefix("@@NOW ") {
            res.now = num(v);
        } else if let Some(v) = line.strip_prefix("@@KIND ") {
            res.machine_kind = machine_kind(v);
        } else if let Some(v) = line.strip_prefix("@@AUTH") {
            let words: Vec<&str> = re!(r"\s+").split(v.jtrim()).collect();
            res.signed_in = AGENT_KINDS.iter().copied().filter(|k| words.contains(&k.as_str())).collect();
        } else if let Some(v) = line.strip_prefix("@@ACCOUNT ") {
            let v = v.jtrim();
            res.account = if v == "-" { None } else { Some(v.to_owned()) };
        } else if let Some(v) = line.strip_prefix("@@AGYINT ") {
            res.agy_integration = Some(v.jtrim() == "installed");
        } else if line == "@@NOSERVER" {
            res.tmux = TmuxProbe::NoServer;
        } else if line == "@@NOTMUX" {
            res.tmux = TmuxProbe::Missing;
        } else if re!(r"^@@[A-Z]+$").is_match(line) {
            cur = line[2..].to_owned();
            sections.insert(cur.clone(), vec![]);
        } else if !cur.is_empty() {
            sections.get_mut(&cur).expect("section").push(line.to_owned());
        }
    }
    let empty = vec![];
    let section = |name: &str| sections.get(name).unwrap_or(&empty);

    // processes
    let mut procs: HashMap<i64, Proc> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for line in section("PS") {
        let Some(m) = re!(r"^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(.*)$").captures(line) else { continue };
        // macOS gives the executable's full path
        let comm = m[4].rsplit('/').next().unwrap_or("").to_owned();
        let p = Proc { pid: num(&m[1]), ppid: num(&m[2]), etimes: num(&m[3]), comm, args: m[5].to_owned() };
        children.entry(p.ppid as i64).or_default().push(p.pid as i64);
        procs.insert(p.pid as i64, p);
    }
    let cwds = proc_cwds(section("CWD"));
    let find_agent = |root: i64| -> Option<(&Proc, AgentKind, Option<String>)> {
        let mut queue = VecDeque::from([(root, 0)]);
        while let Some((pid, depth)) = queue.pop_front() {
            let p = procs.get(&pid);
            if let Some(p) = p {
                // yours first: `claude --dangerously-skip-permissions` saved as "claude-yolo" is that
                if let Some(c) = crate::agents::custom::detect(&p.comm, &p.args) {
                    // a saved wrapper script stands for the agent it starts, whose process holds the
                    // session (pid, folder, resume flag); the wrapper itself is the fallback
                    let started = children.get(&pid).into_iter().flatten().filter_map(|c| procs.get(c)).find_map(|cp| detect_agent(&cp.comm, &cp.args).map(|k| (cp, k)));
                    return Some(match started {
                        Some((cp, kind)) => (cp, kind, Some(c.name)),
                        None => (p, c.like, Some(c.name)),
                    });
                }
                if let Some(kind) = detect_agent(&p.comm, &p.args) {
                    return Some((p, kind, None));
                }
            }
            if depth < 4 {
                for c in children.get(&pid).into_iter().flatten() {
                    queue.push_back((*c, depth + 1));
                }
            }
        }
        None
    };

    // Claude Code's records of its sessions: a window that moved its conversation to the background
    // names that session's job, and the background session (still running) names its transcript
    let mut background: HashMap<String, ParkedClaude> = HashMap::new();
    let mut parked: Vec<(i64, String)> = Vec::new();
    for line in section("CLAUDESESS") {
        let Some((path, record)) = line.split_once('\t') else { continue };
        let Some(v) = json::parse(record) else { continue };
        let v = Some(&v);
        let Some(pid) = v.at("pid").num().map(|p| p as i64) else { continue };
        if let Some(job) = v.at("parkedJobId").str() {
            parked.push((pid, job.to_owned()));
        } else if let (Some("bg"), Some(job), Some(id)) = (v.at("kind").str(), v.at("jobId").str(), v.at("sessionId").str()) {
            if path != "-" && (procs.is_empty() || procs.contains_key(&pid)) {
                background.insert(job.to_owned(), ParkedClaude { session_id: id.to_owned(), transcript: path.to_owned() });
            }
        }
    }
    for (pid, job) in parked {
        if let Some(b) = background.get(&job) {
            res.claude_parked.insert(pid, b.clone());
        }
    }

    let bins: HashSet<&str> = section("BINS").iter().map(|l| l.jtrim()).filter(|l| !l.is_empty()).collect();
    res.installed = AGENT_KINDS.iter().copied().filter(|k| agents()[k].comm.iter().any(|c| bins.contains(c.as_str()))).collect();
    for line in section("STATUS") {
        let Some(m) = re!(r"^(\d+(?:\.\d+)?) (\d+) (\S+) (\S+) (.*)$").captures(line) else { continue };
        let acct = |v: &str| if v == "-" { None } else { Some(v.to_owned()) };
        if let Some(data) = json::parse(&m[5]) {
            res.status_panes.insert(format!("%{}", &m[1]), StatusPane { ts: num(&m[2]), account: acct(&m[3]), account_now: acct(&m[4]), data });
        }
    }
    for line in section("AGY") {
        if let Some(m) = re!(r"^\d+ (\d+) .*\/presence\/([0-9a-f-]{36})\.lock$").captures(line) {
            res.agy_conversations.insert(num(&m[1]) as i64, m[2].to_owned());
        }
    }
    for line in section("AGYTITLE") {
        if let Some(m) = re!(r"^([0-9a-f-]{36}) (.+)$").captures(line) {
            res.agy_titles.insert(m[1].to_owned(), m[2].jtrim().to_owned());
        }
    }
    for line in section("AGYSTATUS") {
        let Some(m) = re!(r"^(\d+(?:\.\d+)?) (\d+) (.*)$").captures(line) else { continue };
        if let Some(data) = json::parse(&m[3]) {
            res.agy_status.insert(format!("%{}", &m[1]), AgyStatusPane { ts: num(&m[2]), data });
        }
    }
    for line in section("COMPACT") {
        if let Some(m) = re!(r"^(\d+(?:\.\d+)?) (\d+)").captures(line) {
            res.compact_panes.insert(format!("%{}", &m[1]), num(&m[2]));
        }
    }

    let hook = section("HOOK").first().map(|h| h.jtrim());
    res.integration = match hook {
        Some("installed") => IntegrationState::Installed,
        Some("removed") => IntegrationState::Removed,
        Some("outdated") => IntegrationState::Outdated,
        _ => IntegrationState::Missing,
    };
    res.hooked = AGENT_KINDS.iter().copied().filter(|k| section("HOOKED").iter().any(|l| l.jtrim() == k.as_str())).collect();
    for line in section("PANES") {
        if line.is_blank() {
            continue;
        }
        let Some(v) = json::parse(line) else { continue };
        let pane = Some(&v).at("pane");
        if pane.truthy() {
            let pane = pane.to_js_string();
            let state = HookPaneState {
                pane: pane.clone(),
                ts: Some(&v).at("ts").num().unwrap_or(f64::NAN),
                data: Some(&v).at("data").cloned().unwrap_or(Value::Null),
                agent: Some(&v).at("agent").str().and_then(|a| serde_json::from_value(Value::String(a.to_owned())).ok()),
                event: Some(&v).at("event").str().map(str::to_owned),
            };
            res.hook_panes.insert(pane, state);
        }
    }

    // tmux tree
    let mut sessions: IndexMap<String, SessionInfo> = IndexMap::new();
    // every session's windows, hidden ones too
    struct Linked {
        session: String,
        attached: bool,
        window: String,
        size: Option<(u32, u32)>,
    }
    let mut windows: Vec<Linked> = vec![];
    for line in section("TMUX") {
        let f = tmux_fields(line);
        if f.len() < 18 {
            continue;
        }
        let [sid, sname, sattached, wid, widx, wname, wactive, wact, pid, pidx, pactive, ppid, pcmd, pcwd, ptitle, pw, ph, pdead] = std::array::from_fn(|i| f[i].as_str());
        let size = (f.get(18).and_then(|w| w.parse().ok()), f.get(19).and_then(|h| h.parse().ok()));
        windows.push(Linked { session: sid.to_owned(), attached: num(sattached) > 0.0, window: wid.to_owned(), size: size.0.zip(size.1) });
        if hidden_session(sname) {
            continue;
        }
        let s =
            sessions.entry(sid.to_owned()).or_insert_with(|| SessionInfo { id: sid.into(), name: sname.into(), attached: inyon_protocol::Num(num(sattached)), windows: vec![] });
        // windows are keyed by session and window id, and each session holds its own
        let w = match s.windows.iter().position(|w| w.id == wid) {
            Some(i) => &mut s.windows[i],
            None => {
                s.windows.push(WindowInfo {
                    id: wid.into(),
                    index: inyon_protocol::Num(num(widx)),
                    name: wname.into(),
                    active: wactive == "1",
                    activity: inyon_protocol::Num(num(wact)),
                    panes: vec![],
                });
                s.windows.last_mut().expect("window")
            }
        };
        w.panes.push(PaneInfo {
            id: pid.into(),
            index: inyon_protocol::Num(num(pidx)),
            active: pactive == "1",
            command: pcmd.into(),
            cwd: pcwd.into(),
            title: ptitle.into(),
            label: None,
            width: inyon_protocol::Num(num(pw)),
            height: inyon_protocol::Num(num(ph)),
            dead: pdead == "1",
            agent: None,
            companion: None,
        });
        if let Some((cp, kind, custom)) = find_agent(num(ppid) as i64) {
            let resume = re!(r"(?:--resume|-r|resume)[ =]([0-9a-f]{8}-[0-9a-f-]{27})").captures(&cp.args).map(|c| c[1].to_owned());
            let cwd = cwds.get(&(cp.pid as i64)).cloned().unwrap_or_else(|| pcwd.to_owned());
            res.agent_procs.insert(
                pid.to_owned(),
                AgentProc { kind, pid: cp.pid, started_at: res.now - cp.etimes, cwd: Some(cwd), resume, args: cp.args.clone(), pane_pid: num(ppid), custom },
            );
        }
    }
    // the "attached" count shouldn't include Inyon's own control-mode clients
    let mut people: HashMap<&str, u32> = HashMap::new();
    for line in section("CLIENTS") {
        let mut parts = line.jtrim().split(' ');
        let (sid, control) = (parts.next().unwrap_or(""), parts.next());
        if !sid.is_empty() && control == Some("0") {
            *people.entry(sid).or_default() += 1;
            // the client's last key and the pane it shows (psmux answers without them)
            if let (Some(at), Some(pane)) = (parts.next().map(num).filter(|t| *t > 0.0), parts.next().filter(|p| p.starts_with('%'))) {
                let t = res.keys_at.entry(pane.to_owned()).or_insert(at);
                *t = t.max(at);
            }
        }
    }
    if sections.contains_key("CLIENTS") {
        for s in sessions.values_mut() {
            s.attached = inyon_protocol::Num(f64::from(people.get(s.id.as_str()).copied().unwrap_or(0)));
        }
    }
    // a window shows in every session that links it, and any one with a person attached sizes it
    let watched: HashSet<&str> =
        windows.iter().filter(|l| if sections.contains_key("CLIENTS") { people.contains_key(l.session.as_str()) } else { l.attached }).map(|l| l.window.as_str()).collect();
    for l in &windows {
        if let Some(size) = l.size.filter(|_| !watched.contains(l.window.as_str())) {
            res.unwatched.insert(l.window.clone(), size);
        }
    }
    // the hook's last word from a window whose conversation went to the background is about the
    // conversation it left, until the background session's own events reach it
    for (pane, ap) in &res.agent_procs {
        let Some(b) = res.claude_parked.get(&(ap.pid as i64)) else { continue };
        if res.hook_panes.get(pane).is_some_and(|h| Some(&h.data).at("session_id").str() != Some(b.session_id.as_str())) {
            res.hook_panes.shift_remove(pane);
        }
    }
    res.sessions = sessions.into_values().collect();
    Ok(res)
}

/// The command `capturePanes` runs: each pane's visible screen, with styling, in one round trip.
pub fn capture_command(pane_ids: &[String]) -> Option<String> {
    if pane_ids.is_empty() {
        return None;
    }
    Some(pane_ids.iter().map(|p| format!("echo {}; tmux capture-pane -p -e -t {} 2>/dev/null", shq(&format!("@@CAP {p}")), shq(p))).collect::<Vec<_>>().join("; "))
}

/// What `capturePanes` makes of that command's output: pane id → its rows.
pub fn parse_captures(stdout: &str) -> IndexMap<String, Vec<String>> {
    let mut out: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut cur: Option<String> = None;
    for line in stdout.split('\n') {
        if let Some(p) = line.strip_prefix("@@CAP ") {
            out.insert(p.to_owned(), vec![]);
            cur = Some(p.to_owned());
        } else if let Some(c) = &cur {
            out.get_mut(c).expect("pane").push(line.to_owned());
        }
    }
    out
}

/// Pastes what comes in on stdin into a pane as one bracketed paste. The text goes through a
/// file, since psmux's load-buffer only reads a path, not "-". psmux also runs a server per
/// session and buffers live in one server, so on Windows the buffer is loaded into the pane's own
/// session (the -t before the command, which psmux takes for any command); otherwise paste-buffer
/// can't find it.
pub fn paste_command(buffer: &str, pane: &str) -> String {
    through_buffer(buffer, pane, "-p ")
}

/// Types what comes in on stdin into a pane as keys would, through a buffer as a paste does but
/// without the paste markers. Text never goes on a command line: another user on the machine can
/// read every process's arguments (/proc/*/cmdline), and this is how messages, drafts and API keys
/// typed into a sign-in reach the pane.
pub fn type_command(buffer: &str, pane: &str) -> String {
    through_buffer(buffer, pane, "")
}

fn through_buffer(buffer: &str, pane: &str, flags: &str) -> String {
    let b = shq(buffer);
    let t = r#"${s:+-t "$s"}"#;
    let p = shq(pane);
    // the trap deletes the file (it holds what was pasted) even when the connection drops half way;
    // once it's gone f is emptied, so the trap doesn't start a second rm (each start costs on Windows)
    format!(
        r#"umask 077; f=$(mktemp) && trap '[ -z "$f" ] || rm -f "$f"' EXIT HUP INT TERM && {{ s=; [ "$OS" = Windows_NT ] && s={p}; cat > "$f" && tmux {t} load-buffer -b {b} "$f" && tmux paste-buffer -d {flags}-b {b} -t {p}; r=$?; rm -f "$f"; f=; [ $r = 0 ] || tmux {t} delete-buffer -b {b} 2>/dev/null; [ $r = 0 ]; }}"#
    )
}

/// Text for a bracketed paste: line ends made \n, and no paste markers of its own. tmux doesn't
/// filter what it pastes, so an end marker inside would close the paste early and the rest would
/// arrive as typed keys (Ctrl-U, !, Enter). Removed until none is left, so halves can't join up.
pub fn paste_text(text: &str) -> String {
    let mut s = re!(r"\r\n?", "g").replace_all(text, "\n").into_owned();
    loop {
        let next = re!(r"(?:\x1b\[|\x9b)20[01]~", "g").replace_all(&s, "").into_owned();
        if next == s {
            return s;
        }
        s = next;
    }
}

/// How `typeText` gets text into a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeText {
    /// one bracketed paste: run `command` with `input` on stdin, then Enter after 150 ms if `submit`
    Paste { command: String, input: String, submit: bool },
    /// typed through a buffer: run `command` with `input` on stdin; the Enter (if any) is in it
    Keys { command: String, input: String },
    /// nothing to type
    Nothing,
}

/// Types text into a pane exactly as a person would; newlines become Ctrl+J (newline without
/// submitting). With `submit`, Enter follows after a short pause, all in one round trip. Long or
/// multi-line text is one paste through `buffer` (`inyon-<uuid>` in the hub).
pub fn type_text(pane: &str, text: &str, submit: bool, buffer: &str) -> TypeText {
    let normal = re!(r"\r\n?", "g").replace_all(text, "\n").into_owned();
    if text.len16() > 800 || re!(r"[\r\n]").is_match(text) {
        // Deliver one bracketed paste instead of a burst of individual key events. The CLI can
        // preserve long/multiline text atomically and display its own collapsed paste preview.
        return TypeText::Paste { command: paste_command(buffer, pane), input: paste_text(text), submit };
    }
    // one line from here on (a line end makes it a paste)
    let mut command = String::new();
    if !normal.is_empty() {
        command = type_command(buffer, pane);
    }
    if submit {
        let enter = format!("sleep 0.12 && tmux send-keys -t {} Enter", shq(pane));
        command = if command.is_empty() { enter } else { format!("{command} && {enter}") };
    }
    if command.is_empty() {
        TypeText::Nothing
    } else {
        TypeText::Keys { command, input: normal }
    }
}

/// letters, the named keys, function keys, and Ctrl/Alt chords (OpenCode's bindings can be
/// remapped to those)
pub fn key_allowed(key: &str) -> bool {
    re!(r"^([A-Za-z0-9]|Enter|Escape|Tab|BTab|Up|Down|Left|Right|S-Left|S-Right|Space|BSpace|Home|End|PageUp|PageDown|F[1-9]|F1[0-2]|(C-|M-|C-M-)[A-Za-z0-9])$").is_match(key)
}

/// The command `sendKeys` runs, or the error it throws for a key that isn't allowed.
pub fn send_keys_command(pane: &str, keys: &[&str]) -> Result<String, String> {
    if let Some(k) = keys.iter().find(|k| !key_allowed(k)) {
        return Err(format!("key not allowed: {k}"));
    }
    Ok(keys.iter().map(|k| format!("tmux send-keys -t {} {k}", shq(pane))).collect::<Vec<_>>().join(" && "))
}

/// The folder under ~/.claude/projects where Claude keeps a working folder's transcripts.
pub fn claude_slug(cwd: &str) -> String {
    // per UTF-16 unit, as the TS regex replaces each one
    cwd.chars().flat_map(|c| std::iter::repeat_n(if c.is_ascii_alphanumeric() { c } else { '-' }, if c.is_ascii_alphanumeric() { 1 } else { c.len_utf16() })).collect()
}

/// A Claude process as `guessTranscript` sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptProc<'a> {
    pub cwd: Option<&'a str>,
    pub started_at: f64,
    pub resume: Option<&'a str>,
}

/// The folder `guessTranscript` looks in and the command that lists its newest transcripts, or
/// None for a process without a folder.
pub fn guess_transcript_command(home: &str, proc: &TranscriptProc) -> Option<(String, String)> {
    let dir = format!("{home}/.claude/projects/{}", claude_slug(proc.cwd?));
    let cmd = format!(r#"cd {} 2>/dev/null && for f in $(ls -t *.jsonl 2>/dev/null | head -5); do echo "$(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f") $f"; done"#, shq(&dir));
    Some((dir, cmd))
}

/// Best guess at a Claude process's transcript without hooks, from what that command printed:
/// the session it was resumed with, or the newest transcript in its project folder written since
/// the process started. A brand-new session has no transcript until its first message, so older
/// files are never picked.
pub fn guess_transcript(stdout: &str, dir: &str, proc: &TranscriptProc) -> Option<String> {
    let files: Vec<(f64, &str)> = stdout
        .jtrim()
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| match l.find(' ') {
            Some(i) => (to_number(&l[..i]), &l[i + 1..]),
            // `l.slice(0, -1)` and the whole line, as indexOf's -1 makes them
            None => (to_number(l.to16(-1)), l),
        })
        .collect();
    if let Some(resume) = proc.resume {
        let hit = files.iter().any(|f| f.1 == format!("{resume}.jsonl"));
        if hit || files.is_empty() {
            return Some(format!("{dir}/{resume}.jsonl"));
        }
    }
    match files.iter().find(|f| f.0 >= proc.started_at - 2.0) {
        Some(f) => Some(format!("{dir}/{}", f.1)),
        None => proc.resume.map(|r| format!("{dir}/{r}.jsonl")),
    }
}

// The shell the probe runs, byte for byte the TS's (fixtures/expected/scripts.json checks it).

/// Prints whether Claude's integration is installed, missing, removed from settings or outdated.
pub const HOOK_STATE_SH: &str = r#"t="$HOME/.inyon"
s="$HOME/.claude/settings.json"
current() { v=$(sed -n 's/^# inyon-integration \([0-9][0-9]*\)$/\1/p' "$1" 2>/dev/null | head -n 1); [ -n "$v" ] && [ "$v" -ge 9 ]; }
if [ ! -x "$t/hook.sh" ] || [ ! -x "$t/statusline.sh" ]; then echo missing
# settings from before the rename to Inyon still name ~/.tether: outdated, so they're rewritten
elif grep -q 'tether/hook.sh' "$s" 2>/dev/null || grep -q 'tether/statusline.sh' "$s" 2>/dev/null; then echo outdated
elif ! grep -q 'inyon/statusline.sh' "$s" 2>/dev/null || ! grep -q 'inyon/hook.sh' "$s" 2>/dev/null; then echo removed
elif current "$t/hook.sh" && current "$t/statusline.sh"; then echo installed
else echo outdated; fi"#;
/// What every poll runs on a machine: its tmux sessions and panes, the agents running in them, and
/// what Inyon's hooks and status lines wrote since.
pub const PROBE: &str = r#"
export PATH="$HOME/.local/bin:$HOME/bin:$HOME/.npm-global/bin:$HOME/.bun/bin:$HOME/.cargo/bin:$HOME/.opencode/bin:$HOME/.grok/bin:$HOME/.kimi-code/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
for d in "$HOME"/.nvm/versions/node/*/bin "$HOME"/.local/share/pnpm; do [ -d "$d" ] && PATH="$PATH:$d"; done
echo "@@HOST $(hostname)"
echo "@@HOME $HOME"
echo "@@OS $(uname -s)"
echo "@@NOW $(date +%s)"
if [ -d "$HOME/.tether" ] && [ ! -L "$HOME/.tether" ] && [ ! -e "$HOME/.inyon" ]; then mv "$HOME/.tether" "$HOME/.inyon" && { [ -e "$HOME/.tether" ] || ln -s "$HOME/.inyon" "$HOME/.tether"; }; fi

k="$HOME/.inyon/machine-kind"
if [ ! -s "$k" ] || [ -n "$(find "$k" -mmin +1440 2>/dev/null)" ]; then
  mkdir -p "$HOME/.inyon" 2>/dev/null
  os=$(uname -s); ct=; hc=; vt=; g=0; b=0; md=; pt=
  case "$os" in
  Linux)
    ct=$(cat /sys/class/dmi/id/chassis_type 2>/dev/null)
    hc=$(hostnamectl chassis 2>/dev/null || hostnamectl status 2>/dev/null | sed -n 's/^ *Chassis: *\([a-z-]*\).*/\1/p')
    vt=$(systemd-detect-virt 2>/dev/null)
    [ -z "$vt" ] && [ -f /.dockerenv ] && vt=docker
    { [ -n "$DISPLAY$WAYLAND_DISPLAY" ] || ls /tmp/.X11-unix/X* /run/user/*/wayland-* >/dev/null 2>&1 || pgrep -x 'Xorg|Xwayland|gnome-shell|plasmashell|kwin_wayland|sway|Hyprland|weston|xfwm4|cinnamon' >/dev/null 2>&1; } && g=1
    for f in /sys/class/power_supply/*/type; do [ "$(cat "$f" 2>/dev/null)" = Battery ] && b=1; done
    md=$(cat /sys/class/dmi/id/product_name 2>/dev/null) ;;
  Darwin)
    g=1
    pmset -g batt 2>/dev/null | grep -q InternalBattery && b=1
    md=$(sysctl -n hw.model 2>/dev/null) ;;
  Windows_NT|MINGW*|MSYS*|CYGWIN*)
    os=Windows; g=1
    w=$(powershell.exe -NoProfile -NonInteractive -Command '$e = Get-CimInstance Win32_SystemEnclosure; $c = Get-CimInstance Win32_ComputerSystem; $o = Get-CimInstance Win32_OperatingSystem; ($e.ChassisTypes -join ",") + "|" + @(Get-CimInstance Win32_Battery).Count + "|" + $c.Model + "|" + $o.ProductType' 2>/dev/null | tr -d '\r' | tail -n 1)
    ct=$(echo "$w" | cut -d '|' -f 1); b=$(echo "$w" | cut -d '|' -f 2); md=$(echo "$w" | cut -d '|' -f 3); pt=$(echo "$w" | cut -d '|' -f 4) ;;
  *) g=1 ;;
  esac
  printf '%s|%s|%s|%s|%s|%s|%s|%s\n' "$os" "$ct" "$hc" "$vt" "$g" "${b:-0}" "$(echo "$md" | tr -d '|')" "$pt" > "$k"
fi
echo "@@KIND $(head -n 1 "$k" 2>/dev/null)"

echo "@@BINS"
for b in claude codex opencode copilot gemini agy cursor-agent grok pi amp goose hermes kimi kiro-cli kiro-cli-chat qwen qodercli qoder codebuddy; do command -v "$b" >/dev/null 2>&1 && echo "$b"; done
if command -v tmux >/dev/null 2>&1; then
  echo "@@TMUX"
  tmux list-panes -a -F '#{session_id}#{session_name}#{session_attached}#{window_id}#{window_index}#{window_name}#{window_active}#{window_activity}#{pane_id}#{pane_index}#{pane_active}#{pane_pid}#{pane_current_command}#{pane_current_path}#{pane_title}#{pane_width}#{pane_height}#{pane_dead}#{window_width}#{window_height}' 2>/dev/null || echo "@@NOSERVER"
else
  echo "@@NOTMUX"
fi
echo "@@CLIENTS"
tmux list-clients -F '#{session_id} #{client_control_mode} #{client_activity} #{pane_id}' 2>/dev/null
echo "@@PS"
# etimes (seconds) is Linux procps; elsewhere (macOS, BusyBox) etime is [[dd-]hh:]mm:ss
ps -e -o pid=,ppid=,etimes=,comm=,args= 2>/dev/null || ps -A -o pid=,ppid=,etime=,comm=,args= 2>/dev/null | awk '{
  n = split($3, t, /[-:]/); d = 0; h = 0
  if ($3 ~ /-/) { d = t[1]; h = t[2]; m = t[3]; s = t[4] } else if (n == 3) { h = t[1]; m = t[2]; s = t[3] } else { m = t[1]; s = t[2] }
  $3 = d * 86400 + h * 3600 + m * 60 + s; print }'
echo "@@CWD"
# no /proc on macOS: lsof knows the working directory there
for p in $(pgrep -f 'claude|codex|opencode|gemini|command-?code|agy' 2>/dev/null); do
  c=$(readlink /proc/$p/cwd 2>/dev/null) || c=$(lsof -a -p "$p" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -n 1)
  echo "$p $c"
done
echo "@@AGY"
# each Antigravity CLI holds ~/.gemini/antigravity-cli/presence/<conversation>.lock open; the newest fd wins
agy_ids=""
for p in $(pgrep -x agy 2>/dev/null); do
  if [ -d /proc/$p/fd ]; then
    l=$(for fd in /proc/$p/fd/*; do l=$(readlink "$fd" 2>/dev/null); case "$l" in */antigravity-cli/presence/*.lock) echo "${fd##*/} $p $l";; esac; done | sort -n | tail -n 1)
  else
    l=$(lsof -p $p 2>/dev/null | awk -v p=$p '/antigravity-cli/presence/.*.lock/ {print 0, p, $NF}' | tail -n 1)
  fi
  [ -n "$l" ] && echo "$l" && agy_ids="$agy_ids $(basename "${l##* }" .lock)"
done
echo "@@AGYTITLE"
# titles agy gives its conversations (shown in its /resume list)
if [ -n "$agy_ids" ] && command -v sqlite3 >/dev/null 2>&1; then
  for id in $agy_ids; do case "$id" in *[!0-9a-f-]*) continue;; esac
    echo "$id $(sqlite3 -readonly "$HOME/.gemini/antigravity-cli/conversation_summaries.db" "select title from conversation_summaries where conversation_id='$id'" 2>/dev/null | head -n 1)"
  done
fi
echo "@@AGYSTATUS"
# status files of panes that closed over an hour ago only pile up
live=" $(tmux list-panes -a -F '#{pane_id}' 2>/dev/null | tr -d '%' | tr '
' ' ') "
for f in $(find "$HOME/.inyon/status" "$HOME/.inyon/panes" "$HOME/.inyon/agy-status" -maxdepth 1 -type f -mmin +60 2>/dev/null); do
  n=$(basename "$f"); n=${n%.*}
  case "$n" in ''|*[!0-9.]*) continue;; esac
  case "$live" in *" $n "*) ;; *) rm -f "$f";; esac
done
for f in "$HOME"/.inyon/agy-status/*.json; do [ -f "$f" ] && echo "$(basename "$f" .json) $(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f") $(cat "$f")"; done 2>/dev/null
if command -v agy >/dev/null 2>&1; then
  if grep -q 'inyon/agy-statusline.sh' "$HOME/.gemini/antigravity-cli/settings.json" 2>/dev/null && { current() { v=$(sed -n 's/^# inyon-integration \([0-9][0-9]*\)$/\1/p' "$1" 2>/dev/null | head -n 1); [ -n "$v" ] && [ "$v" -ge 9 ]; }; current "$HOME/.inyon/agy-statusline.sh"; }; then echo "@@AGYINT installed"; else echo "@@AGYINT missing"; fi
fi
# the agents other than Claude Code whose settings run Inyon's hook
echo "@@HOOKED"
grep -qs 'inyon/hook.sh copilot' "${COPILOT_HOME:-$HOME/.copilot}/hooks/inyon.json" && echo copilot
grep -qs 'inyon/hook.sh gemini' "$HOME/.gemini/settings.json" && echo gemini
grep -qs 'inyon/hook.sh antigravity' "$HOME/.gemini/config/hooks.json" && echo antigravity
echo "@@HOOK"
t="$HOME/.inyon"
s="$HOME/.claude/settings.json"
current() { v=$(sed -n 's/^# inyon-integration \([0-9][0-9]*\)$/\1/p' "$1" 2>/dev/null | head -n 1); [ -n "$v" ] && [ "$v" -ge 9 ]; }
if [ ! -x "$t/hook.sh" ] || [ ! -x "$t/statusline.sh" ]; then echo missing
# settings from before the rename to Inyon still name ~/.tether: outdated, so they're rewritten
elif grep -q 'tether/hook.sh' "$s" 2>/dev/null || grep -q 'tether/statusline.sh' "$s" 2>/dev/null; then echo outdated
elif ! grep -q 'inyon/statusline.sh' "$s" 2>/dev/null || ! grep -q 'inyon/hook.sh' "$s" 2>/dev/null; then echo removed
elif current "$t/hook.sh" && current "$t/statusline.sh"; then echo installed
else echo outdated; fi
t="$HOME/.inyon"
echo "@@AUTH $(printf '%s' "$({ [ -s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/.credentials.json ] || grep -q '"oauthAccount"' "${CLAUDE_CONFIG_DIR:-$HOME}/.claude.json" 2>/dev/null; } && printf 'claude ')$({ [ -e "$HOME"/.codex/auth.json ]; } && printf 'codex ')$({ [ -e "$HOME"/.local/share/opencode/auth.json ] || [ -e "$HOME/.local/share/opencode/opencode.db" ]; } && printf 'opencode ')$({ if [ -f "$HOME/.inyon/.copilot-signed-in" ] && [ -z "$(find "$HOME/.inyon/.copilot-signed-in" -mmin +5 2>/dev/null)" ] && [ ! "$HOME/.copilot/config.json" -nt "$HOME/.inyon/.copilot-signed-in" ]; then [ "$(cat "$HOME/.inyon/.copilot-signed-in")" = 1 ]; elif node -e '
const {spawn}=require('\''node:child_process'\'');
const p=spawn('\''copilot'\'',['\''--headless'\'','\''--stdio'\'','\''--no-auto-update'\''],{stdio:['\''pipe'\'','\''pipe'\'','\''ignore'\'']});
let buffer=Buffer.alloc(0),done=false;
const timer=setTimeout(()=>finish('\''Copilot model discovery timed out'\''),18000);
function finish(error,value){if(done)return;done=true;clearTimeout(timer);p.kill();if(error){console.error(error);process.exitCode=1;}else console.log(JSON.stringify(value));}
function send(id,method){const body=Buffer.from(JSON.stringify({jsonrpc:'\''2.0'\'',id,method,params:{}}));p.stdin.write('\''Content-Length: '\''+body.length+'\''\r\n\r\n'\'');p.stdin.write(body);}
p.on('\''error'\'',()=>finish('\''Copilot CLI is not installed or could not start'\''));
p.on('\''exit'\'',()=>{if(!done)finish('\''Copilot model service exited before returning its catalog'\'')});
p.stdout.on('\''data'\'',chunk=>{buffer=Buffer.concat([buffer,chunk]);while(true){const end=buffer.indexOf('\''\r\n\r\n'\'');if(end<0)return;const length=Number(buffer.subarray(0,end).toString().match(/Content-Length:\s*(\d+)/i)?.[1]);if(!Number.isSafeInteger(length)||length<0||length>10000000)return finish('\''Invalid Copilot response'\'');if(buffer.length<end+4+length)return;let response;try{response=JSON.parse(buffer.subarray(end+4,end+4+length).toString())}catch{return finish('\''Invalid Copilot response'\'')};buffer=buffer.subarray(end+4+length);if(response.id===1){if(response.error)return finish('\''Copilot connection failed'\'');send(2,'\''auth.getStatus'\'')}else if(response.id===2){if(response.error)return finish('\''Copilot catalog unavailable; check CLI sign-in'\'');finish(response.result?.isAuthenticated ? null : '\''Copilot is not signed in'\'',null)}}});
p.stdin.on('\''error'\'',()=>finish('\''Copilot model service disconnected'\''));
send(1,'\''connect'\'');
' >/dev/null 2>&1; then mkdir -p "$HOME/.inyon" && echo 1 > "$HOME/.inyon/.copilot-signed-in"; else mkdir -p "$HOME/.inyon" && echo 0 > "$HOME/.inyon/.copilot-signed-in"; false; fi; } && printf 'copilot ')$({ [ -s "$HOME"/.gemini/oauth_creds.json ] || [ -s "$HOME"/.gemini/google_accounts.json ] || grep -qs GEMINI_API_KEY "$HOME/.gemini/.env"; } && printf 'gemini ')$({ [ -s "$HOME"/.gemini/antigravity-cli/antigravity-oauth-token ] || grep -qs '"modelProvider"' "$HOME/.gemini/antigravity-cli/settings.json"; } && printf 'antigravity ')$({ [ -e "$HOME"/.config/cursor/auth.json ]; } && printf 'cursor ')$({ [ -e "$HOME"/.grok/auth.json ]; } && printf 'grok ')$({ [ -e "$HOME"/.pi/agent/auth.json ]; } && printf 'pi ')$({ [ -e "$HOME"/.local/share/amp/secrets.json ]; } && printf 'amp ')$({ [ -e "$HOME"/.config/goose/config.yaml ]; } && printf 'goose ')$({ [ -e "$HOME"/.hermes/auth.json ] || [ -e "$HOME"/.hermes/.env ]; } && printf 'hermes ')$({ [ -e "$HOME"/.kimi-code/credentials/kimi-code.json ]; } && printf 'kimi ')$({ [ -e "$HOME"/.local/share/kiro-cli/data.sqlite3 ]; } && printf 'kiro ')$({ grep -qs '"selectedType"' "$HOME"/.qwen/settings.json; } && printf 'qwen ')$({ [ -e "$HOME"/.qoder/.auth/user ]; } && printf 'qoder ')$({ [ -e "$HOME"/.codebuddy/.credentials.json ]; } && printf 'codebuddy ')")"
echo "@@ACCOUNT $(a() { v=$(tr -d '\n\r' < "$1" 2>/dev/null | grep -o '"oauthAccount": *{[^}]*' | grep -o '"accountUuid": *"[^"]*"' | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/'); echo "${v:--}"; }; a "${CLAUDE_CONFIG_DIR:-$HOME}/.claude.json")"
echo "@@PANES"
cat "$t"/panes/*.json 2>/dev/null
echo "@@COMPACT"
for f in "$t"/panes/*.compact; do [ -f "$f" ] && echo "$(basename "$f" .compact) $(cat "$f")"; done 2>/dev/null
echo "@@CLAUDESESS"
# Claude Code windows that moved their conversation to the background, and the background sessions with their transcripts
cc="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
for f in "$cc"/sessions/*.json; do
  [ -f "$f" ] || continue
  l=$(tr -d '\n\r' < "$f")
  case "$l" in
  *'"parkedJobId":"'*) printf '%s\t%s\n' - "$l" ;;
  *'"kind":"bg"'*) s=$(printf '%s' "$l" | sed -n 's/.*"sessionId":"\([0-9a-f-]*\)".*/\1/p'); tp=$([ -n "$s" ] && ls "$cc"/projects/*/"$s".jsonl 2>/dev/null | head -n 1); printf '%s\t%s\n' "${tp:--}" "$l" ;;
  esac
done 2>/dev/null
echo "@@STATUS"
# pane, mtime, account when written, account now, payload
for f in "$t"/status/*.json; do
  [ -f "$f" ] || continue
  w=-; c=-
  if [ -f "${f%.json}.acct" ]; then read -r w cfg < "${f%.json}.acct"; c=$(a() { v=$(tr -d '\n\r' < "$1" 2>/dev/null | grep -o '"oauthAccount": *{[^}]*' | grep -o '"accountUuid": *"[^"]*"' | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/'); echo "${v:--}"; }; a "$cfg"); fi
  echo "$(basename "$f" .json) $(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f") ${w:--} ${c:--} $(cat "$f")"
done 2>/dev/null
echo "@@END"
"#;

/// Runs the probe on a machine and reads what it printed (`probe()` in tmux.ts).
pub async fn probe(host: &Host) -> Result<ProbeResult, String> {
    let t0 = now_ms();
    let r = host.exec(PROBE, ExecOptions::timeout(15000)).await.map_err(|e| e.message)?;
    let latency = now_ms().saturating_sub(t0) as f64;
    // loads the custom agents for parse_probe, which reads only what's loaded
    crate::agents::custom::list();
    parse_probe(&r.stdout, &r.stderr, r.code, latency)
}

/// Captures the visible screen of several panes in one round trip, with styling.
pub async fn capture_panes(host: &Host, pane_ids: &[String]) -> HubResult<IndexMap<String, Vec<String>>> {
    let Some(cmd) = capture_command(pane_ids) else { return Ok(IndexMap::new()) };
    let r = host.exec(&cmd, ExecOptions::timeout(10000)).await?;
    Ok(parse_captures(&r.stdout))
}

/// `typeText(host, pane, text, submit)`: types text into a pane as a person would.
pub async fn type_into(host: &Host, pane: &str, text: &str, submit: bool) -> HubResult<()> {
    match type_text(pane, text, submit, &format!("inyon-{}", crate::ids::random_uuid())) {
        TypeText::Nothing => Ok(()),
        TypeText::Keys { command, input } => {
            let r = host.exec(&command, ExecOptions::input(input)).await?;
            if r.code != 0 {
                return Err(HubError::plain(nonempty_or(r.stderr.jtrim(), "send-keys failed")));
            }
            Ok(())
        }
        TypeText::Paste { command, input, submit } => {
            let r = host.exec(&command, ExecOptions::input(input)).await?;
            if r.code != 0 {
                return Err(HubError::plain(nonempty_or(r.stderr.jtrim(), "Could not paste text into the terminal")));
            }
            if submit {
                sleep(150).await;
                send_keys(host, pane, &["Enter"]).await?;
            }
            Ok(())
        }
    }
}

/// `sendKeys(host, pane, keys)`
pub async fn send_keys(host: &Host, pane: &str, keys: &[&str]) -> HubResult<()> {
    let cmd = send_keys_command(pane, keys).map_err(HubError::plain)?;
    let r = host.run(&cmd).await?;
    if r.code != 0 {
        return Err(HubError::plain(nonempty_or(r.stderr.jtrim(), "send-keys failed")));
    }
    Ok(())
}

/// `guessTranscript(host, home, proc)`
pub async fn find_transcript(host: &Host, home: &str, proc: &TranscriptProc<'_>) -> HubResult<Option<String>> {
    let Some((dir, cmd)) = guess_transcript_command(home, proc) else { return Ok(None) };
    let r = host.run(&cmd).await?;
    Ok(guess_transcript(&r.stdout, &dir, proc))
}

fn nonempty_or(s: &str, fallback: &str) -> String {
    if s.is_empty() {
        fallback.to_owned()
    } else {
        s.to_owned()
    }
}

/// Real-time pane activity from tmux itself (tmux/live.ts). One control-mode client per tmux
/// session (read-only, and flagged so it never affects window sizes) reports `%output` for every
/// pane as it changes, plus structural events (windows/panes opened, closed, renamed).
pub struct LiveTmux {
    host: Host,
    on_output: Rc<dyn Fn(&str)>,
    on_structure: Rc<dyn Fn()>,
    clients: Rc<RefCell<IndexMap<String, StreamHandle>>>,
}

impl LiveTmux {
    pub fn new(host: Host, on_output: impl Fn(&str) + 'static, on_structure: impl Fn() + 'static) -> Self {
        LiveTmux { host, on_output: Rc::new(on_output), on_structure: Rc::new(on_structure), clients: Rc::default() }
    }

    /// Keep exactly one client per session id in `sessions`.
    pub fn sync(&self, sessions: &[String]) {
        let want: IndexSet<&str> = sessions.iter().map(String::as_str).collect();
        let gone: Vec<(String, StreamHandle)> = {
            let mut c = self.clients.borrow_mut();
            let ids: Vec<String> = c.keys().filter(|id| !want.contains(id.as_str())).cloned().collect();
            ids.into_iter().filter_map(|id| c.shift_remove(&id).map(|s| (id, s))).collect()
        };
        for (_, s) in gone {
            s.kill();
        }
        for id in want {
            if !self.clients.borrow().contains_key(id) {
                self.attach(id);
            }
        }
    }

    fn attach(&self, id: &str) {
        // exec skips the tmux() function that adds -u, so add it here (not for psmux, where tmux is a function)
        let cmd = format!(r#"exec tmux $([ "$(command -v tmux)" = tmux ] || echo -u) -C attach-session -t {} -f read-only,ignore-size"#, shq(id));
        let (on_output, on_structure, clients, sid) = (self.on_output.clone(), self.on_structure.clone(), self.clients.clone(), id.to_owned());
        let on_line = move |line: String| {
            if line.starts_with("%output ") || line.starts_with("%extended-output ") {
                if let Some(pane) = line.split(' ').nth(1).filter(|p| p.starts_with('%')) {
                    on_output(pane);
                }
            } else if re!(r"^%(window-add|window-close|unlinked-window-add|unlinked-window-close|window-renamed|layout-change|sessions-changed|session-renamed|pane-mode-changed|window-pane-changed)\b").is_match(&line) {
                on_structure();
            } else if line.starts_with("%exit") {
                clients.borrow_mut().shift_remove(&sid);
            }
        };
        let Ok(s) = self.host.stream(&cmd, on_line, true) else { return };
        self.clients.borrow_mut().insert(id.to_owned(), s.clone());
        let (clients, id) = (self.clients.clone(), id.to_owned());
        crate::platform::rt::spawn(async move {
            s.exited.wait().await;
            let mut c = clients.borrow_mut();
            if c.get(&id).is_some_and(|x| x.same(&s)) {
                c.shift_remove(&id);
            }
        });
    }

    pub fn stop(&self) {
        let all: Vec<StreamHandle> = self.clients.borrow_mut().drain(..).map(|(_, s)| s).collect();
        for s in all {
            s.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a paste never carries its own end marker, for any caller (companions.test.ts)
    #[test]
    fn a_paste_never_carries_its_own_end_marker_for_any_caller() {
        assert_eq!(paste_text("x\r\n\u{1b}[201~\u{15}!id\r"), "x\n\u{15}!id\n");
        assert_eq!(paste_text("a\u{1b}[20\u{1b}[201~1~b\u{9b}200~c"), "abc");
        let TypeText::Paste { input, .. } = type_text("%1", "x\n\u{1b}[201~\u{15}!id\r", true, "b1") else { panic!() };
        assert_eq!(input, "x\n\u{15}!id\n");
    }

    /// legacy.test.ts
    #[test]
    fn the_old_names_terminal_views_and_sign_ins_stay_out_of_the_session_list() {
        assert!(hidden_session("tether-view-ab12cd"));
        assert!(hidden_session("inyon-login-claude-x1"));
        assert!(!hidden_session("tether-work"));
    }

    /// tmux 3.4 escapes the field separator; its lines read the same as newer ones (locale.test.ts)
    #[test]
    fn tmux_3_4_escapes_the_field_separator_its_lines_read_the_same_as_newer_ones() {
        // as tmux 3.4 printed a session "a\b" holding window "tëst"
        assert_eq!(tmux_fields("a\\\\b\\037tëst\\037%0"), vec!["a\\b", "tëst", "%0"]);
        // newer tmux: the separator as it is, and a backslash left alone
        assert_eq!(tmux_fields("a\\b\u{1f}tëst\u{1f}%0"), vec!["a\\b", "tëst", "%0"]);
    }

    /// an agent whose folder was deleted is still matched to its chat (deleted-cwd.test.ts)
    #[test]
    fn an_agent_whose_folder_was_deleted_is_still_matched_to_its_chat() {
        use crate::agents::codex::{pick_codex_rollout, CodexProc, CodexRollout};
        // what readlink /proc/<pid>/cwd prints once the worktree the agent runs in is removed
        let cwds = proc_cwds(&["4242 /home/u/wt/review (deleted)".into(), "4243 /home/u/my (deleted) notes".into()]);
        assert_eq!(cwds.get(&4242).map(String::as_str), Some("/home/u/wt/review"));
        // only the mark Linux adds at the end comes off
        assert_eq!(cwds.get(&4243).map(String::as_str), Some("/home/u/my (deleted) notes"));

        let proc = CodexProc { pid: 4242, cwd: cwds.get(&4242).cloned(), started_at: 1000.0, ..Default::default() };
        let rows = vec![CodexRollout { path: "/s/rollout-a.jsonl".into(), id: Some("a".into()), cwd: Some("/home/u/wt/review".into()), created: 1010.0, mtime: 2000.0 }];
        assert_eq!(pick_codex_rollout(&rows, &proc, std::slice::from_ref(&proc), &HashMap::new()), Some("/s/rollout-a.jsonl".into()));
    }

    #[test]
    fn the_probe_output_reads_into_sessions_and_agents() {
        let sep = "\u{1f}";
        let pane = |sid: &str, sname: &str, pid: &str, ppid: &str| {
            [
                "$".to_owned() + sid,
                sname.into(),
                "2".into(),
                "@1".into(),
                "0".into(),
                "main".into(),
                "1".into(),
                "1700000000".into(),
                pid.into(),
                "0".into(),
                "1".into(),
                ppid.into(),
                "zsh".into(),
                "/home/u/p".into(),
                "t".into(),
                "120".into(),
                "40".into(),
                "0".into(),
            ]
            .join(sep)
        };
        let out = [
            "@@HOST box ".to_owned(),
            "@@HOME /home/u".into(),
            "@@OS Linux".into(),
            "@@NOW 2000".into(),
            "@@KIND Linux|1|vm|kvm|0|0|KVM|".into(),
            "@@BINS".into(),
            "claude".into(),
            "@@TMUX".into(),
            pane("1", "work", "%1", "100"),
            pane("2", "inyon-view-x", "%2", "200"),
            "@@CLIENTS".into(),
            "$1 0 1990 %1".into(),
            // Inyon's own control-mode client: neither attached nor typing
            "$1 1 1999 %1".into(),
            "@@PS".into(),
            "  100     1   50 zsh -zsh".into(),
            "  101   100   40 /usr/bin/claude claude --resume 7409eef9-d507-4808-a592-0bfabe896e3a".into(),
            "@@CWD".into(),
            "101 /home/u/p/sub".into(),
            "@@HOOK".into(),
            "installed".into(),
            "@@AUTH claude codex".into(),
            "@@ACCOUNT -".into(),
            "@@PANES".into(),
            r#"{"pane":"%1","ts":5,"data":{"session_id":"s"}}"#.into(),
            "@@COMPACT".into(),
            "1 1990".into(),
            "@@STATUS".into(),
            r#"1 1995 A - {"session_id":"s"}"#.into(),
            "@@END".into(),
        ]
        .join("\n");
        let r = parse_probe(&out, "", 0, 12.0).unwrap();
        assert_eq!((r.hostname.as_str(), r.os.as_str(), r.now), ("box", "Linux", 2000.0));
        assert_eq!(r.machine_kind, Some(MachineKind::Server));
        assert_eq!(r.installed, vec![AgentKind::Claude]);
        assert_eq!(r.signed_in, vec![AgentKind::Claude, AgentKind::Codex]);
        assert_eq!(r.integration, IntegrationState::Installed);
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].attached, inyon_protocol::Num(1.0));
        assert_eq!(r.keys_at.iter().collect::<Vec<_>>(), [(&"%1".to_owned(), &1990.0)]);
        // two people on the pane: the later key counts; psmux prints no time or pane
        let two = parse_probe(&out.replace("$1 0 1990 %1", "$1 0 1990 %1\n$1 0 1995 %1\n$1 0 1980 %1"), "", 0, 12.0).unwrap();
        assert_eq!((two.keys_at["%1"], two.sessions[0].attached), (1995.0, inyon_protocol::Num(3.0)));
        let psmux = parse_probe(&out.replace("$1 0 1990 %1", "$1 0"), "", 0, 12.0).unwrap();
        assert_eq!((psmux.keys_at.len(), psmux.sessions[0].attached), (0, inyon_protocol::Num(1.0)));
        // review: a width tmux didn't print as a number is NaN, null on the wire, as Number() gives
        assert_eq!(out.matches("120").count(), 2);
        let odd = parse_probe(&out.replacen("120", "wide", 1), "", 0, 12.0).unwrap();
        let pane = &odd.sessions[0].windows[0].panes[0];
        assert!(pane.width.0.is_nan());
        assert_eq!(serde_json::to_value(pane).unwrap()["width"], serde_json::Value::Null);
        // tmux older than the window size fields: nothing to rest
        assert!(r.unwatched.is_empty());
        let a = &r.agent_procs["%1"];
        assert_eq!((a.kind, a.pid, a.started_at, a.cwd.as_deref()), (AgentKind::Claude, 101.0, 1960.0, Some("/home/u/p/sub")));
        assert_eq!(a.resume.as_deref(), Some("7409eef9-d507-4808-a592-0bfabe896e3a"));
        assert_eq!(r.hook_panes["%1"].data["session_id"], "s");
        assert!(r.claude_parked.is_empty());
        // Claude Code moved the window's conversation to a background session, which the window
        // now shows: that session's transcript, and none of the hook's word on the one it left
        let sessions = [
            "@@CLAUDESESS",
            "-\t{\"pid\":101,\"sessionId\":\"s\",\"kind\":\"interactive\",\"tmux\":\"work:@1.%1\",\"parkedJobId\":\"eef2ead0\"}",
            "/home/u/.claude/projects/-home-u-p/eef2ead0-8d08.jsonl\t{\"pid\":102,\"sessionId\":\"eef2ead0-8d08\",\"kind\":\"bg\",\"jobId\":\"eef2ead0\"}",
            "@@STATUS",
        ];
        let moved = out.replace("@@STATUS", &sessions.join("\n"));
        let running = parse_probe(&moved.replace("@@CWD", "  102     1   30 claude claude --bg\n@@CWD"), "", 0, 12.0).unwrap();
        let want = ParkedClaude { session_id: "eef2ead0-8d08".into(), transcript: "/home/u/.claude/projects/-home-u-p/eef2ead0-8d08.jsonl".into() };
        assert_eq!(running.claude_parked.get(&101), Some(&want));
        assert!(!running.hook_panes.contains_key("%1"));
        // once the background session's own events reach the window, they stay
        let routed =
            parse_probe(&moved.replace("@@CWD", "  102     1   30 claude claude --bg\n@@CWD").replace(r#""session_id":"s"}}"#, r#""session_id":"eef2ead0-8d08"}}"#), "", 0, 12.0)
                .unwrap();
        assert_eq!(routed.hook_panes["%1"].data["session_id"], "eef2ead0-8d08");
        // a background session that has ended (its record left behind) moves nothing
        let ended = parse_probe(&moved, "", 0, 12.0).unwrap();
        assert!(ended.claude_parked.is_empty());
        assert_eq!(ended.hook_panes["%1"].data["session_id"], "s");
        assert_eq!(r.compact_panes["%1"], 1990.0);
        assert_eq!((r.status_panes["%1"].account.as_deref(), r.status_panes["%1"].account_now.as_deref()), (Some("A"), None));
        // a custom agent (Settings › Agents) is the agent it works like, under its own name
        use crate::agents::custom::{set, CustomAgent};
        set(vec![CustomAgent { id: "oc".into(), name: "OpenClaude".into(), command: "openclaude".into(), like: AgentKind::Claude }]);
        let oc = parse_probe(&out.replace("/usr/bin/claude claude", "openclaude openclaude"), "", 0, 12.0).unwrap();
        assert_eq!((oc.agent_procs["%1"].kind, oc.agent_procs["%1"].custom.as_deref()), (AgentKind::Claude, Some("OpenClaude")));
        assert_eq!(r.agent_procs["%1"].custom, None);
        // review: a saved wrapper that starts the agent without exec is followed to the agent
        set(vec![CustomAgent { id: "w".into(), name: "Wrapped".into(), command: "run-claude".into(), like: AgentKind::Claude }]);
        let wrapped = parse_probe(
            &out.replace("  101   100   40 /usr/bin/claude claude", "  102   100   45 run-claude /bin/sh /home/u/bin/run-claude\n  101   102   40 /usr/bin/claude claude"),
            "",
            0,
            12.0,
        )
        .unwrap();
        let w = &wrapped.agent_procs["%1"];
        assert_eq!((w.pid, w.kind, w.custom.as_deref(), w.resume.as_deref()), (101.0, AgentKind::Claude, Some("Wrapped"), Some("7409eef9-d507-4808-a592-0bfabe896e3a")));
        set(vec![]);
        assert_eq!(parse_probe("oops\nfirst\nsecond", "", 1, 0.0), Err("first second".into()));
    }

    #[test]
    fn captures_split_by_pane_and_commands_are_quoted() {
        assert_eq!(capture_command(&[]), None);
        assert_eq!(capture_command(&["%1".into()]).unwrap(), "echo '@@CAP %1'; tmux capture-pane -p -e -t '%1' 2>/dev/null");
        let c = parse_captures("junk\n@@CAP %1\na\nb\n@@CAP %2\n");
        assert_eq!(c["%1"], vec!["a", "b"]);
        assert_eq!(c["%2"], vec![""]);
        assert_eq!(
            type_text("%1", "hi", true, "b"),
            TypeText::Keys { command: format!("{} && sleep 0.12 && tmux send-keys -t '%1' Enter", type_command("b", "%1")), input: "hi".into() }
        );
        assert_eq!(type_text("%1", "", true, "b"), TypeText::Keys { command: "sleep 0.12 && tmux send-keys -t '%1' Enter".into(), input: String::new() });
        assert_eq!(type_text("%1", "", false, "b"), TypeText::Nothing);
        assert!(matches!(type_text("%1", "a\r\nb", false, "b"), TypeText::Paste { input, .. } if input == "a\nb"));
        assert_eq!(send_keys_command("%1", &["Enter", "rm"]), Err("key not allowed: rm".into()));
        assert_eq!(claude_slug("/home/u/my.app"), "-home-u-my-app");
    }

    #[test]
    fn typed_text_never_reaches_a_command_line() {
        // other users on the machine can read every process's arguments: messages, drafts and API keys go on stdin
        let samples = ["a pasted API key", "hello world", "it's \"quoted\" $HOME `id`", "ünïcødé 🙂", &"x".repeat(799), &"y".repeat(900), "two\nlines"];
        for text in samples {
            match type_text("%1", text, true, "inyon-b") {
                TypeText::Keys { command, input } => {
                    assert!(!command.contains(text), "{text} in {command}");
                    assert_eq!(input, text);
                }
                TypeText::Paste { command, input, .. } => {
                    assert!(!command.contains(text));
                    assert_eq!(input, paste_text(text));
                }
                TypeText::Nothing => panic!("{text}"),
            }
        }
        // typed, not pasted: no paste markers around it
        assert!(type_command("b", "%1").contains("paste-buffer -d -b 'b'"));
        assert!(paste_command("b", "%1").contains("paste-buffer -d -p -b 'b'"));
    }

    #[test]
    fn a_transcript_is_guessed_from_the_resume_id_or_the_newest_file() {
        let p = TranscriptProc { cwd: Some("/p"), started_at: 100.0, resume: None };
        let (dir, _) = guess_transcript_command("/h", &p).unwrap();
        assert_eq!(dir, "/h/.claude/projects/-p");
        assert_eq!(guess_transcript("200 new.jsonl\n50 old.jsonl\n", &dir, &p), Some("/h/.claude/projects/-p/new.jsonl".into()));
        assert_eq!(guess_transcript("50 old.jsonl\n", &dir, &p), None);
        let r = TranscriptProc { resume: Some("r"), ..p };
        assert_eq!(guess_transcript("", &dir, &r), Some("/h/.claude/projects/-p/r.jsonl".into()));
    }

    #[test]
    fn windows_no_person_is_attached_to_are_listed_with_their_size() {
        let sep = "\u{1f}";
        let row = |sid: &str, sname: &str, attached: &str, wid: &str, pane: &str, size: &str| {
            [sid, sname, attached, wid, "1", "w", "1", "0", pane, "0", "1", "100", "zsh", "/h", "t", "80", "24", "0"].join(sep) + &format!("{sep}{}", size.replace('x', sep))
        };
        let out = [
            "@@HOST box".to_owned(),
            "@@OS Linux".into(),
            "@@NOW 2000".into(),
            "@@TMUX".into(),
            // laptop attached to $1, which holds @1
            row("$1", "work", "1", "@1", "%1", "120x29"),
            // $2 is only Inyon's control client: @2 counts as unwatched
            row("$2", "bg", "1", "@2", "%2", "80x24"),
            // @3 sits in a detached session, but a phone's view session links it
            row("$3", "other", "0", "@3", "%3", "40x20"),
            row("$9", "inyon-view-ab12cd", "1", "@3", "%3", "40x20"),
            // a size tmux didn't print: skipped
            row("$4", "old", "0", "@4", "%4", "x"),
            "@@CLIENTS".into(),
            "$1 0 1990 %1".into(),
            "$2 1 1999 %2".into(),
            "$9 0 1995 %3".into(),
            "@@END".into(),
        ]
        .join("\n");
        let r = parse_probe(&out, "", 0, 12.0).unwrap();
        assert_eq!(r.unwatched.into_iter().collect::<Vec<_>>(), [("@2".to_owned(), (80, 24))]);
        // psmux lists no clients: tmux's own attached count decides
        let psmux = parse_probe(&out.replace("@@CLIENTS\n$1 0 1990 %1\n$2 1 1999 %2\n$9 0 1995 %3\n", ""), "", 0, 12.0).unwrap();
        assert!(psmux.unwatched.is_empty(), "{:?}", psmux.unwatched);
    }
}
