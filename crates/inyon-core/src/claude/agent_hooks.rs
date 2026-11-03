//! Inyon's hook in the agents besides Claude Code that run hooks: GitHub Copilot CLI (a file of
//! its own in ~/.copilot/hooks), Gemini CLI (~/.gemini/settings.json, shaped like Claude Code's)
//! and Antigravity CLI (a named entry in ~/.gemini/config/hooks.json). Each calls
//! `~/.inyon/hook.sh <agent> <event>`, which logs the event for the pane and answers `{}`, so the
//! hub hears at once when a turn starts or ends instead of at the next screen read.
//!
//! Codex isn't here: it runs hooks in a background process that can't tell which pane they're
//! for, and they only run once trusted, which writes ~/.codex/config.toml. OpenCode's own server
//! already streams its events.

use inyon_protocol::AgentKind;
use serde_json::{json, Map, Value};

use crate::hosts::host::{ExecOptions, Host};
use crate::hosts::http_error::{HubError, HubResult};
use crate::js::json::{self, stringify_pretty, Js};
use crate::js::JsStr;
use crate::platform::console;

/// The agents this installs into, in the probe's order.
pub const HOOKED_AGENTS: [AgentKind; 3] = [AgentKind::Copilot, AgentKind::Gemini, AgentKind::Antigravity];

const COPILOT_HOME: &str = r#""${COPILOT_HOME:-$HOME/.copilot}""#;
const COPILOT_FILE: &str = r#""${COPILOT_HOME:-$HOME/.copilot}/hooks/inyon.json""#;
/// Not permissionRequest: an answer there decides the permission. Copilot says it's asking as a
/// notification (permission_prompt) too.
const COPILOT_EVENTS: [&str; 6] = ["sessionStart", "sessionEnd", "userPromptSubmitted", "agentStop", "notification", "errorOccurred"];

const GEMINI_FILE: &str = r#""$HOME/.gemini/settings.json""#;
const GEMINI_EVENTS: [&str; 5] = ["SessionStart", "SessionEnd", "BeforeAgent", "AfterAgent", "Notification"];

const AGY_FILE: &str = r#""$HOME/.gemini/config/hooks.json""#;
const AGY_EVENTS: [&str; 2] = ["PreInvocation", "Stop"];
/// The name of Inyon's entry among agy's named hooks.
const AGY_NAME: &str = "inyon";

/// The command an agent's settings run for one of its events. Without the script (Inyon removed,
/// or a hub that doesn't know these entries turned the integration off) it answers `{}` and does
/// nothing, so a leftover entry never fails the agent's hook.
pub fn hook_command(agent: AgentKind, event: &str) -> String {
    format!("test -x ~/.inyon/hook.sh && ~/.inyon/hook.sh {} {event} || echo '{{}}'", agent.as_str())
}

fn ours(agent: AgentKind, command: Option<&Value>) -> bool {
    command.str().is_some_and(|c| c.contains(&format!(".inyon/hook.sh {}", agent.as_str())))
}

/// Copilot's hook file: all Inyon's, so it's written whole.
pub fn copilot_hooks() -> Value {
    let hooks: Map<String, Value> =
        COPILOT_EVENTS.iter().map(|e| ((*e).to_owned(), json!([{ "type": "command", "bash": hook_command(AgentKind::Copilot, e), "timeoutSec": 10 }]))).collect();
    json!({ "version": 1, "hooks": hooks })
}

