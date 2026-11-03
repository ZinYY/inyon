//! The companion agent: one Claude Code or Codex session Inyon starts on the hub's own machine, in
//! a tmux session called "companion", that you talk to from the app or a companion app (Legato's
//! island). It reads and steers your other sessions only through the `inyon` command: `sessions`,
//! `read`, `send` and `answer`. Its folder is ~/.inyon/companion, where Inyon writes the
//! instructions it starts with.

use std::cell::RefCell;

use inyon_protocol::{AgentKind, AssistantBlock, ChatItem, PromptOption, ServerMsg, SessionInfo};
use serde_json::{json, Value};

use crate::claude::actions::{self, LaunchOpts};
use crate::claude::last_reply::{plain_text, recent_items, reply_of};
use crate::clock::now_ms;
use crate::config::{load_settings, save_settings};
use crate::hosts::host::ExecOptions;
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;
use crate::js::JsStr;

/// The machine it runs on: the hub's own, where the `inyon` command can use the local token.
pub const HOST: &str = "local";
pub const SESSION: &str = "companion";
pub const FOLDER: &str = "~/.inyon/companion";
/// the most of a reply a companion.reply frame carries
pub const REPLY_MAX: usize = 2000;
/// The first line of the instruction files. While it's there Inyon rewrites them on every start;
/// delete it and your edits stay.
pub const MARK: &str = "<!-- Inyon rewrites this file when the companion starts. Delete this line to keep your own changes. -->";

thread_local! {
    /// the agent from Settings, once read
    static AGENT: RefCell<Option<Option<AgentKind>>> = const { RefCell::new(None) };
}

fn kind_of(name: &str) -> Option<AgentKind> {
    match name {
        "claude" => Some(AgentKind::Claude),
        "codex" => Some(AgentKind::Codex),
        _ => None,
    }
}

fn name_of(kind: AgentKind) -> &'static str {
    if kind == AgentKind::Codex {
        "codex"
    } else {
        "claude"
    }
}

/// The agent the companion runs, while it's turned on.
pub fn agent() -> Option<AgentKind> {
    if let Some(a) = AGENT.with(|a| *a.borrow()) {
        return a;
    }
    let a = load_settings().0.get("companion").and_then(|c| c.get("agent")).and_then(Value::as_str).and_then(kind_of);
    AGENT.with(|c| *c.borrow_mut() = Some(a));
    a
}

fn set_agent(kind: Option<AgentKind>) -> HubResult<()> {
    let mut s = load_settings();
    match kind {
        Some(k) => s.0.insert("companion".into(), json!({ "agent": name_of(k) })),
        None => s.0.remove("companion"),
    };
    save_settings(&s).map_err(|e| http(500, e))?;
    AGENT.with(|c| *c.borrow_mut() = Some(kind));
    Ok(())
}

/// Marks the companion's pane: the first agent pane of the "companion" session on the hub's
/// machine, while the companion is turned on. Every other pane is left unmarked.
pub fn mark(host_id: &str, sessions: &mut [SessionInfo], on: bool) {
    let mut found = false;
    for s in sessions.iter_mut() {
        let ours = on && host_id == HOST && s.name == SESSION;
        for p in s.windows.iter_mut().flat_map(|w| w.panes.iter_mut()) {
            let me = ours && !found && p.agent.is_some() && !p.dead;
            found |= me;
            p.companion = me.then_some(true);
        }
    }
}

/// The companion's pane id, while it runs.
pub fn pane() -> Option<String> {
    let rt = registry().get(HOST)?;
    let r = rt.borrow();
    r.state.sessions.iter().flat_map(|s| s.windows.iter().flat_map(|w| w.panes.iter())).find(|p| p.companion == Some(true)).map(|p| p.id.clone())
}

/// `GET /api/companion`
pub fn status() -> Value {
    json!({ "agent": agent().map(name_of), "host": HOST, "pane": pane() })
}

