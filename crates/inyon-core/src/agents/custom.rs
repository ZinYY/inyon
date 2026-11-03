// Custom agents (Settings › Agents): any command, with the built-in agent it works like. A pane
// running one is that agent to the rest of the hub (chat, hooks, prompts, models), and carries the
// custom name so the app can show it.

use std::cell::RefCell;

use inyon_protocol::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude::actions::agent_kind;
use crate::config::load_settings;

pub const MAX_AGENTS: usize = 24;
const MAX_NAME: usize = 40;
const MAX_COMMAND: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomAgent {
    pub id: String,
    pub name: String,
    pub command: String,
    pub like: AgentKind,
}

// interpreters that run an agent's script: `node ~/.local/bin/openclaude` is openclaude. Shells
// aren't here, because a wrapper script that doesn't exec has the real agent as its child.
const INTERPRETERS: [&str; 6] = ["node", "bun", "deno", "python", "python3", "uv"];

fn base(word: &str) -> &str {
    let w = word.rsplit(['/', '\\']).next().unwrap_or(word);
    w.strip_suffix(".exe").unwrap_or(w)
}

impl CustomAgent {
    /// The command's program, as `ps` names it.
    pub fn program(&self) -> &str {
        base(self.command.split_whitespace().next().unwrap_or(""))
    }

    /// Whether a process is this agent: the program matches (its name, its argv[0], or the script
    /// an interpreter runs) and every further word of the command is on its command line.
    pub fn matches(&self, comm: &str, args: &str) -> bool {
        let prog = self.program();
        if prog.is_empty() {
            return false;
        }
        let argv: Vec<&str> = args.split_whitespace().collect();
        let first = argv.first().map(|w| base(w)).unwrap_or("");
        let named = base(comm) == prog || first == prog || (INTERPRETERS.contains(&first) && argv.get(1).map(|w| base(w)) == Some(prog));
        named && self.command.split_whitespace().skip(1).all(|w| argv.contains(&w))
    }
}

/// A name as an id: lowercase letters, digits and dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// The list as saved: `value` checked entry by entry. A bad entry is an error, so a typo in the
/// app shows instead of an agent quietly disappearing.
pub fn parse_list(value: &Value) -> Result<Vec<CustomAgent>, String> {
    let Value::Array(items) = value else { return Err("Custom agents must be a list".into()) };
    if items.len() > MAX_AGENTS {
        return Err(format!("At most {MAX_AGENTS} custom agents"));
    }
    let mut out: Vec<CustomAgent> = vec![];
    for item in items {
        let text = |k: &str| item.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("").to_owned();
        let (name, command, like) = (text("name"), text("command"), text("like"));
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!("A custom agent needs a name of 1 to {MAX_NAME} characters"));
        }
        if command.is_empty() || command.chars().count() > MAX_COMMAND || command.chars().any(char::is_control) {
            return Err(format!("{name}: the command must be one line of 1 to {MAX_COMMAND} characters"));
        }
        let Some(like) = agent_kind(&like).filter(|k| k.chat()) else { return Err(format!("{name}: pick the agent it works like")) };
        let id = slug(&name);
        if id.is_empty() {
            return Err(format!("{name}: use at least one letter or digit in the name"));
        }
        if out.iter().any(|a| a.id == id) {
            return Err(format!("Two custom agents are called {name}"));
        }
        out.push(CustomAgent { id, name, command, like });
    }
    Ok(out)
}

thread_local! {
    static CACHE: RefCell<Option<Vec<CustomAgent>>> = const { RefCell::new(None) };
}

/// The saved custom agents, read once and kept until `set` replaces them.
pub fn list() -> Vec<CustomAgent> {
    CACHE.with(|c| c.borrow_mut().get_or_insert_with(|| load_settings().0.get("customAgents").and_then(|v| parse_list(v).ok()).unwrap_or_default()).clone())
}

/// After the settings were saved.
pub fn set(agents: Vec<CustomAgent>) {
    CACHE.with(|c| *c.borrow_mut() = Some(agents));
}

pub fn by_id(id: &str) -> Option<CustomAgent> {
    list().into_iter().find(|a| a.id == id)
}

/// The custom agent a process is, if any (the first that matches, in the saved order). Reads only
/// what `list` already loaded, so parsing a probe never touches the store.
pub fn detect(comm: &str, args: &str) -> Option<CustomAgent> {
    CACHE.with(|c| c.borrow().as_ref().and_then(|l| l.iter().find(|a| a.matches(comm, args)).cloned()))
}

#[cfg(test)]
#[path = "custom_tests.rs"]
mod tests;
