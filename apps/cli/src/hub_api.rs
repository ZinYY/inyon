//! `inyon hub machines`, `inyon sessions`, `inyon pages --hub` and `inyon send`: the hub's machines,
//! sessions and pages from the command line, and a message an agent gets once it's free. On the
//! hub's machine they use the local token the hub wrote as it started; on a joined machine, the token
//! that machine keeps for its link to the hub (hub 0.22 and on take either). `inyon read`,
//! `inyon answer` and `inyon companion` work on the hub's machine only (hub 0.22 and on); the
//! companion agent runs them.

use serde_json::{json, Value};

use crate::commands::Outcome;
use crate::paths;

/// The hub to ask and the token to show it.
pub struct Access {
    pub url: String,
    token: String,
    local: bool,
}

pub const NO_HUB: &str = "This machine doesn't run a hub and isn't joined to one. Join a hub first (`inyon docs add machines` says how).";

/// The hub on this machine, else the hub this machine joined.
pub fn access() -> Result<Access, String> {
    if crate::engine::hub_here() {
        if let Ok(t) = std::fs::read_to_string(paths::data_dir().join("local-token")) {
            let env = paths::read_env_file(&paths::data_dir().join("hub.env"));
            return Ok(Access { url: crate::commands::hub_local_url(&env), token: t.trim().to_string(), local: true });
        }
        return Err("The hub on this machine isn't running. Start it with: inyon hub start".into());
    }
    let url = paths::saved_hub().ok_or(NO_HUB)?;
    let token = paths::env_var("INYON_TOKEN").or_else(|| paths::saved_env().get("INYON_TOKEN").cloned()).filter(|t| !t.is_empty()).ok_or(NO_HUB)?;
    Ok(Access { url, token, local: false })
}

/// Why the hub said no, in words that say what to do.
pub fn refusal(status: u16, body: &Value, local: bool) -> String {
    let error = body.get("error").and_then(Value::as_str).unwrap_or("");
    match (status, body.get("reason").and_then(Value::as_str)) {
        (401, _) if local => "The hub on this machine didn't take its own token. Restart it (inyon hub restart) and try again.".into(),
        (401, _) => "The hub didn't take this machine's token. It needs hub 0.22 or later; if it has that, join this machine to it again.".into(),
        (403, Some("send-off")) => error.to_string(),
        (403, Some("scope")) => "The hub is older than this command expects. Update it to 0.22 or later.".into(),
        _ if !error.is_empty() => error.to_string(),
        _ => format!("The hub answered {status}."),
    }
}

