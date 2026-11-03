//! Port of features/signin.ts: an agent's sign-in, run in its own hidden tmux session so the app
//! can drive it (read the link and the device code, pick from its menus, paste the code back).

use std::cell::RefCell;
use std::collections::HashSet;

use indexmap::{IndexMap, IndexSet};
use inyon_protocol::{AgentKind, PromptOption, ScreenPrompt, AGENT_KINDS};
use serde::Serialize;
use serde_json::Value;

use super::setup::PATHS;
use crate::claude::actions::shell_ready;
use crate::claude::screen::parse_screen;
use crate::clock::now_ms;
use crate::hosts::host::{shq, Host};
use crate::hosts::http_error::{HubError, HubResult};
use crate::hosts::registry::registry;
use crate::js::json::value_string;
use crate::js::text::JsStr;
use crate::js::to_number;
use crate::platform::rt::{sleep, spawn};
use crate::re;
use crate::tmux::tmux::{send_keys, type_into, LOGIN_PREFIX};

/// Sign-in commands that work without a browser on the machine: a device code (Codex) or a link
/// whose page hands back a code to paste (Claude Code, OpenCode, Gemini CLI).
fn sign_in(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "claude auth login",
        AgentKind::Codex => "codex login --device-auth",
        AgentKind::Opencode => "opencode auth login",
        AgentKind::Gemini => "NO_BROWSER=true gemini",
        AgentKind::Copilot => "copilot login --device-code",
        // signs in on first run: a menu, then a link and a prompt for the code the page shows. It runs
        // in an empty folder (so it never asks to trust your home) and is closed once the token lands.
        AgentKind::Antigravity => {
            r#"F="$HOME/.gemini/antigravity-cli/antigravity-oauth-token"; d=$(mktemp -d); cd "$d"; ( while [ ! -s "$F" ]; do sleep 1; done; sleep 2; pkill -P $$ -x agy ) & agy; cd /; rmdir "$d" 2>/dev/null; test -s "$F""#
        }
        // the others: their own sign-in command from the catalog (a device code where they have one)
        _ => &kind.def().login,
    }
}

/// The same on Windows, typed into PowerShell: psmux panes run PowerShell, and there's no sh to
/// hand a script to. Antigravity stays open after signing in there; closing the sign-in ends it.
fn sign_in_windows_cmd(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Gemini => "$env:NO_BROWSER='true'; gemini",
        AgentKind::Antigravity => "agy",
        _ => sign_in(kind),
    }
}

/// A menu on the sign-in's screen (e.g. "Select login method"), to answer with `{ option }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignInMenu {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    pub options: Vec<PromptOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignInInput {
    pub label: String,
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignInScreen {
    pub lines: Vec<String>,
    pub links: Vec<String>,
    /// a device code to type on the sign-in page, e.g. ABCD-12345
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// set once the command has finished (non-zero when it printed an error, whatever it exited with)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i64>,
    /// what the command said went wrong ("Unknown provider …")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// a menu on screen (e.g. "Select login method"), to answer with `{ option }`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<SignInMenu>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<SignInInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignInStart {
    pub pane: String,
}

struct Watched {
    host_id: String,
    pane: String,
    seen: u64,
}

/// sign-ins the app is watching, and when it last looked; one nobody looks at is closed
const ABANDONED_MS: u64 = 3 * 60_000;