/// Gemini CLI's settings with Inyon's hook on each event, leaving every other hook as it was.
pub fn with_gemini_hooks(mut settings: Map<String, Value>) -> Result<Map<String, Value>, String> {
    if settings.get("hooks").is_none_or(Value::is_null) {
        settings.insert("hooks".into(), Value::Object(Map::new()));
    }
    let Some(Value::Object(hooks)) = settings.get_mut("hooks") else { return Err("\"hooks\" in ~/.gemini/settings.json isn't an object".into()) };
    for e in GEMINI_EVENTS {
        if hooks.get(e).is_none_or(Value::is_null) {
            hooks.insert(e.into(), Value::Array(vec![]));
        }
        let Some(Value::Array(groups)) = hooks.get_mut(e) else { return Err(format!("the {e} hooks in ~/.gemini/settings.json aren't a list")) };
        if groups.iter().any(|g| Some(g).at("hooks").arr().is_some_and(|hs| hs.iter().any(|h| ours(AgentKind::Gemini, Some(h).at("command"))))) {
            continue;
        }
        // Gemini CLI's timeouts are in milliseconds
        groups.push(json!({ "hooks": [{ "name": "inyon", "type": "command", "command": hook_command(AgentKind::Gemini, e), "timeout": 10000 }] }));
    }
    Ok(settings)
}

/// Gemini CLI's settings without Inyon's hook, and None when it had none.
pub fn without_gemini_hooks(mut settings: Map<String, Value>) -> Option<Map<String, Value>> {
    let Some(Value::Object(hooks)) = settings.get_mut("hooks") else { return None };
    let mut changed = false;
    for name in hooks.keys().cloned().collect::<Vec<_>>() {
        let Some(Value::Array(groups)) = hooks.get(&name) else { continue };
        let mut kept = vec![];
        for g in groups {
            let hs = Some(g).at("hooks").arr().cloned().unwrap_or_default();
            let left: Vec<Value> = hs.iter().filter(|h| !ours(AgentKind::Gemini, Some(*h).at("command"))).cloned().collect();
            changed |= left.len() != hs.len();
            if !left.is_empty() {
                let mut m = g.as_object().cloned().unwrap_or_default();
                m.insert("hooks".into(), Value::Array(left));
                kept.push(Value::Object(m));
            } else if hs.is_empty() {
                kept.push(g.clone());
            }
        }
        if kept.is_empty() {
            hooks.shift_remove(&name);
        } else {
            hooks.insert(name, Value::Array(kept));
        }
    }
    if hooks.is_empty() {
        settings.shift_remove("hooks");
    }
    changed.then_some(settings)
}

/// agy's named hooks with Inyon's entry in them (replacing an older one).
pub fn with_agy_hooks(mut hooks: Map<String, Value>) -> Map<String, Value> {
    let mut entry = Map::new();
    for e in AGY_EVENTS {
        entry.insert(e.into(), json!([{ "type": "command", "command": hook_command(AgentKind::Antigravity, e), "timeout": 10 }]));
    }
    hooks.insert(AGY_NAME.into(), Value::Object(entry));
    hooks
}

fn pretty(v: &Value) -> String {
    format!("{}\n", stringify_pretty(v))
}

/// A JSON object file on the machine, when `present` (a shell test) says its agent is there:
/// Some(empty) when the file doesn't exist yet. Text that isn't a JSON object is an error.
async fn read_object(host: &Host, file: &str, present: &str) -> HubResult<Option<Map<String, Value>>> {
    let r = host.run(&format!("if {present}; then echo @@; cat {file} 2>/dev/null; fi")).await?;
    let Some(rest) = r.stdout.strip_prefix("@@") else { return Ok(None) };
    let text = rest.jtrim();
    if text.is_empty() {
        return Ok(Some(Map::new()));
    }
    match json::parse(text) {
        Some(Value::Object(m)) => Ok(Some(m)),
        _ => Err(HubError::http(409, format!("{} on that machine is not valid JSON; fix it first", file.trim_matches('"')))),
    }
}

