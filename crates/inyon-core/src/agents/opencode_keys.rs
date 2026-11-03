//! Port of agents/opencode-keys.ts: the keys Inyon presses in OpenCode's terminal, as OpenCode
//! 1.19 binds them unless the config says otherwise (`keybinds` in opencode.json, user or
//! project), read from the machine and kept for a minute.

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

use std::cell::RefCell;
use std::collections::HashMap;

use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::js::json;
use crate::platform::rt::{spawn_task, Deferred};
use crate::re;

/// The default bindings, in OpenCode's order.
pub const DEFAULT_KEYS: &[(&str, &str)] = &[
    ("leader", "ctrl+x"),
    ("agent_cycle", "tab"),
    ("agent_cycle_reverse", "shift+tab"),
    ("variant_cycle", "ctrl+t"),
    ("model_list", "<leader>m"),
    ("session_interrupt", "escape"),
];

pub const DEFAULT_LEADER: &str = "ctrl+x";

fn default_key(name: &str) -> &'static str {
    DEFAULT_KEYS.iter().find(|(k, _)| *k == name).map_or("", |(_, v)| v)
}

/// The keys to press for each action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenCodeKeys {
    pub agent_cycle: Vec<String>,
    pub agent_cycle_reverse: Vec<String>,
    pub variant_cycle: Vec<String>,
    pub model_list: Vec<String>,
    pub session_interrupt: Vec<String>,
}

fn named(key: &str) -> Option<&'static str> {
    Some(match key {
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "return" | "enter" => "Enter",
        "space" => "Space",
        "backspace" => "BSpace",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        _ => return None,
    })
}

/// How many `<leader>` prefixes one binding may stack. The TS resolves each with a recursive call,
/// and Bun runs out of stack somewhere between 12,000 and 15,000 of them; the RangeError ends in
/// `opencodeKeys`'s catch, which falls back to the default keys. Past this many, so does the Rust.
pub const MAX_LEADERS: usize = 12_000;

/// A binding stacking more than [`MAX_LEADERS`] `<leader>` prefixes (the TS throws on it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooDeep;

/// One binding as OpenCode writes it ("ctrl+t", "shift+tab", `<leader>m`, "f2"; alternatives
/// separated by commas) as the keys `tmux send-keys` takes, or None when none of its alternatives
/// can be sent that way (super, shift with most keys), or when it stacks so many `<leader>`
/// prefixes that the TS would throw.
pub fn tmux_keys(binding: &str, leader: Option<&str>) -> Option<Vec<String>> {
    binding_keys(binding, leader.unwrap_or(DEFAULT_LEADER), 0).ok().flatten()
}

/// `tmuxKeys`, without its recursion: a run of `<leader>` prefixes is counted, and the leader's
/// keys repeated that many times before the key itself. `depth` is how deep the TS would be
/// already (the leader's own binding is resolved under the bindings that use it).
fn binding_keys(binding: &str, leader: &str, depth: usize) -> Result<Option<Vec<String>>, TooDeep> {
    for alt in binding.split(',').map(|s| crate::js::text::lower(crate::js::JsStr::jtrim(s))).filter(|s| !s.is_empty()) {
        if alt == "none" {
            continue;
        }
        let mut leaders = 0;
        let mut rest = alt.as_str();
        while let Some(r) = rest.strip_prefix("<leader>") {
            leaders += 1;
            if depth + leaders > MAX_LEADERS {
                return Err(TooDeep);
            }
            // each level splits, trims and lower-cases what's left, as the recursive call would
            rest = crate::js::JsStr::jtrim(r);
        }
        if leaders > 0 {
            let first = binding_keys(leader, DEFAULT_LEADER, depth + leaders)?;
            let key = if rest.is_empty() || rest == "none" { None } else { plain_key(rest) };
            if let (Some(first), Some(key)) = (first, key) {
                let mut out = Vec::with_capacity(first.len() * leaders + key.len());
                for _ in 0..leaders {
                    out.extend(first.iter().cloned());
                }
                out.extend(key);
                return Ok(Some(out));
            }
            continue;
        }
        if let Some(keys) = plain_key(&alt) {
            return Ok(Some(keys));
        }
    }
    Ok(None)
}

/// One alternative without a `<leader>` ("ctrl+t", "shift+tab", "f2").
fn plain_key(alt: &str) -> Option<Vec<String>> {
    let mut parts: Vec<&str> = alt.split('+').collect();
    let key = parts.pop().unwrap_or("");
    let has = |m: &str| parts.contains(&m);
    if has("super") || has("cmd") || has("meta") {
        return None;
    }
    let mut name = if let Some(n) = named(key) {
        n.to_owned()
    } else if re!(r"^f([1-9]|1[0-2])$").is_match(key) {
        key.to_uppercase()
    } else if re!(r"^[a-z0-9]$").is_match(key) {
        key.to_owned()
    } else {
        return None;
    };
    if has("shift") {
        if name == "Tab" {
            name = "BTab".into();
        } else if re!(r"^[a-z]$").is_match(&name) {
            name = name.to_uppercase();
        } else {
            return None;
        }
    }
    let ctrl = has("ctrl") || has("control");
    let alt_ = has("alt") || has("option");
    if (ctrl || alt_) && !re!(r"^[a-zA-Z0-9]$").is_match(&name) {
        return None;
    }
    Some(vec![format!("{}{}{name}", if ctrl { "C-" } else { "" }, if alt_ { "M-" } else { "" })])
}