impl Access {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let url = format!("{}{path}", self.url);
        let res = crate::http::hub_call(method, &url, &self.token, body, self.local).map_err(|e| format!("Couldn't reach the hub at {}: {e}", crate::http::host_of(&self.url)))?;
        let v: Value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
        if !res.ok() {
            return Err(refusal(res.status, &v, self.local));
        }
        Ok(v)
    }

    pub fn hosts(&self) -> Result<Vec<Value>, String> {
        Ok(self.call("GET", "/api/overview", None)?.get("hosts").and_then(Value::as_array).cloned().unwrap_or_default())
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn name_of(h: &Value) -> &str {
    let c = &h["config"];
    Some(s(c, "name")).filter(|n| !n.is_empty()).unwrap_or(s(c, "id"))
}

/// Every pane of every session on a machine, with the session it's in.
fn panes(h: &Value) -> impl Iterator<Item = (&Value, &Value, &Value)> {
    h["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|sess| sess["windows"].as_array().into_iter().flatten().flat_map(move |w| w["panes"].as_array().into_iter().flatten().map(move |p| (sess, w, p))))
}

fn agent_panes(h: &Value) -> impl Iterator<Item = (&Value, &Value, &Value)> {
    panes(h).filter(|(_, _, p)| p["agent"].is_object() && p["dead"] != json!(true))
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn columns(rows: &[Vec<String>]) -> Vec<String> {
    let n = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..n).map(|i| rows.iter().filter_map(|r| r.get(i)).map(|c| c.chars().count()).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = r.iter().enumerate().map(|(i, c)| if i + 1 == r.len() { c.clone() } else { format!("{c:<w$}", w = widths[i]) }).collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect()
}

/// `inyon hub machines`: one line per machine.
pub fn machine_lines(hosts: &[Value]) -> Vec<String> {
    if hosts.is_empty() {
        return vec!["The hub has no machines yet.".into()];
    }
    let rows: Vec<Vec<String>> = hosts
        .iter()
        .map(|h| {
            let sessions = h["sessions"].as_array().map_or(0, Vec::len);
            let agents: Vec<&Value> = agent_panes(h).map(|(_, _, p)| p).collect();
            let working = agents.iter().filter(|p| p["agent"]["state"] == "working").count();
            let waiting = agents.iter().filter(|p| p["agent"]["state"] == "waiting").count();
            let mut what = vec![plural(sessions, "session"), plural(agents.len(), "agent")];
            if working > 0 {
                what.push(format!("{working} working"));
            }
            if waiting > 0 {
                what.push(format!("{waiting} waiting for you"));
            }
            vec![name_of(h).to_string(), s(h, "status").to_string(), what.join(", ")]
        })
        .collect();
    columns(&rows)
}

/// `inyon sessions`: one line per agent, and one per session with none.
pub fn session_lines(hosts: &[Value]) -> Vec<String> {
    let mut rows = Vec::new();
    for h in hosts {
        for sess in h["sessions"].as_array().into_iter().flatten() {
            let agents: Vec<(&Value, &Value)> = sess["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|w| w["panes"].as_array().into_iter().flatten().map(move |p| (w, p)))
                .filter(|(_, p)| p["agent"].is_object() && p["dead"] != json!(true))
                .collect();
            let target = format!("{}/{}", name_of(h), s(sess, "name"));
            if agents.is_empty() {
                rows.push(vec![target.clone(), "none".into()]);
            }
            for (w, p) in &agents {
                let t = if agents.len() > 1 { format!("{target}:{}.{}", w["index"], p["index"]) } else { target.clone() };
                rows.push(vec![t, s(&p["agent"], "kind").into(), s(&p["agent"], "state").into(), s(p, "cwd").into()]);
            }
        }
    }
    if rows.is_empty() {
        return vec!["No tmux sessions on the hub's machines.".into()];
    }
    columns(&rows)
}

/// A pane to send to, from `machine/session` or `machine/session:window.pane`.
#[derive(Debug, PartialEq)]
pub struct Target {
    pub host: String,
    pub pane: String,
    pub label: String,
    pub agent: String,
}

pub fn resolve(hosts: &[Value], target: &str) -> Result<Target, String> {
    let usage = "Name the session as machine/session, or machine/session:window.pane when it runs more than one agent (`inyon sessions` lists them).";
    let (machine, rest) = target.split_once('/').ok_or(usage)?;
    let (session, at) = match rest.rsplit_once(':') {
        Some((s, at)) if at.split_once('.').is_some_and(|(w, p)| w.parse::<u32>().is_ok() && p.parse::<u32>().is_ok()) => (s, Some(at)),
        _ => (rest, None),
    };
    let h = hosts
        .iter()
        .find(|h| name_of(h) == machine || s(&h["config"], "id") == machine)
        .or_else(|| {
            let lower: Vec<&Value> = hosts.iter().filter(|h| name_of(h).eq_ignore_ascii_case(machine)).collect();
            (lower.len() == 1).then(|| lower[0])
        })
        .ok_or_else(|| format!("The hub has no machine called {machine}. `inyon hub machines` lists them."))?;
    let label = format!("{}/{session}", name_of(h));
    let found: Vec<(&Value, &Value, &Value)> = agent_panes(h).filter(|(sess, _, _)| s(sess, "name") == session).collect();
    if found.is_empty() {
        return Err(if panes(h).any(|(sess, _, _)| s(sess, "name") == session) {
            format!("No agent is running in {label}.")
        } else {
            format!("{} has no session called {session}. `inyon sessions` lists them.", name_of(h))
        });
    }
    let pick = match at {
        Some(at) => found.iter().find(|(_, w, p)| format!("{}.{}", w["index"], p["index"]) == at).ok_or_else(|| format!("No agent is running in {label}:{at}."))?,
        None if found.len() == 1 => &found[0],
        None => {
            let list: Vec<String> = found.iter().map(|(_, w, p)| format!("{label}:{}.{} ({})", w["index"], p["index"], s(&p["agent"], "kind"))).collect();
            return Err(format!("{label} runs {} agents. Name one: {}", found.len(), list.join(", ")));
        }
    };
    let (_, w, p) = pick;
    let label = if at.is_some() || found.len() > 1 { format!("{label}:{}.{}", w["index"], p["index"]) } else { label };
    Ok(Target { host: s(&h["config"], "id").into(), pane: s(p, "id").into(), label, agent: s(&p["agent"], "kind").into() })
}

/// The pane's part of an API path: its id without the `%`, as the app sends it.
fn pane_path(host: &str, pane: &str) -> String {
    let enc = |x: &str| url::form_urlencoded::byte_serialize(x.as_bytes()).collect::<String>().replace('+', "%20");
    format!("/api/hosts/{}/panes/{}", enc(host), enc(pane.trim_start_matches('%')))
}

// ---------- the commands ----------

pub fn machines_command() -> Outcome {
    let a = access()?;
    let hosts = a.hosts()?;
    Ok(json!({ "hub": a.url, "hosts": hosts, "message": machine_lines(&hosts).join("\n") }))
}

pub fn sessions_command() -> Outcome {
    let a = access()?;
    let hosts = a.hosts()?;
    Ok(json!({ "hub": a.url, "hosts": hosts, "message": session_lines(&hosts).join("\n") }))
}

pub fn pages_command() -> Outcome {
    let a = access()?;
    let hosts = a.hosts()?;
    let pages = a.call("GET", "/api/pages", None)?.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let machine = |id: &str| {
        if id == "local" {
            "the hub".to_string()
        } else {
            hosts.iter().find(|h| s(&h["config"], "id") == id).map(|h| name_of(h).to_string()).unwrap_or_else(|| id.to_string())
        }
    };
    let lines = if pages.is_empty() {
        vec!["No pages on the hub's machines yet.".to_string()]
    } else {
        let rows: Vec<Vec<String>> = pages
            .iter()
            .map(|p| {
                vec![
                    s(p, "name").into(),
                    machine(s(p, "host")),
                    paths::basename(s(p, "project").trim_end_matches(['/', '\\'])).into(),
                    format!("/pages/{}/{}", s(p, "host"), s(p, "slug")),
                ]
            })
            .collect();
        columns(&rows)
    };
    Ok(json!({ "hub": a.url, "pages": pages, "message": lines.join("\n") }))
}

pub fn send_command(target: &str, text: &str) -> Outcome {
    if text.trim().is_empty() {
        return Err("Say what to send: inyon send <machine>/<session> \"text\" (or - to read it from stdin).".into());
    }
    let a = access()?;
    let t = resolve(&a.hosts()?, target)?;
    let held = a.call("POST", &format!("{}/held", pane_path(&t.host, &t.pane)), Some(&json!({ "text": text })))?;
    Ok(
        json!({ "target": t.label, "host": t.host, "pane": t.pane, "agent": t.agent, "held": held, "message": format!("Held for {} ({}). It goes in once the agent is free, and shows in the app until then.", t.label, t.agent) }),
    )
}

/// `inyon read`'s lines: who said what, one entry after another.
pub fn read_lines(items: &[Value]) -> Vec<String> {
    if items.is_empty() {
        return vec!["Nothing said in that session yet.".into()];
    }
    items.iter().map(|i| format!("{}: {}", s(i, "role"), s(i, "text"))).collect()
}

pub fn read_command(target: &str, lines: usize) -> Outcome {
    let a = access()?;
    let t = resolve(&a.hosts()?, target)?;
    let v = a.call("GET", &format!("{}/history?limit={}", pane_path(&t.host, &t.pane), lines.clamp(1, 200)), None)?;
    let items = v.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(json!({ "target": t.label, "agent": t.agent, "items": items, "message": read_lines(&items).join("\n") }))
}

pub fn answer_command(target: &str, choice: &str) -> Outcome {
    if choice.trim().is_empty() {
        return Err("Name the choice: inyon answer <machine>/<session> <number or words>.".into());
    }
    let a = access()?;
    let t = resolve(&a.hosts()?, target)?;
    let v = a.call("POST", &format!("{}/answer", pane_path(&t.host, &t.pane)), Some(&json!({ "choice": choice })))?;
    let label = s(&v, "label").to_string();
    Ok(json!({ "target": t.label, "option": v.get("option"), "label": label, "message": format!("Answered {}: {label}", t.label) }))
}

/// What `inyon companion` says about it.
pub fn companion_line(v: &Value) -> String {
    let agent = match s(v, "agent") {
        "codex" => "Codex",
        "claude" => "Claude Code",
        _ => return "The companion is off. Start it with: inyon companion start (or --agent codex)".into(),
    };
    if v["pane"].is_string() {
        format!("The companion runs {agent} in the tmux session companion on the hub's machine.")
    } else {
        format!("The companion is on ({agent}), but its agent isn't running. Start it again with: inyon companion start --agent {}", s(v, "agent"))
    }
}

pub fn companion_command() -> Outcome {
    let a = access()?;
    let v = a.call("GET", "/api/companion", None)?;
    Ok(json!({ "agent": v.get("agent"), "pane": v.get("pane"), "message": companion_line(&v) }))
}

pub fn companion_start(agent: &str) -> Outcome {
    let a = access()?;
    let v = a.call("POST", "/api/companion", Some(&json!({ "agent": agent })))?;
    Ok(json!({ "agent": v.get("agent"), "pane": v.get("pane"), "message": companion_line(&v) }))
}

pub fn companion_stop() -> Outcome {
    let a = access()?;
    a.call("DELETE", "/api/companion", None)?;
    Ok(json!({ "agent": null, "message": "The companion is off." }))
}

/// The text to send: the words given, or stdin for `-`.
pub fn send_text(words: &[String]) -> Result<String, String> {
    if words.len() == 1 && words[0] == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
        return Ok(s.trim_end_matches(['\r', '\n']).to_string());
    }
    Ok(words.join(" "))
}

#[cfg(test)]
#[path = "hub_api_tests.rs"]
mod tests;