/// What the companion is told when it starts (AGENTS.md, and CLAUDE.md for Claude Code).
pub fn instructions() -> String {
    format!(
        r#"{MARK}

# Inyon companion

You are the companion agent in Inyon. The person talks to you from the Inyon app, their phone or a
companion app like Legato, often by voice, while their coding agents work in tmux sessions on
their machines. Help them keep track of those agents and steer them.

## How to answer

- Short and plain. Your reply may be read aloud or shown on a small screen: no tables, code blocks
  or headings, and a few sentences unless they ask for more.
- Lead with what needs them: an agent waiting for an answer, or one that stopped on an error.
- Name sessions the way `inyon sessions` does: machine/session.

## What you can do

- `inyon sessions`: every session on every machine, with its agent, state and folder.
- `inyon hub machines`: the machines, and whether each is online.
- `inyon read <machine>/<session>`: the latest messages of a session's conversation (`-n 40` for more).
- `inyon send <machine>/<session> "text"`: a message for that agent. It goes in once the agent is free.
- `inyon answer <machine>/<session> <choice>`: answers the question an agent is asking, by the
  choice's number or its words. `inyon sessions --json` shows the question and its choices.
- `inyon pages --hub`: the pages the agents have published.
- `inyon docs`: how Inyon works, when they ask about it.

Add `--json` to any of them for every detail.

## Rules

- Never send a message or answer a question unless they asked for it in this conversation. Before
  you do, say in one line what you'll send or pick.
- Don't edit files, run builds or start servers in their projects. The agents in those sessions
  do the work; you tell them what to do.
- Reach other sessions only through these commands: no tmux send-keys, no ssh.
- When a command fails, say so in one line with what it said.
"#
    )
}

/// Claude Code's settings for the folder: the commands that only read run without asking. A send
/// or an answer still asks first, so it shows as a question in the app and on the island.
pub fn claude_settings() -> Value {
    json!({
        "permissions": {
            "allow": ["Bash(inyon sessions:*)", "Bash(inyon read:*)", "Bash(inyon hub machines:*)", "Bash(inyon pages:*)", "Bash(inyon docs:*)"]
        }
    })
}

/// Codex's rule for the folder: a send or an answer asks first, as it does for Claude Code. Its
/// sandbox can't hold them back, since the `inyon` command needs the network.
pub const CODEX_RULES: &str = r#"prefix_rule(pattern = ["inyon", ["send", "answer"]], decision = "prompt", justification = "The companion sends to your sessions only once you allow it")
"#;

/// Writes Codex's rule into the folder, and trusts the folder in Codex's config.toml, since Codex
/// reads a folder's rules only once it's trusted. A config.toml that names the folder already is
/// left alone. Prints `trusted` when the folder is.
pub fn codex_setup_script() -> String {
    r#"umask 077; d="$HOME/.inyon/companion"; mkdir -p "$d/.codex/rules"; cat > "$d/.codex/rules/companion.rules"
c="${CODEX_HOME:-$HOME/.codex}/config.toml"
if ! grep -qF "$d" "$c" 2>/dev/null; then mkdir -p "$(dirname "$c")" && printf '\n[projects."%s"]\ntrust_level = "trusted"\n' "$d" >> "$c"; fi
awk -v h="[projects.\"$d\"]" '$0 == h { t = 1; next } /^\[/ { t = 0 } t && /^trust_level *= *"trusted"/ { f = 1 } END { if (f) print "trusted" }' "$c" 2>/dev/null"#
        .into()
}

/// The flags it starts with. Codex's sandbox keeps the network closed, and the `inyon` command
/// needs it to reach the hub on this machine. Without its rule (`trusted` false) Codex asks before
/// every command that isn't known to be safe.
pub fn launch_flags(kind: AgentKind, trusted: bool) -> Vec<String> {
    match kind {
        AgentKind::Codex => {
            let mut f: Vec<String> = vec!["--sandbox workspace-write".into(), "-c sandbox_workspace_write.network_access=true".into()];
            if !trusted {
                f.push("--ask-for-approval untrusted".into());
            }
            f
        }
        _ => vec![],
    }
}