/// `keybinds` from the files OpenCode reads (user, then the project's), later ones winning.
pub fn merge_keybinds(files: &[&str]) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for text in files {
        let Some(Value::Object(config)) = parse_jsonc(text) else { continue };
        let Some(Value::Object(binds)) = config.get("keybinds") else { continue };
        for (k, v) in binds {
            if let Value::String(v) = v {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// JSON with comments and trailing commas, as opencode.jsonc allows; None when it doesn't parse.
pub fn parse_jsonc(text: &str) -> Option<Value> {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    // where in `out` the last comma outside a string is, while only whitespace has followed it
    let mut comma: Option<usize> = None;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            out.extend(&c[i..(j + 1).min(c.len())]);
            comma = None;
            i = j + 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            let end = (i + 2..c.len().saturating_sub(1)).find(|&k| c[k] == '*' && c[k + 1] == '/');
            i = end.map_or(c.len(), |e| e + 2);
        } else {
            // a trailing comma goes; one inside a string stays
            if let (Some(at), '}' | ']') = (comma, ch) {
                out.remove(at);
            }
            if ch == ',' {
                comma = Some(out.len());
            } else if !matches!(ch, ' ' | '\t' | '\n' | '\r') {
                comma = None;
            }
            out.push(ch);
            i += 1;
        }
    }
    json::parse(&out)
}

/// The keys to press for each action, from the bindings (defaults filled in).
///
/// A binding stacking too many `<leader>` prefixes makes `keysFrom` throw in the TS, and
/// `opencodeKeys` then uses the defaults for every key; so does this.
pub fn keys_from(binds: &IndexMap<String, String>) -> OpenCodeKeys {
    let leader = binds.get("leader").map_or(DEFAULT_LEADER, String::as_str);
    let keys = |name: &str| -> Result<Vec<String>, TooDeep> {
        let set = binding_keys(binds.get(name).map_or(default_key(name), String::as_str), leader, 0)?;
        Ok(set.or_else(|| tmux_keys(default_key(name), None)).unwrap_or_default())
    };
    let read = || -> Result<OpenCodeKeys, TooDeep> {
        Ok(OpenCodeKeys {
            agent_cycle: keys("agent_cycle")?,
            agent_cycle_reverse: keys("agent_cycle_reverse")?,
            variant_cycle: keys("variant_cycle")?,
            model_list: keys("model_list")?,
            session_interrupt: keys("session_interrupt")?,
        })
    };
    read().unwrap_or_else(|TooDeep| keys_from(&IndexMap::new()))
}

/// The command printing the user's and the project's config files, each after "@@FILE".
pub fn opencode_keys_command(cwd: Option<&str>) -> String {
    let mut files = vec![r#""${XDG_CONFIG_HOME:-$HOME/.config}/opencode/opencode.json""#.to_owned(), r#""${XDG_CONFIG_HOME:-$HOME/.config}/opencode/opencode.jsonc""#.to_owned()];
    if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
        let q = shq(cwd);
        files.extend([format!("{q}/opencode.json"), format!("{q}/opencode.jsonc"), format!("{q}/.opencode/opencode.json"), format!("{q}/.opencode/opencode.jsonc")]);
    }
    format!(r#"for f in {}; do [ -f "$f" ] && {{ echo "@@FILE"; cat "$f"; echo; }}; done; true"#, files.join(" "))
}

/// The keys from what that command printed.
pub fn opencode_keys(stdout: &str) -> OpenCodeKeys {
    let files: Vec<&str> = stdout.split("@@FILE\n").skip(1).collect();
    keys_from(&merge_keybinds(&files))
}

thread_local! {
    static KEYS: RefCell<HashMap<String, (u64, Deferred<OpenCodeKeys>)>> = RefCell::new(HashMap::new());
}

/// `opencodeKeys(host, cwd)`: OpenCode's key bindings on a machine, for a session in `cwd`; read
/// again after a minute. A machine that doesn't answer gets the defaults.
pub fn opencode_keys_of(host: &Host, cwd: Option<&str>) -> Deferred<OpenCodeKeys> {
    let key = format!("{}\0{}", host.id(), cwd.unwrap_or(""));
    if let Some((_, k)) = KEYS.with(|c| c.borrow().get(&key).filter(|(at, _)| now_ms().saturating_sub(*at) < 60_000).cloned()) {
        return k;
    }
    let (h, cmd) = (host.clone(), opencode_keys_command(cwd));
    let keys = spawn_task(async move {
        match h.exec(&cmd, ExecOptions::timeout(10_000)).await {
            Ok(r) => opencode_keys(&r.stdout),
            Err(_) => keys_from(&IndexMap::new()),
        }
    });
    KEYS.with(|c| {
        let mut c = c.borrow_mut();
        c.insert(key, (now_ms(), keys.clone()));
        if c.len() > 200 {
            c.retain(|_, (at, _)| now_ms().saturating_sub(*at) <= 60_000);
        }
    });
    keys
}

#[cfg(test)]
#[path = "opencode_keys_tests.rs"]
mod tests;