/// Writes `v` to `file` readable by you only, with no half-written copy if the connection drops,
/// and a backup of what was there before Inyon first changed it (`backup`: a shared file).
async fn write_object(host: &Host, file: &str, v: &Value, backup: bool) -> HubResult<()> {
    let f = file.trim_matches('"');
    let mut cmd = vec![format!(r#"umask 077; trap 'rm -f "{f}.inyon-tmp"' EXIT HUP INT TERM"#), format!(r#"mkdir -p "$(dirname "{f}")""#)];
    if backup {
        cmd.push(format!(r#"[ -f "{f}" ] && [ ! -f "{f}.inyon-backup" ] && cp "{f}" "{f}.inyon-backup""#));
    }
    cmd.push(format!(r#"cat > "{f}.inyon-tmp" && mv "{f}.inyon-tmp" "{f}""#));
    let r = host.exec(&cmd.join("; "), ExecOptions::input(pretty(v))).await?;
    if r.code != 0 {
        let e = r.stderr.jtrim();
        return Err(HubError::http(500, if e.is_empty() { format!("Could not write {f}") } else { e.to_owned() }));
    }
    Ok(())
}

async fn install_one(host: &Host, agent: AgentKind) -> HubResult<bool> {
    match agent {
        AgentKind::Copilot => {
            let present = format!(r#"command -v copilot >/dev/null 2>&1 || [ -d {COPILOT_HOME} ]"#);
            if host.run(&format!("{present} && echo yes")).await?.stdout.jtrim() != "yes" {
                return Ok(false);
            }
            write_object(host, COPILOT_FILE, &copilot_hooks(), false).await?;
        }
        AgentKind::Gemini => {
            let Some(settings) = read_object(host, GEMINI_FILE, "command -v gemini >/dev/null 2>&1").await? else { return Ok(false) };
            let next = with_gemini_hooks(settings.clone()).map_err(HubError::plain)?;
            if next != settings {
                write_object(host, GEMINI_FILE, &Value::Object(next), true).await?;
            }
        }
        AgentKind::Antigravity => {
            let present = r#"command -v agy >/dev/null 2>&1 || [ -d "$HOME/.gemini/antigravity-cli" ]"#;
            let Some(hooks) = read_object(host, AGY_FILE, present).await? else { return Ok(false) };
            let next = with_agy_hooks(hooks.clone());
            if next != hooks {
                write_object(host, AGY_FILE, &Value::Object(next), true).await?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Adds Inyon's hook to each of those agents on the machine. One whose settings can't be read
/// (not valid JSON) is skipped and logged, so it doesn't stop the others or Claude Code's.
/// Sessions already running pick it up when restarted. Returns the agents it's in now.
pub async fn install_agent_hooks(host: &Host) -> HubResult<Vec<AgentKind>> {
    let mut done = vec![];
    for agent in HOOKED_AGENTS {
        match install_one(host, agent).await {
            Ok(true) => done.push(agent),
            Ok(false) => {}
            Err(e) => console::log(&format!("{} hook on {}: {}", agent.as_str(), host.id(), e.message)),
        }
    }
    Ok(done)
}

/// Takes Inyon's hook back out of each agent's settings.
pub async fn uninstall_agent_hooks(host: &Host) -> HubResult<()> {
    host.run(&format!(r#"grep -qs 'inyon/hook.sh copilot' {COPILOT_FILE} && rm -f {COPILOT_FILE}; true"#)).await?;
    if let Some(settings) = read_object(host, GEMINI_FILE, &format!("[ -f {GEMINI_FILE} ]")).await.unwrap_or(None) {
        if let Some(next) = without_gemini_hooks(settings) {
            write_object(host, GEMINI_FILE, &Value::Object(next), true).await?;
        }
    }
    if let Some(mut hooks) = read_object(host, AGY_FILE, &format!("[ -f {AGY_FILE} ]")).await.unwrap_or(None) {
        if hooks.shift_remove(AGY_NAME).is_some() {
            if hooks.is_empty() {
                host.run(&format!("rm -f {AGY_FILE}")).await?;
            } else {
                write_object(host, AGY_FILE, &Value::Object(hooks), true).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_hooks_tests.rs"]
mod tests;