/// `POST /api/companion`: starts the companion with `agent` ("claude" or "codex").
pub async fn start(agent_name: &str) -> HubResult<Value> {
    let kind = kind_of(agent_name).ok_or_else(|| http(400, "The companion runs Claude Code or Codex"))?;
    let host = registry().host(HOST)?;
    let open = host.run(&format!("tmux has-session -t '={SESSION}' 2>/dev/null")).await?.code == 0;
    if open {
        match (agent(), pane()) {
            (Some(_), Some(_)) => return Err(http(409, "The companion is already running")),
            // ours, with its agent gone (it quit, or crashed): start over
            (Some(_), None) => {
                host.run(&format!("tmux kill-session -t '={SESSION}'")).await?;
            }
            (None, _) => return Err(http(409, "A tmux session called companion is already open on this machine. Rename or close it first.")),
        }
    }
    let dir = r#""$HOME/.inyon/companion""#;
    host.run(&format!("umask 077; mkdir -p {dir}/.claude")).await?;
    // instructions you've made your own (their first line deleted) stay as they are
    for f in ["AGENTS.md", "CLAUDE.md"] {
        let path = format!("{dir}/{f}");
        let cmd = format!(r#"if [ ! -f {path} ] || head -n 1 {path} | grep -q 'Inyon rewrites this file'; then cat > {path}; else cat > /dev/null; fi"#);
        host.exec(&cmd, ExecOptions::input(instructions())).await?;
    }
    let settings = format!("{dir}/.claude/settings.json");
    host.exec(&format!("[ -f {settings} ] || cat > {settings}"), ExecOptions::input(format!("{}\n", crate::js::json::stringify_pretty(&claude_settings())))).await?;
    let trusted = kind == AgentKind::Codex && host.exec(&codex_setup_script(), ExecOptions::input(CODEX_RULES)).await?.stdout.trim() == "trusted";
    set_agent(Some(kind))?;
    let opts = LaunchOpts { name: Some(SESSION.into()), cwd: Some(FOLDER.into()), agent: Some(name_of(kind).into()), extra: launch_flags(kind, trusted), ..Default::default() };
    let pane = match actions::launch(HOST, &opts).await {
        Ok(p) => p,
        Err(e) => {
            set_agent(None)?;
            return Err(e);
        }
    };
    Ok(json!({ "agent": name_of(kind), "host": HOST, "pane": pane }))
}

/// `DELETE /api/companion`: ends its session and turns it off.
pub async fn stop() -> HubResult<()> {
    if agent().is_none() {
        return Ok(());
    }
    let host = registry().host(HOST)?;
    host.run(&format!("tmux kill-session -t '={SESSION}' 2>/dev/null")).await?;
    set_agent(None)?;
    registry().refresh(HOST, 100);
    Ok(())
}

/// One tool call, as a line: its name and what it was given.
fn tool_line(name: &str, input: &Value) -> String {
    let what = ["command", "description", "file_path", "path", "pattern", "url", "query", "prompt"].iter().find_map(|k| input.get(*k).and_then(Value::as_str));
    match what {
        Some(w) => format!("{name}: {}", plain_text(w, 160).replace('\n', " ")),
        None => name.to_owned(),
    }
}

/// A conversation as `inyon read` shows it: who said what, tool calls as one line each, the last
/// `limit` entries. Thinking, tool output and bookkeeping are left out.
pub fn condense(items: &[ChatItem], limit: usize) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    for it in items {
        let (role, text) = match it {
            ChatItem::User { text, shell, .. } if !text.jtrim().is_empty() => (if *shell == Some(true) { "shell" } else { "you" }, plain_text(text, 1200)),
            ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } if !text.jtrim().is_empty() => ("agent", plain_text(text, 2000)),
            ChatItem::Assistant { block: AssistantBlock::ToolUse { name, input, .. }, .. } => ("tool", tool_line(name, input)),
            ChatItem::ToolResult { is_error: true, text, .. } => ("error", plain_text(text, 300)),
            ChatItem::Notice { text, .. } if !text.jtrim().is_empty() => ("notice", plain_text(text, 300)),
            _ => continue,
        };
        out.push(json!({ "role": role, "text": text }));
    }
    let skip = out.len().saturating_sub(limit);
    out.split_off(skip)
}