thread_local! {
    static WATCHED: RefCell<IndexMap<String, Watched>> = RefCell::new(IndexMap::new());
    /// sign-ins whose end was already acted on
    static FINISHED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Closes the sign-ins nobody has looked at for a few minutes, checked every minute.
pub fn start() {
    spawn(async {
        loop {
            sleep(60_000).await;
            let gone: Vec<(String, String)> = WATCHED.with(|w| {
                let mut w = w.borrow_mut();
                let keys: Vec<String> = w.iter().filter(|(_, v)| now_ms().saturating_sub(v.seen) >= ABANDONED_MS).map(|(k, _)| k.clone()).collect();
                keys.iter().filter_map(|k| w.shift_remove(k)).map(|v| (v.host_id, v.pane)).collect()
            });
            for (host_id, pane) in gone {
                spawn(async move {
                    let _ = stop_signin(&host_id, &pane).await;
                });
            }
        }
    });
}

fn seen(host_id: &str, pane: &str) {
    WATCHED.with(|w| w.borrow_mut().insert(format!("{host_id} {pane}"), Watched { host_id: host_id.to_owned(), pane: pane.to_owned(), seen: now_ms() }));
}

/// The sign-in command; for OpenCode, `provider` skips its provider menu and goes straight to that one's.
pub fn sign_in_command(kind: AgentKind, provider: Option<&str>, windows: bool) -> String {
    match provider {
        Some(p) if kind == AgentKind::Opencode && !p.is_empty() => format!("opencode auth login -p {}", shq(p)),
        _ if windows => sign_in_windows_cmd(kind).to_owned(),
        _ => sign_in(kind).to_owned(),
    }
}

/// Runs the agent's sign-in in its own hidden tmux session, so it can be driven from the app. The
/// session ends by itself an hour after the command finishes.
pub async fn start_signin(host_id: &str, kind: &str, provider: Option<&str>) -> HubResult<SignInStart> {
    let kind = AGENT_KINDS.iter().copied().find(|k| k.as_str() == kind).ok_or_else(|| HubError::http(400, "Unknown agent"))?;
    if let Some(p) = provider {
        if !re!(r"^[\w.-]{1,64}$").is_match(p) {
            return Err(HubError::http(400, "Unknown provider"));
        }
    }
    let host = registry().host(host_id)?;
    let random: String = crate::ids::random_uuid().chars().filter(|c| *c != '-').take(5).collect();
    let name = format!("{LOGIN_PREFIX}{}-{random}", kind.as_str());
    let windows = registry().get(host_id).is_some_and(|rt| rt.borrow().state.os.as_deref() == Some("Windows"));
    let script = format!("{PATHS}; {}; s=$?; echo; echo \"[inyon-exit $s]\"; exec sleep 3600", sign_in_command(kind, provider, false));
    // wide, so links wrap as little as possible; capture -J re-joins what still wraps
    let run = if windows { String::new() } else { format!(" {}", shq(&format!("sh -c {}", shq(&script)))) };
    let r = host.run(&format!("tmux new-session -d -s {} -x 220 -y 50 -P -F '#{{pane_id}}'{run}", shq(&name))).await?;
    let pane = r.stdout.jtrim().to_owned();
    if r.code != 0 || !re!(r"^%\d+(\.\d+)?$").is_match(&pane) {
        let err = r.stderr.jtrim();
        return Err(HubError::http(500, if err.is_empty() { "Could not start the sign-in" } else { err }));
    }
    if windows {
        shell_ready(&host, &pane, 8000).await?;
        type_into(&host, &pane, &sign_in_windows(kind, provider), true).await?;
    }
    seen(host_id, &pane);
    Ok(SignInStart { pane })
}

/// The PowerShell line for a sign-in on Windows; it prints the same exit mark as the sh script.
pub fn sign_in_windows(kind: AgentKind, provider: Option<&str>) -> String {
    format!("{}; Write-Output ''; Write-Output \"[inyon-exit $([int]$LASTEXITCODE)]\"", sign_in_command(kind, provider, true))
}

struct Owned {
    host: Host,
    sign_in_kind: Option<AgentKind>,
    session: String,
}

async fn own_pane(host_id: &str, pane: &str) -> HubResult<Owned> {
    if !re!(r"^%\d+(\.\d+)?$").is_match(pane) {
        return Err(HubError::http(400, "Bad pane"));
    }
    let host = registry().host(host_id)?;
    let r = host.run(&format!("tmux display-message -p -t {} '#{{session_name}}' 2>/dev/null", shq(pane))).await?;
    let session = r.stdout.jtrim().to_owned();
    if !session.starts_with(LOGIN_PREFIX) {
        return Err(HubError::http(404, "That sign-in has ended"));
    }
    // the session is named inyon-login-<agent>-<random>
    let sign_in_kind = AGENT_KINDS.iter().copied().find(|k| session.starts_with(&format!("{LOGIN_PREFIX}{}-", k.as_str())));
    Ok(Owned { host, sign_in_kind, session })
}

/// OpenCode's Clack provider/auth prompts have radio bullets, not numbered choices.
/// Only expose the current visible window; arrow navigation uses its selected row.
pub fn parse_open_code_auth_menu(lines: &[String]) -> Option<ScreenPrompt> {
    let start = lines.iter().rposition(|line| re!(r"^\s*[◆◇]\s+").is_match(line))?;
    if !re!(r"^\s*◆").is_match(&lines[start]) {
        return None;
    }
    let mut options: Vec<PromptOption> = vec![];
    for line in &lines[start + 1..] {
        if let Some(m) = re!(r"^\s*[│┃]?\s*([●○◯])\s+(.+?)\s*$").captures(line) {
            options.push(PromptOption { n: options.len() as u64 + 1, label: m[2].to_owned(), detail: None, selected: &m[1] == "●", checked: None, current: None });
        }
    }
    if options.is_empty() || !options.iter().any(|o| o.selected) {
        return None;
    }
    let heading = re!(r"^\s*◆\s*").replace(&lines[start], "").jtrim().to_owned();
    serde_json::from_value(serde_json::json!({ "body": [heading], "options": options, "multi": false, "navigate": true })).ok()
}

/// The menu on the sign-in's visible screen, read the way chat reads the agent's own menus.
async fn sign_in_menu(owned: &Owned, pane: &str) -> HubResult<Option<ScreenPrompt>> {
    let r = owned.host.run(&format!("tmux display -p -t {} '#{{pane_width}}'; tmux capture-pane -p -t {}", shq(pane), shq(pane))).await?;
    let mut parts = r.stdout.split('\n');
    let width = parts.next().unwrap_or("");
    let lines: Vec<String> = parts.map(str::to_owned).collect();
    if owned.sign_in_kind == Some(AgentKind::Opencode) {
        if let Some(menu) = parse_open_code_auth_menu(&lines) {
            return Ok(Some(menu));
        }
    }
    let w = to_number(width);
    let w = if w.is_nan() || w == 0.0 { None } else { Some(w as u32) };
    let p = parse_screen(&lines, w, owned.sign_in_kind.unwrap_or(AgentKind::Claude)).prompt;
    Ok(p.filter(|p| p.input.is_none() && p.options.len() >= 2))
}

pub async fn read_signin(host_id: &str, pane: &str) -> HubResult<SignInScreen> {
    let owned = own_pane(host_id, pane).await?;
    seen(host_id, pane);
    let r = owned.host.run(&format!("tmux capture-pane -p -J -S -200 -t {}", shq(pane))).await?;
    let mut screen = parse_signin(&r.stdout);
    if screen.exit.is_some() {
        registry().refresh(host_id, 0);
        // a provider connected (or not): OpenCode's lists on this machine are read again
        let key = format!("{host_id} {pane}");
        if owned.sign_in_kind == Some(AgentKind::Opencode) && !FINISHED.with(|f| f.borrow().contains(&key)) {
            FINISHED.with(|f| f.borrow_mut().insert(key));
            let host_id = host_id.to_owned();
            spawn(async move {
                let _ = super::providers::after_credential_change(&host_id).await;
            });
        }
    } else if let Some(p) = sign_in_menu(&owned, pane).await? {
        let question = p.body.iter().map(|l| l.jtrim()).rfind(|l| !l.is_empty() && !re!(r"^[╌─]+$").is_match(l)).map(str::to_owned);
        screen.menu = Some(SignInMenu { question, options: p.options });
    } else if owned.sign_in_kind == Some(AgentKind::Opencode) {
        screen.input = parse_open_code_auth_input(&screen.lines);
    }
    Ok(screen)
}

pub fn parse_open_code_auth_input(lines: &[String]) -> Option<SignInInput> {
    let heading = lines.iter().rev().find(|line| re!(r"^\s*[◆◇]\s+").is_match(line))?;
    if !re!(r"^\s*◆").is_match(heading) {
        return None;
    }
    let label = re!(r"^\s*◆\s*").replace(heading, "").jtrim().to_owned();
    if !re!(r"\b(?:enter|paste)\b", "i").is_match(&label) {
        return None;
    }
    let secret = re!(r"\b(?:key|token|password|secret)\b", "i").is_match(&label);
    Some(SignInInput { label, secret })
}

/// Full-screen sign-ins (agy) draw a long link across several lines of their own, which `capture -J`
/// can't re-join: a line that ends in a link and fills the width continues on the next unbroken line.
fn rejoin_links(lines: &[String]) -> Vec<String> {
    let width = lines.iter().map(|l| l.len16()).max().unwrap_or(0) as isize;
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        let mut line = lines[i].clone();
        if re!(r"https?://\S+$").is_match(&line) && line.len16() as isize >= width - 3 {
            while i + 1 < lines.len() && re!(r"^\s*[^\s─━]\S*$").is_match(&lines[i + 1]) && !re!(r"^\s*https?:").is_match(&lines[i + 1]) {
                i += 1;
                let next = &lines[i];
                line.push_str(next.jtrim());
                if (next.len16() as isize) < width - 3 {
                    break;
                }
            }
        }
        out.push(line);
        i += 1;
    }
    out
}

pub fn parse_signin(captured: &str) -> SignInScreen {
    let lines: Vec<String> = captured.split('\n').map(|l| l.jtrim_end().to_owned()).collect();
    let mut exit: Option<i64> = None;
    let mut shown: Vec<String> = lines
        .into_iter()
        .filter(|l| match re!(r"^\[inyon-exit (\d+)\]$").captures(l.jtrim()) {
            Some(m) => {
                exit = Some(to_number(&m[1]) as i64);
                false
            }
            None => true,
        })
        .collect();
    while shown.last().is_some_and(|l| l.is_empty()) {
        shown.pop();
    }
    let lead = shown.iter().take_while(|l| l.is_empty()).count();
    shown.drain(..lead);
    let text = rejoin_links(&shown).join("\n");
    let links: IndexSet<String> = re!(r#"https?://[^\s"'<>]+"#).find_iter(&text).map(|m| re!(r"[).,;]+$").replace(m.as_str(), "").into_owned()).collect();
    let code = re!(r"\b[A-Z0-9]{4,5}-[A-Z0-9]{4,5}\b").find(&re!(r"https?://\S+").replace_all(&text, "")).map(|m| m.as_str().to_owned());
    // OpenCode's sign-in prints "Error: Unknown provider …" and still exits 0
    let error = shown.iter().find_map(|l| re!(r"^(?:[│■]\s+)?Error:\s+(.+)$").captures(l.jtrim()).map(|m| m[1].to_owned()).filter(|e| !e.is_empty()));
    if error.is_some() && exit == Some(0) {
        exit = Some(1);
    }
    let from = shown.len().saturating_sub(60);
    SignInScreen { lines: shown[from..].to_vec(), links: links.into_iter().collect(), code, exit, error, menu: None, input: None }
}

/// Types into the sign-in (a pasted code), or presses keys to move through its menus.
pub async fn answer_signin(host_id: &str, pane: &str, b: &Value) -> HubResult<()> {
    let owned = own_pane(host_id, pane).await?;
    let host = &owned.host;
    if let Some(option) = b.get("option").and_then(Value::as_f64) {
        let p = sign_in_menu(&owned, pane).await?;
        let Some(p) = p.filter(|p| p.options.iter().any(|o| o.n as f64 == option)) else {
            return Err(HubError::http(409, "That choice is no longer on screen"));
        };
        if p.navigate == Some(true) {
            let delta = option - p.options.iter().find(|o| o.selected).map_or(1.0, |o| o.n as f64);
            let mut keys = vec![if delta > 0.0 { "Down" } else { "Up" }; delta.abs() as usize];
            keys.push("Enter");
            send_keys(host, pane, &keys).await?;
        } else {
            send_keys(host, pane, &[&crate::js::fmt_number(option)]).await?;
        }
    } else if let Some(text) = b.get("text").and_then(Value::as_str).map(|t| t.jtrim()).filter(|t| !t.is_empty()) {
        type_into(host, pane, text, true).await?;
    } else if let Some(keys) = b.get("keys").and_then(Value::as_array).filter(|k| k.len() <= 10) {
        let keys: Vec<String> = keys.iter().map(value_string).collect();
        send_keys(host, pane, &keys.iter().map(String::as_str).collect::<Vec<_>>()).await?;
    } else {
        return Err(HubError::http(400, "Nothing to send"));
    }
    Ok(())
}

pub async fn stop_signin(host_id: &str, pane: &str) -> HubResult<()> {
    WATCHED.with(|w| w.borrow_mut().shift_remove(&format!("{host_id} {pane}")));
    let Owned { host, session, .. } = own_pane(host_id, pane).await?;
    // the session checked above, by its exact name: asking tmux again could come back empty if the
    // pane has just gone, and kill-session with an empty target ends the most recent session instead
    host.run(&format!("tmux kill-session -t {} 2>/dev/null; true", shq(&format!("={session}")))).await?;
    registry().refresh(host_id, 0);
    Ok(())
}

#[cfg(test)]
#[path = "signin_tests.rs"]
mod tests;