/// `GET /api/hosts/:id/panes/:pane/history`
pub async fn history(host_id: &str, pane_id: &str, limit: usize) -> HubResult<Value> {
    let found = registry().find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    if found.pane.agent.is_none() {
        return Err(http(409, "No agent is running in this pane"));
    }
    let items = recent_items(host_id, pane_id).await?;
    Ok(json!({ "items": condense(&items, limit.clamp(1, 200)) }))
}

/// The choice `inyon answer` names, by its number or its words: the whole label first (any case),
/// then the one label that starts with them, then the one that contains them, then the one that
/// has each of the words somewhere ("yes trust" for "Yes, I trust this folder"). A word is what
/// spaces separate, less the punctuation of a sentence around it, so an address, a path or a flag
/// is one word and only matches whole: `https://com.example` never picks `https://example.com`.
pub fn pick_choice(options: &[PromptOption], choice: &str) -> Result<(u64, String), String> {
    let c = choice.jtrim().to_lowercase();
    let list = || options.iter().map(|o| format!("{}. {}", o.n, o.label)).collect::<Vec<_>>().join(", ");
    if c.is_empty() {
        return Err(format!("Name a choice: {}", list()));
    }
    if let Ok(n) = c.parse::<u64>() {
        return options.iter().find(|o| o.n == n).map(|o| (o.n, o.label.clone())).ok_or_else(|| format!("That question has no choice {n}. Its choices: {}", list()));
    }
    let words = |t: &str| t.split_whitespace().map(|w| w.trim_matches(|ch: char| ",.;:!?()[]\"`".contains(ch))).filter(|w| !w.is_empty()).map(str::to_owned).collect::<Vec<_>>();
    let asked = words(&c);
    let tests: [&dyn Fn(&str) -> bool; 4] = [&|l| l == c, &|l| l.starts_with(&c), &|l| l.contains(&c), &|l| {
        let have = words(l);
        !asked.is_empty() && asked.iter().all(|w| have.contains(w))
    }];
    for test in tests {
        let found: Vec<&PromptOption> = options.iter().filter(|o| test(&o.label.to_lowercase())).collect();
        match found.as_slice() {
            [o] => return Ok((o.n, o.label.clone())),
            [] => continue,
            _ => return Err(format!("More than one choice matches \"{}\". Name it by number: {}", choice.jtrim(), list())),
        }
    }
    Err(format!("That question has no choice like \"{}\". Its choices: {}", choice.jtrim(), list()))
}

/// `POST /api/hosts/:id/panes/:pane/answer` from the inyon command on the hub's machine: picks the
/// choice named on the question that's on screen now.
pub async fn answer_by_choice(host_id: &str, pane_id: &str, choice: &str) -> HubResult<Value> {
    let screen = actions::current_screen(host_id, pane_id).await?;
    let Some(p) = screen.prompt.filter(|p| !p.options.is_empty()) else {
        return Err(http(409, "No question is waiting in that session"));
    };
    let (n, label) = pick_choice(&p.options, choice).map_err(|e| http(400, e))?;
    let key = inyon_protocol::prompt_key(&p.body, &p.options);
    actions::answer(host_id, pane_id, n as u32, None, Some(&key)).await?;
    Ok(json!({ "ok": true, "option": n, "label": label }))
}

thread_local! {
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The frame for a turn the companion finished: its reply as plain text, or why it stopped.
pub async fn reply(host_id: &str, pane_id: &str, failure: Option<String>) -> ServerMsg {
    let n = NEXT.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let at = now_ms();
    let id = format!("{at:x}-{n}");
    let (text, error) = match failure {
        Some(f) => (f, Some(true)),
        None => match recent_items(host_id, pane_id).await {
            Ok(items) => match reply_of(&items) {
                Some(r) => (plain_text(&r, REPLY_MAX), None),
                None => ("Done.".to_owned(), None),
            },
            Err(e) => (format!("Couldn't read the reply: {}", e.message), Some(true)),
        },
    };
    ServerMsg::CompanionReply { id, host: host_id.to_owned(), pane: pane_id.to_owned(), text, error, at }
}

#[cfg(test)]
#[path = "companion_tests.rs"]
mod tests;
