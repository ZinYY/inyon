//! What each command does, apart from drawing. Everything here prints plain text, or JSON with
//! `--json` (`{"v":1,…}`), so scripts and the app can read it.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::engine;
use crate::http;
use crate::networks;
use crate::paths::{self, bin_dir, data_dir, node_dir, read_env_file, saved_env, saved_hub};
use crate::service::{self, Installed, Role, Sys, ROLES};
use crate::state;
use crate::sys::{self, which, Platform, RealProc};
use crate::ui::setup::{Bind, Plan, RoleChoice};
use crate::ui::{self, Theme, VERSION};
use crate::update;

pub type Outcome = Result<Value, String>;

/// A command's result as text or JSON, and the exit code it ends with.
pub fn finish(r: Outcome, json: bool) -> i32 {
    match r {
        // the command printed its answer itself (`pair`, whose answers come one at a time)
        Ok(Value::Null) => 0,
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&with_v(v)).unwrap_or_default());
            } else if let Some(text) = v.get("message").and_then(|m| m.as_str()) {
                if !text.is_empty() {
                    println!("{text}");
                }
            }
            0
        }
        Err(e) => {
            if json {
                println!("{}", json!({ "v": 1, "ok": false, "error": e }));
            } else {
                eprintln!("{e}");
            }
            1
        }
    }
}

pub(crate) fn with_v(mut v: Value) -> Value {
    if let Value::Object(m) = &mut v {
        m.insert("v".into(), json!(1));
        m.entry("ok").or_insert(json!(true));
    }
    v
}

fn installed(r: &Installed) -> Value {
    json!({ "ok": r.ok, "how": r.how })
}

/// A command the status screen hands over to when you press its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    /// `wizard`: the screens that ask what to set up, as `inyon setup` typed in a terminal shows
    Setup {
        wizard: bool,
    },
    Pair,
}

pub fn named(name: &str) -> Option<Named> {
    match name {
        // pressed on a screen, so a person is there to answer the wizard
        "setup" => Some(Named::Setup { wizard: true }),
        "pair" => Some(Named::Pair),
        _ => None,
    }
}

/// Runs `name` as if typed, from the dashboard's shortcuts.
pub fn run_named(name: &str) {
    let proc = RealProc;
    let sys = Sys::new(&proc);
    let theme = Theme::detect(true);
    match named(name) {
        Some(Named::Setup { wizard }) => {
            let code = finish(setup(&sys, theme, None, wizard), false);
            std::process::exit(code);
        }
        Some(Named::Pair) => {
            let code = finish(pair(&[], false), false);
            std::process::exit(code);
        }
        None => {}
    }
}

// ---------- version, phones, pair, forget, push ----------

pub fn version(json: bool) -> Outcome {
    if json {
        return Ok(json!({ "cli": VERSION, "engine": engine::version(), "message": format!("inyon {VERSION}") }));
    }
    Ok(json!({ "message": format!("inyon {VERSION}") }))
}

pub fn phones() -> Outcome {
    let all = state::phones();
    if all.is_empty() {
        return Ok(json!({ "phones": [], "message": "No phones paired yet. Run `inyon pair`." }));
    }
    let mut lines = Vec::new();
    let mut list = Vec::new();
    for (key, p) in &all {
        let seen = p.last_seen.map(state::format_time).unwrap_or_else(|| "never".into());
        lines.push(format!("{:<24} {}  paired {}, last seen {seen}", p.name, key.chars().take(12).collect::<String>(), state::format_date(p.paired_at)));
        list.push(json!({ "key": key, "name": p.name, "pairedAt": p.paired_at, "lastSeen": p.last_seen }));
    }
    Ok(json!({ "phones": list, "message": lines.join("\n") }))
}

/// Runs the engine's command and passes its output through; the exit code is the engine's.
fn through(args: Vec<String>) -> Outcome {
    let status = engine::run_through(&args)?;
    if status.success() {
        Ok(json!({ "message": "" }))
    } else {
        Err(format!("inyon-engine {} ended with {}", args.first().cloned().unwrap_or_default(), status.code().unwrap_or(1)))
    }
}

/// The pairing code as the terminal shows it: one of the engine's answers (`pair --json`) drawn
/// here, or the line that the tunnel is still starting. The link is left out in tmux unless
/// `--show-link` (inyon_engine::output::link_hidden).
#[cfg(test)]
fn render_pair(v: &Value) -> String {
    inyon_engine::output::render_pair_with(v, false)
}

/// `inyon pair`: the engine makes the secret and publishes it; the code is drawn here, again when
/// the tunnel's address arrives after it. With `json` each of the engine's answers is printed as
/// it comes, one JSON line each; the last one is current.
pub fn pair(args: &[String], json: bool) -> Outcome {
    use std::io::{IsTerminal, Write};
    // the command's own flag: an engine from before 0.20.3 wouldn't know it
    let hide_link = inyon_engine::output::link_hidden(args.iter().any(|a| a == "--show-link"));
    let args: Vec<String> = args.iter().filter(|a| *a != "--show-link").cloned().collect();
    let args = pair_args(&args, json, std::io::stdout().is_terminal());
    // the Inyon app runs the link on Windows: it starts it, so the code comes from its link
    // rather than one this command would start and keep running
    link_in_app(&crate::sys::RealProc);
    engine::pair(&args, &mut |v| {
        if json {
            println!("{}", serde_json::to_string(&with_v(v.clone())).unwrap_or_default());
        } else {
            print!("{}", inyon_engine::output::render_pair_with(v, hide_link));
        }
        let _ = std::io::stdout().flush();
    })?;
    // printed already
    Ok(Value::Null)
}

/// Has the running Inyon app run the link, and waits up to 20 s for it to answer.
fn link_in_app(proc: &dyn sys::Proc) {
    let Some(a) = crate::app::running(proc) else { return };
    if a.runs(Role::Link) {
        return;
    }
    if crate::app::ask(proc, crate::app::Do::Start, Some(Role::Link), None, 5000).is_err() {
        return;
    }
    eprintln!("The Inyon app is starting the link…");
    let port = state::link_config().port.unwrap_or(4780);
    for _ in 0..40 {
        if service::link_answers(port) {
            return;
        }
        proc.sleep_ms(500);
    }
}

/// What the engine's `pair` gets. Drawn for no terminal (an installer in a script or a container),
/// the code after the first wait is the last: nobody is there for the one that follows the tunnel.
fn pair_args(args: &[String], json: bool, terminal: bool) -> Vec<String> {
    let mut a = args.to_vec();
    if !json && !terminal {
        a.push("--once".into());
    }
    a
}

pub fn forget(which: &str) -> Outcome {
    let mut v = engine::ask(&["forget", which], &[])?;
    let name = v.get("forgotten").and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or(which).to_string();
    v["message"] = json!(format!("Forgot {name}. It can't connect until it pairs again."));
    Ok(v)
}

/// `inyon push`: the list is drawn here; `set` and `test` carry the app's JSON on stdin, so they
/// go straight through to the engine.
pub fn push(args: &[String]) -> Outcome {
    if args.first().is_some_and(|a| a == "set" || a == "test") {
        let mut all = vec!["push".to_string()];
        all.extend(args.iter().cloned());
        return through(all);
    }
    let mut v = engine::ask(&["push", "list"], &[])?;
    let browsers: Vec<Value> = v.get("browsers").and_then(Value::as_array).cloned().unwrap_or_default();
    let lines: Vec<String> = browsers
        .iter()
        .map(|b| {
            let name = b.get("name").and_then(Value::as_str).unwrap_or("forgotten phone");
            let done = b.get("done").and_then(Value::as_bool).unwrap_or(true);
            format!("{:<24} {}{}", name, b.get("endpoint").and_then(Value::as_str).unwrap_or(""), if done { "" } else { " (needs-you only)" })
        })
        .collect();
    v["message"] = json!(if lines.is_empty() { "No browser gets notifications from this machine yet.".to_string() } else { lines.join("\n") });
    Ok(v)
}

/// `inyon hub code`: the setup code a new hub asks for, from the engine.
pub fn hub_code() -> Outcome {
    engine::ask(&["hub", "code"], &[])
}

/// `inyon hub companion-code`: a one-time code for a companion app, from the running hub.
pub fn hub_companion_code() -> Outcome {
    engine::ask(&["hub", "companion-code"], &[])
}

/// `inyon hub reset-password [password]`: asked for here when not given, handed to the engine in
/// its environment rather than on the process list. The engine writes it; a hub running in the
/// background is restarted here to pick it up.
pub fn hub_reset_password(sys: &Sys, password: Option<&str>) -> Outcome {
    let pw = match password {
        Some(p) => p.to_string(),
        None => ui::ask_hidden("New Inyon password: ")?,
    };
    if pw.is_empty() {
        return Err("No password given.".into());
    }
    let mut v = engine::ask(&["hub", "reset-password"], &[("INYON_NEW_PASSWORD", &pw)])?;
    if !service::has_service(sys, Role::Hub) {
        return Ok(v);
    }
    let r = service::restart_service(sys, Role::Hub, &[paths::installed_binary().to_string_lossy().into_owned()]);
    v["restarted"] = json!(r.ok);
    v["message"] = json!(if r.ok {
        "Password updated; everyone was signed out. The hub restarted.".to_string()
    } else {
        format!("Password updated; everyone was signed out. Restart the hub to use it ({}).", r.how)
    });
    Ok(v)
}

// ---------- join, disconnect ----------

/// Checks a hub address; refuses to send a token over plain http to anything but this machine or a
/// VPN address. A name has to point only at those (a MagicDNS name does; a LAN's .local doesn't).
/// What `inyon join` says when it's missing the hub or the token.
pub const JOIN_USAGE: &str = "usage: INYON_TOKEN=<token> inyon join <hub-url> (or paste the whole command from the hub's Machines page, Add a machine)";

/// What `inyon join` and setup say to a phone's pairing link.
pub const PHONE_LINK: &str = "That link pairs a phone with the hub; to add this computer, run the command from the hub's Machines page (Add a machine) here instead.";

/// Whether `text` is a link that pairs a phone (the hub's QR code or a machine's), not a hub.
pub fn is_phone_link(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("inyon://") || t.contains("#hub?") || t.contains("#pair?") || t.contains("#pair&") || t.ends_with("#pair")
}

/// What the hub's "Add a machine" command says: the hub, and the token and certificate it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCommand {
    pub hub: String,
    pub token: Option<String>,
    pub cert: Option<String>,
}

/// Reads what was pasted into setup's Join screen: the hub's Add a machine command whole (the
/// Linux and macOS one or the Windows one), or only the hub's address. A phone's pairing link is
/// refused with where the command is.
pub fn parse_add_machine(text: &str) -> Result<MachineCommand, String> {
    let t = text.trim();
    if is_phone_link(t) {
        return Err(PHONE_LINK.into());
    }
    let word_after = |key: &str| -> Option<String> {
        let at = t.find(key)? + key.len();
        let w: String = t[at..].chars().take_while(|c| !c.is_whitespace() && *c != '\'' && *c != '"').collect();
        (!w.is_empty()).then_some(w)
    };
    let quoted_after = |key: &str| -> Option<String> {
        let at = t.find(key)? + key.len();
        let rest = &t[at..];
        let end = rest.find('\'')?;
        (end > 0).then(|| rest[..end].to_string())
    };
    let url = |w: &str| (w.starts_with("https://") || w.starts_with("http://")).then(|| w.trim_end_matches('/').to_string());
    // Windows: powershell … install.ps1 … -Hub '<hub>' -Token '<token>'
    if let Some(hub) = quoted_after("-Hub '").and_then(|h| url(&h)) {
        return Ok(MachineCommand { hub, token: quoted_after("-Token '"), cert: quoted_after("-Cert '") });
    }
    // Linux and macOS: INYON_TOKEN=<token> … sh -c '<script>' inyon <hub> <sha256>
    if let Some(token) = word_after("INYON_TOKEN=") {
        let after_script = t.rfind("' inyon ").map(|i| &t[i + "' inyon ".len()..]);
        let hub = after_script.and_then(|rest| rest.split_whitespace().next()).and_then(url).or_else(|| t.split_whitespace().find_map(url));
        let Some(hub) = hub else {
            return Err("That command has a token but no hub address. Copy the whole command from the hub's Machines page (Add a machine).".into());
        };
        return Ok(MachineCommand { hub, token: Some(token), cert: word_after("INYON_HUB_CERT=") });
    }
    if let Some(hub) = url(t).filter(|_| !t.contains(char::is_whitespace)) {
        return Ok(MachineCommand { hub, token: None, cert: None });
    }
    Err("Paste the command from the hub's Machines page (Add a machine), or the hub's address (https://…).".into())
}

pub fn check_hub_url(hub: &str) -> Result<(), String> {
    check_hub_url_with(hub, &|host, port| inyon_engine::hub::check_name_now(host, port))
}

/// Looks a name up and checks where it points (inyon_engine::hub::check_name_now).
type LookUp<'a> = &'a dyn Fn(&str, u16) -> Result<(), Option<String>>;

/// Read the way the engine reads it (url::Url), so the host checked is the host connected to,
/// whatever an `@`, `?` or `#` in the address says.
fn check_hub_url_with(hub: &str, look_up: LookUp) -> Result<(), String> {
    let bad = || format!("{hub} isn't a web address.");
    let url = url::Url::parse(hub).map_err(|_| bad())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(bad());
    }
    if url.scheme() == "https" || paths::env_var("INYON_ALLOW_INSECURE").is_some() {
        return Ok(());
    }
    let host = url.host_str().unwrap_or_default();
    match inyon_engine::hub::plain_host(&url) {
        inyon_engine::hub::Plain::Ok => Ok(()),
        inyon_engine::hub::Plain::LookUp => match look_up(host, url.port_or_known_default().unwrap_or(80)) {
            Ok(()) => Ok(()),
            Err(Some(why)) => Err(why),
            Err(None) => Err(format!(
                "Couldn't look up {host}, so it can't be checked that plain http to it stays on this machine or a VPN. Check the address, or use the hub's https address."
            )),
        },
        inyon_engine::hub::Plain::Refused => Err(inyon_engine::hub::plain_refusal(host)),
    }
}

/// A hub on a certificate it made itself (setup's LAN choice) puts the certificate's sha256 in its
/// "Add a machine" command as INYON_HUB_CERT: the certificate it hands out is kept if it has that
/// sha256, and from then on everything this machine asks the hub checks against it. Joining
/// without one forgets the certificate of a hub joined before.
fn trust_hub_cert(hub: &str, sha256: Option<&str>, get: &dyn Fn(&str) -> Result<http::Response, String>, file: &std::path::Path) -> Result<(), String> {
    let Some(sha256) = sha256 else {
        let _ = fs::remove_file(file);
        return Ok(());
    };
    if !hub.starts_with("https://") {
        return Err(format!("INYON_HUB_CERT names a certificate, but {hub} isn't an https address."));
    }
    let url = format!("{hub}/node/hub.crt");
    let res = get(&url).map_err(|e| format!("Couldn't reach {url} ({e})."))?;
    if !res.ok() {
        return Err(format!("{url} answered {}. Update the hub, then get a new command from it.", res.status));
    }
    let pem = inyon_engine::http::pinned_cert(&res.body, sha256)?;
    fs::write(file, pem).map_err(|e| format!("Couldn't save the hub's certificate to {}: {e}", file.display()))
}

/// What the hub said to `inyon join` swapping the pasted token for its own.
#[derive(Debug, PartialEq)]
enum Claim {
    /// the token to keep; the pasted one no longer works
    Swapped(String),
    /// keep the pasted one: a hub from before 0.20.3, a machine an older inyon already paired with
    /// it, or a hub that couldn't be reached just now
    Kept,
    /// the pasted token was used already, replaced or expired
    Refused(String),
}

/// The token in the "Add a machine" command sits in shell history, a terminal's scrollback and, on
/// Windows, a task's command line. `inyon join` trades it for one the hub hands out once, over the
/// connection its certificate was just checked on, so the pasted one stops working.
fn claim_token(hub: &str, token: &str, post: &dyn Fn(&str, &str) -> Result<inyon_engine::http::Response, String>) -> Claim {
    let Ok(res) = post(&format!("{hub}/node/claim"), token) else {
        return Claim::Kept;
    };
    match res.status {
        200 => serde_json::from_slice::<Value>(&res.body)
            .ok()
            .and_then(|v| v["token"].as_str().map(str::to_owned))
            .filter(|t| t.len() >= 32 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            // an older hub answers with its web page
            .map_or(Claim::Kept, Claim::Swapped),
        403 => Claim::Refused(format!(
            "The hub refused this token: {}. Get a new command from the hub's Add a machine.",
            String::from_utf8_lossy(&res.body).trim().trim_end_matches('.')
        )),
        _ => Claim::Kept,
    }
}

/// `inyon join <hub-url>`, what the hub's "Add a machine" command runs: saves the hub and token
/// (readable only by you) and runs the connection in the background.
pub fn join(sys: &Sys, hub: &str, token: &str, cert: Option<&str>, autostart: bool) -> Outcome {
    let hub = hub.trim().trim_end_matches('/');
    if is_phone_link(hub) {
        return Err(PHONE_LINK.into());
    }
    if hub.is_empty() || token.is_empty() {
        return Err(JOIN_USAGE.into());
    }
    check_hub_url(hub)?;
    let dir = paths::ensure_node_dir().map_err(|e| e.to_string())?;
    let before = paths::saved_env();
    let pin = cert.filter(|c| !c.trim().is_empty());
    trust_hub_cert(hub, pin, &|url| http::get_unverified(url), &inyon_engine::http::hub_cert_file())?;
    let win = sys.platform == Platform::Windows;
    // the version that joined, so the connection knows this address passed this check (one saved
    // before 0.20 on plain http still connects, with a warning); and INYON_ALLOW_INSECURE if it was needed
    let marks = inyon_engine::connect::join_marks(hub);
    let file = dir.join(if win { ".env" } else { "env" });
    let save = |token: &str| -> Result<(), String> {
        let mut lines = vec![format!("INYON_HUB={hub}"), format!("INYON_TOKEN={token}")];
        lines.extend(marks.iter().cloned());
        if win {
            if let Some(sh) = paths::env_var("INYON_SH") {
                lines.push(format!("INYON_SH={sh}"));
            }
        }
        write_secret(&file, &format!("{}\n", lines.join("\n"))).map_err(|e| e.to_string())
    };
    save(token)?;
    // the other file name may hold a token from an earlier pairing
    let _ = fs::remove_file(dir.join(if win { "env" } else { ".env" }));
    if win {
        sys::run(&["icacls", &file.to_string_lossy(), "/inheritance:r", "/grant:r", &format!("{}:(R,W)", sys::username()), "SYSTEM:(F)"]);
    }
    // saved first, so the swap goes over the hub's pinned certificate like the connection will
    let post = |url: &str, t: &str| inyon_engine::http::request("POST", url, &[("authorization", &format!("Bearer {t}"))], Some(&[]), std::time::Duration::from_secs(10));
    match claim_token(hub, token, &post) {
        Claim::Swapped(fresh) => save(&fresh)?,
        Claim::Kept => {}
        Claim::Refused(why) => {
            // the same command run again on a machine it already added: keep what that saved
            let same_hub = before.get("INYON_HUB").is_some_and(|h| h.trim_end_matches('/') == hub);
            match before.get("INYON_TOKEN").filter(|t| same_hub && !t.is_empty() && t.as_str() != token) {
                Some(kept) => save(kept)?,
                None => {
                    let _ = fs::remove_file(&file);
                    return Err(why);
                }
            }
        }
    }
    for role in service::old_services(sys) {
        if role == Role::Node {
            service::move_old_service(sys, role);
        }
    }
    let mut notes = Vec::new();
    if let Some(m) = service::missing_mux(sys.platform) {
        notes.push(format!("Note: {m}"));
    }
    if !autostart {
        notes.push(format!("Saved. Run `inyon connect` to connect to {}.", http::host_of(hub)));
        return Ok(json!({ "hub": hub, "message": notes.join("\n") }));
    }
    let r = service::install_service(sys, Role::Node, true);
    if !r.ok {
        return Err(format!("Couldn't run the connection in the background ({}). Run `inyon connect` to connect now.", r.how));
    }
    notes.push(format!("inyon is connecting to {}. It runs as {}.", http::host_of(hub), r.how));
    Ok(json!({ "hub": hub, "service": installed(&r), "message": notes.join("\n") }))
}

/// Leaves the hub: the connection stops and the saved address and token go.
pub fn disconnect(sys: &Sys) -> Outcome {
    let hub = saved_hub();
    // the hub is forgotten first: run by a node the hub removed, stopping the service last may
    // stop this process with it
    let dir = node_dir();
    for name in ["env", ".env"] {
        let _ = fs::remove_file(dir.join(name));
    }
    let _ = fs::remove_file(dir.join("node.json"));
    let _ = fs::remove_file(inyon_engine::http::hub_cert_file());
    let r = service::uninstall_service(sys, Role::Node);
    let what = hub.as_deref().map(http::host_of).unwrap_or_else(|| "the hub".into());
    if !r.ok {
        return Err(format!("Left {what}, but couldn't remove everything: {}.", r.how));
    }
    Ok(json!({ "hub": hub, "message": format!("Left {what}; {}.", r.how) }))
}

/// `inyon stop`: everything inyon runs in the background here stops (the Inyon app with all it
/// runs on Windows, the user services elsewhere) until the next sign-in or `inyon setup`. The hub
/// keeps running; `inyon hub stop` stops it.
pub fn stop(sys: &Sys) -> Outcome {
    let mut stopped: Vec<String> = Vec::new();
    if sys.platform == Platform::Windows {
        if let Some(a) = crate::app::running(sys.proc) {
            let what = if a.running.is_empty() { "the Inyon app".to_string() } else { format!("the Inyon app and what it ran ({})", a.running.join(", ")) };
            match crate::app::ask(sys.proc, crate::app::Do::Quit, None, None, 10_000) {
                Ok(_) => {
                    stopped.push(what);
                    // what it ran ends with its job
                    sys.proc.sleep_ms(500);
                }
                Err(e) => return Err(format!("{e} Quit Inyon in the tray icon's menu instead.")),
            }
        }
    }
    // what runs outside the app: an older version's engine, a service, a copy started in the background
    for role in [Role::Link, Role::Node] {
        let running = update::read_running(role).and_then(|r| r.pid).filter(|p| *p != sys.pid && sys.proc.alive(*p));
        let what = if role == Role::Link { "the phone link" } else { "the hub connection" };
        let r = service::stop_now(sys, role, running);
        if r || running.is_some() {
            stopped.push(what.to_string());
        }
    }
    if stopped.is_empty() {
        return Ok(json!({ "stopped": [], "message": "Nothing of inyon's runs in the background here." }));
    }
    let again = match sys.platform {
        Platform::Windows => "Opening Inyon from the Start menu, or the next sign-in, starts it again.",
        _ => "The next sign-in starts it again; `inyon service uninstall` turns that off.",
    };
    Ok(json!({ "stopped": stopped, "message": format!("Stopped {}. {again}", stopped.join(" and ")) }))
}

// ---------- services ----------

/// The job `service install` or `uninstall` is about: the one named, or the one this machine has.
pub fn role_arg(sys: &Sys, arg: Option<&str>, installing: bool) -> Result<Role, String> {
    if let Some(a) = arg {
        return Role::parse(a).ok_or_else(|| format!("{a} isn't one of link, node, hub"));
    }
    if saved_hub().is_some() {
        return Ok(Role::Node);
    }
    if !installing {
        let have: Vec<Role> = ROLES.iter().copied().filter(|r| service::has_service(sys, *r)).collect();
        if have.len() == 1 {
            return Ok(have[0]);
        }
    }
    Ok(Role::Link)
}

pub fn service_install(sys: &Sys, role: Option<&str>) -> Outcome {
    let role = role_arg(sys, role, true)?;
    say_retired();
    let r = service::install_service(sys, role, true);
    if !r.ok {
        return Err(format!("Couldn't install the service: {}", r.how));
    }
    Ok(json!({ "role": role.name(), "service": installed(&r), "message": format!("inyon {role} runs as {}.", r.how) }))
}

/// `inyon service install` as typed: for the link, cloudflared is fetched first, so the service
/// finds it and the first pairing doesn't wait for the download.
pub fn service_install_command(sys: &Sys, role: Option<&str>) -> Outcome {
    let note = if role_arg(sys, role, true)? == Role::Link { fetch_tunnel() } else { None };
    let mut v = service_install(sys, role)?;
    if let Some(n) = note {
        v["message"] = json!(format!("{n}\n{}", v["message"].as_str().unwrap_or_default()));
    }
    Ok(v)
}

/// Gets cloudflared now when the link will open a tunnel and it isn't here yet (about 40 MB from
/// GitHub, checked against its pinned checksum). Without it the link downloads it as it starts,
/// and the first pairing waits for that. A failure is a note: the link tries again itself.
pub fn fetch_tunnel() -> Option<String> {
    if !state::link_config().tunnel.unwrap_or(true) || inyon_engine::link::cloudflared_here().is_some() {
        return None;
    }
    eprintln!("Getting cloudflared for the tunnel…");
    Some(match inyon_engine::link::ensure_cloudflared() {
        Ok(_) => "✓ cloudflared is here for the tunnel".to_string(),
        Err(e) => format!("! Couldn't get cloudflared for the tunnel yet ({e}). The link tries again as it starts."),
    })
}

/// Says once that INYON_LEGACY_ENGINE, INYON_HUB_IMPL=bun or INYON_ENGINE asks for what 0.23
/// removed, and clears the notes older versions left (see engine::retired).
pub fn say_retired() {
    if let Some(line) = engine::retired() {
        eprintln!("Note: {line}");
    }
}

pub fn service_uninstall(sys: &Sys, role: Option<&str>) -> Outcome {
    let role = role_arg(sys, role, false)?;
    let r = service::uninstall_service(sys, role);
    // the link started without a service manager (or by hand): stop the copy that's still running
    if role == Role::Link {
        if let Some(pid) = state::link_state().and_then(|s| s.pid).filter(|p| *p != sys.pid && sys.proc.alive(*p)) {
            sys.proc.signal(pid, sys::Signal::Term);
        }
    }
    let kept = if role == Role::Link { " Your pairings are kept." } else { "" };
    if !r.ok {
        return Err(format!("Couldn't remove everything: {}.{kept}", r.how));
    }
    Ok(json!({ "role": role.name(), "service": installed(&r), "message": format!("inyon {role} stopped; {}.{kept}", r.how) }))
}

pub fn service_status(sys: &Sys) -> Outcome {
    let jobs: Vec<Value> = ROLES.iter().map(|r| json!({ "role": r.name(), "service": service::has_service(sys, *r), "running": update::is_running(sys.proc, *r) })).collect();
    let lines: Vec<String> = jobs
        .iter()
        .map(|j| {
            format!(
                "{:<5} {}{}",
                j["role"].as_str().unwrap_or(""),
                if j["service"].as_bool().unwrap_or(false) { "set up" } else { "not set up" },
                if j["running"].as_bool().unwrap_or(false) { ", running" } else { "" }
            )
        })
        .collect();
    Ok(json!({ "jobs": jobs, "message": lines.join("\n") }))
}

// ---------- the hub ----------

pub fn hub_env_file() -> PathBuf {
    data_dir().join("hub.env")
}

fn hub_env() -> std::collections::BTreeMap<String, String> {
    let mut env = read_env_file(&hub_env_file());
    // the hub's settings are text; a variable that isn't (in any locale) is none of them
    for (k, v) in std::env::vars_os() {
        if let (Ok(k), Ok(v)) = (k.into_string(), v.into_string()) {
            env.insert(k, v);
        }
    }
    env
}

/// The hub's own address on this machine, for asking it questions.
pub fn hub_local_url(env: &std::collections::BTreeMap<String, String>) -> String {
    let tls = env.get("TLS_CERT").is_some_and(|v| !v.is_empty()) && env.get("TLS_KEY").is_some_and(|v| !v.is_empty());
    let bind = env.get("BIND").cloned().filter(|b| !b.is_empty()).unwrap_or_else(|| "127.0.0.1".into());
    let host = if bind == "0.0.0.0" || bind == "::" || bind == "127.0.0.1" { "127.0.0.1".to_string() } else { bind };
    format!("{}://{host}:{}", if tls { "https" } else { "http" }, env.get("PORT").cloned().filter(|p| !p.is_empty()).unwrap_or_else(|| "4777".into()))
}

fn ask_hub(url: &str) -> Option<Value> {
    let res = http::get_local(&format!("{url}/api/session"), &[]).ok()?;
    serde_json::from_slice(&res.body).ok()
}

/// How to open a hub that answers on this machine only from a phone.
pub fn reach_advice(nets: &[networks::Network], port: u16, env_file: &str) -> Vec<String> {
    let private: Vec<&networks::Network> = nets.iter().filter(|n| n.kind != networks::Kind::Lan).collect();
    if private.is_empty() {
        return vec![format!("It answers on this machine only. To open it from your phone, put it behind HTTPS (Caddy, a tunnel) or set BIND in {env_file}.")];
    }
    let mut lines = vec!["To open it from your phone:".to_string()];
    for n in private {
        lines.push(format!(
            "  {}: add BIND={} and INYON_ALLOW_HTTP=1 to {env_file}, run `inyon hub start` again, then open http://{}:{port}",
            n.kind.label(),
            n.ip,
            n.name.as_deref().unwrap_or(&n.ip)
        ));
    }
    // the network encrypts it either way; browsers still treat plain http as unsafe
    lines.push("Over plain http the browser won't install the app or show notifications; HTTPS lets it.".into());
    lines
}

/// Sets the hub up as a background service and says where to open it, with the setup code.
pub fn hub_start(sys: &Sys) -> Outcome {
    if sys.platform == Platform::Windows {
        return Err("The Inyon hub runs on Linux and macOS. On Windows, install it inside WSL.".into());
    }
    say_retired();
    let env = hub_env();
    let r = service::install_service(sys, Role::Hub, true);
    if !r.ok {
        return Err(format!("Couldn't run the hub in the background: {}. Run `inyon hub` to start it here.", r.how));
    }
    let tls = env.get("TLS_CERT").is_some_and(|v| !v.is_empty()) && env.get("TLS_KEY").is_some_and(|v| !v.is_empty());
    let bind = env.get("BIND").cloned().filter(|b| !b.is_empty()).unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = env.get("PORT").and_then(|p| p.parse().ok()).unwrap_or(4777);
    let local_host = bind.starts_with("127.") || bind == "::1" || bind == "localhost" || bind == "0.0.0.0" || bind == "::";
    let where_ = format!("{}://{}:{port}", if tls { "https" } else { "http" }, if local_host { "localhost".to_string() } else { bind.clone() });
    let mut lines = vec![format!("The Inyon hub runs as {}.", r.how)];
    // a new hub writes a one-time code for choosing its password as it starts
    let local = hub_local_url(&env);
    let mut code = String::new();
    let answered = ui::with_motif("waiting for the hub", Theme::detect(true), move || {
        for _ in 0..60 {
            if let Some(session) = ask_hub(&local) {
                return Some(session);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        None
    });
    if answered.as_ref().and_then(|s| s.get("setup")).and_then(|v| v.as_bool()) == Some(true) {
        code = fs::read_to_string(data_dir().join("setup-code")).map(|c| c.trim().to_string()).unwrap_or_default();
    }
    lines.push(String::new());
    lines.push(if code.is_empty() {
        format!("Open {where_} in a browser and sign in.")
    } else {
        format!("Open {where_} in a browser and enter the setup code {code} to choose a password (`inyon hub code` shows it again).")
    });
    if bind == "127.0.0.1" && !tls {
        lines.push(String::new());
        lines.extend(reach_advice(&networks::detect(), port, &hub_env_file().to_string_lossy()));
    }
    Ok(json!({ "service": installed(&r), "address": where_, "code": code, "message": lines.join("\n") }))
}

pub fn hub_stop(sys: &Sys) -> Outcome {
    let r = service::uninstall_service(sys, Role::Hub);
    if !r.ok {
        return Err(format!("Couldn't remove everything: {}.", r.how));
    }
    Ok(json!({ "service": installed(&r), "message": format!("The hub stopped; {}. Its records stay in {}.", r.how, data_dir().display()) }))
}

pub fn hub_restart(sys: &Sys) -> Outcome {
    if !service::can_restart(sys, Role::Hub) {
        return Err("Couldn't restart the hub: no background service or process found.".into());
    }
    say_retired();
    let r = service::restart_service(sys, Role::Hub, &[paths::installed_binary().to_string_lossy().into_owned()]);
    if !r.ok {
        return Err(format!("Couldn't restart the hub: {}.", r.how));
    }
    Ok(json!({ "service": installed(&r), "message": format!("The hub restarted ({}).", r.how) }))
}

// ---------- logs, doctor ----------

pub fn logs(sys: &Sys, role: Option<&str>, lines: usize, follow: bool) -> Outcome {
    let role = match role {
        Some(r) => Role::parse(r).ok_or_else(|| format!("{r} isn't one of link, node, hub"))?,
        None => {
            let active = service::active_roles(sys);
            active.first().copied().unwrap_or(Role::Link)
        }
    };
    let file = service::log_file(role);
    let unit = format!("inyon-{role}");
    // a systemd unit logs to the journal, not the file
    if sys.platform == Platform::Linux && service::unit_file(&unit).exists() && which("journalctl").is_some() {
        let n = lines.to_string();
        let mut args = vec!["journalctl", "--user", "-u", &unit, "-n", &n, "--no-pager", "-o", "cat"];
        if follow {
            args.push("-f");
        }
        let mut c = std::process::Command::new(args[0]);
        c.args(&args[1..]);
        let status = sys::console_child(&mut c).status().map_err(|e| e.to_string())?;
        return if status.success() { Ok(json!({ "message": "" })) } else { Err("journalctl couldn't show the log".into()) };
    }
    let text = fs::read_to_string(&file).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    let tail: Vec<String> = all.iter().rev().take(lines).rev().map(|l| l.to_string()).collect();
    if !follow {
        if tail.is_empty() {
            return Ok(json!({ "file": file, "lines": [], "message": format!("Nothing in {} yet.", file.display()) }));
        }
        return Ok(json!({ "file": file, "lines": tail, "message": tail.join("\n") }));
    }
    println!("{}", tail.join("\n"));
    let mut seen = text.len() as u64;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let Ok(now) = fs::read(&file) else { continue };
        if (now.len() as u64) < seen {
            seen = 0;
        }
        if (now.len() as u64) > seen {
            print!("{}", String::from_utf8_lossy(&now[seen as usize..]));
            seen = now.len() as u64;
        }
    }
}

fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), std::time::Duration::from_millis(300)).is_ok()
}

/// How phones reach the link, for `doctor`: whether its port answers and, on Windows, whether
/// the firewall lets the same network in, or that it's the tunnel only and how to change that.
/// On Windows the link answers on 127.0.0.1 alone (for the tunnel) unless LAN or VPN is on, so its
/// port is checked only then; elsewhere it's always checked, as before.
fn link_reach(platform: Platform, cfg: &state::LinkConfig, answers: &dyn Fn(u16) -> bool, firewall_allows: &dyn Fn(u16) -> bool) -> Vec<(bool, String, String)> {
    let windows = platform == Platform::Windows;
    let port = cfg.port.unwrap_or(4780);
    let mut out = Vec::new();
    let direct = cfg.lan.unwrap_or(false) || cfg.vpn.unwrap_or(!windows);
    if !windows || direct {
        out.push((true, "link port".to_string(), format!("{port}: {}", if answers(port) { "answers" } else { "nothing listens" })));
    }
    if windows && direct {
        if firewall_allows(port) {
            out.push((true, "firewall".to_string(), format!("the rule \"{}\" lets phones on your network and VPN in", service::FIREWALL_RULE)));
        } else {
            out.push((
                false,
                "firewall".to_string(),
                format!("Windows Firewall keeps phones on your network and VPN out, so they connect through the tunnel only. Run `inyon setup` and tick LAN or VPN to add the rule, or in PowerShell as administrator:\n  {}", service::firewall_command(port)),
            ));
        }
    }
    if let Some(note) = crate::ui::dashboard::tunnel_only(cfg, windows) {
        out.push((true, "reach".to_string(), note));
    }
    out
}

pub fn doctor(sys: &Sys) -> Outcome {
    let mut checks: Vec<(bool, String, String)> = Vec::new();
    checks.push((true, "engine".into(), format!("built in ({VERSION})")));
    if let Some(line) = engine::retired() {
        checks.push((false, "bun".into(), line));
    }
    checks.push((true, "inyon".into(), format!("{VERSION} at {}", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default())));
    match service::missing_mux(sys.platform) {
        None => checks.push((true, if sys.platform == Platform::Windows { "psmux" } else { "tmux" }.into(), "installed".into())),
        Some(m) => checks.push((false, "tmux".into(), m)),
    }
    let agents: Vec<&str> = ["claude", "codex", "gemini", "opencode"].into_iter().filter(|a| which(a).is_some()).collect();
    checks.push((!agents.is_empty(), "agents".into(), if agents.is_empty() { "none of claude, codex, gemini, opencode on PATH".into() } else { agents.join(", ") }));
    for r in ROLES {
        let has = service::has_service(sys, r);
        let running = update::is_running(sys.proc, r);
        if has || running {
            checks.push((running || !has, format!("{r}"), format!("{}{}", if has { "set up" } else { "started by hand" }, if running { ", running" } else { ", not running" })));
        }
    }
    let old = service::old_services(sys);
    if !old.is_empty() {
        checks.push((
            false,
            "old services".into(),
            format!("{} still set up as tether-node; `inyon migrate` moves them", old.iter().map(|r| r.name()).collect::<Vec<_>>().join(", ")),
        ));
    }
    checks.extend(link_reach(sys.platform, &state::link_config(), &|p| port_open(p), &service::firewall_allows));
    if service::has_service(sys, Role::Hub) {
        let port: u16 = read_env_file(&hub_env_file()).get("PORT").and_then(|p| p.parse().ok()).unwrap_or(4777);
        checks.push((port_open(port), "hub port".into(), format!("{port}: {}", if port_open(port) { "answers" } else { "nothing listens" })));
    }
    let nets = networks::detect();
    checks.push((
        true,
        "networks".into(),
        if nets.is_empty() { "no LAN or VPN address found".into() } else { nets.iter().map(|n| format!("{} {}", n.kind.label(), n.ip)).collect::<Vec<_>>().join(", ") },
    ));
    let cloudflared = which("cloudflared").is_some() || bin_dir().join("cloudflared").exists() || bin_dir().join("cloudflared.exe").exists();
    checks.push((true, "tunnel".into(), if cloudflared { "cloudflared is here".into() } else { "cloudflared isn't here yet; the link fetches it on first use".into() }));
    checks.push((true, "clock".into(), format!("{} UTC", state::format_utc(state::now_ms()))));
    if let Some(check) = inyon_engine::connect::saved_hub_check() {
        let host = saved_hub().map(|h| http::host_of(&h)).unwrap_or_default();
        // what the running connection last said, when it has a problem of its own
        let said = connection_note().filter(|(problem, _)| *problem);
        checks.push(match (check, said) {
            (Err(why), _) => (false, "hub".into(), why),
            (Ok(Some(warning)), _) => (false, "hub".into(), warning),
            (Ok(None), Some((_, why))) => (false, "hub".into(), why),
            (Ok(None), None) => (true, "hub".into(), host),
        });
    }
    if let Ok(s) = fs::read_to_string(update::status_file()) {
        let s = s.trim().to_string();
        checks.push((!s.starts_with("failed"), "last update".into(), s));
    }
    let lines: Vec<String> = checks.iter().map(|(ok, name, detail)| format!("{} {:<12} {detail}", if *ok { "✓" } else { "!" }, name)).collect();
    let list: Vec<Value> = checks.iter().map(|(ok, name, detail)| json!({ "ok": ok, "check": name, "detail": detail })).collect();
    let all_ok = checks.iter().all(|c| c.0);
    Ok(json!({ "ok": all_ok, "checks": list, "message": lines.join("\n") }))
}

/// What the hub connection last wrote about itself in node.json, while it runs: (true, why) when
/// it can't connect, (false, warning) when it connects in a way worth knowing about (plain http to a
/// hub joined before 0.20).
pub fn connection_note() -> Option<(bool, String)> {
    let v: Value = serde_json::from_str(&fs::read_to_string(paths::node_dir().join("node.json")).ok()?).ok()?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    text("problem").map(|p| (true, p)).or_else(|| text("warning").map(|w| (false, w)))
}

// ---------- update ----------

struct RealHost<'a> {
    sys: &'a Sys<'a>,
    quiet: bool,
}

impl update::Host for RealHost<'_> {
    fn version(&self) -> String {
        VERSION.to_string()
    }
    fn roles(&self) -> Vec<Role> {
        service::active_roles(self.sys)
    }
    fn restart(&self, role: Role, cmd: &[String]) -> Installed {
        service::restart_service(self.sys, role, cmd)
    }
    fn started(&self, role: Role, version: Option<&str>, before: Option<u32>) -> bool {
        update::wait_started(self.sys.proc, role, version, before, &update::WaitOptions::default())
    }
    fn old_roles(&self) -> Vec<Role> {
        service::old_services(self.sys)
    }
    fn move_old(&self, role: Role) -> Installed {
        service::move_old_service(self.sys, role)
    }
    fn log(&self, line: &str) {
        if !self.quiet {
            println!("{line}");
        }
    }
    fn platform(&self) -> Platform {
        self.sys.platform
    }
}

/// Gets the newest version and restarts what runs in the background with it. With --background
/// (how the app asks for it) the update runs on its own, since the restart ends the process that asked.
pub fn update_command(sys: &Sys, background: bool, json: bool) -> Outcome {
    update::set_status("running");
    if background {
        let r = service::detach_update(sys, &sys.program);
        if !r.ok {
            update::set_status(&format!("failed: {}", r.how));
            return Err(format!("Couldn't start the update: {}", r.how));
        }
        return Ok(
            json!({ "background": true, "how": r.how, "message": format!("Updating inyon in the background ({}). Progress goes to {}.", r.how, node_dir().join("update.log").display()) }),
        );
    }
    let mut me = PathBuf::from(sys.program.first().cloned().unwrap_or_default());
    // npm's copy stays as npm installed it: update the one the services run from instead
    let npm = update::from_npm(&me);
    if npm {
        me = paths::installed_binary();
        if !me.exists() {
            update::set_status("failed: installed with npm");
            return Err(format!("This inyon came from npm. Update it with:\n  {}", update::NPM_UPDATE));
        }
    }
    let install = update::find_install(&me, saved_hub());
    let host = RealHost { sys, quiet: json };
    let result = update::update(&install, &host, &update::RealFiles);
    service::forget_task(sys, service::UPDATE_TASK);
    match result {
        Ok(mut r) => {
            update::set_status(&if r.changed { format!("done {}", r.version.clone().unwrap_or_default()).trim().to_string() } else { format!("current {VERSION}") });
            if npm {
                r.message = format!("{} The inyon command itself came from npm. Update it with:\n  {}", r.message, update::NPM_UPDATE);
            }
            if r.changed && sys.platform == Platform::Windows {
                // the Start menu, the version Settings → Apps shows, and logon tasks set up by an
                // earlier version, which open the Inyon app from now on
                if let Err(e) = service::adopt_app(sys) {
                    crate::app::trace(&format!("! after the update: {e}"));
                }
            }
            Ok(json!({ "changed": r.changed, "version": r.version, "message": r.message }))
        }
        Err(e) => {
            let m = e.to_string();
            update::set_status(&format!("failed: {m}"));
            Err(m)
        }
    }
}

/// `inyon migrate`: moves every job from its tether-node service to inyon's own, and from the Bun
/// engine to the one built in (0.19): Windows' logon tasks get the windowless copy of this program
/// in place of the Bun engine, and a Bun engine nothing needs any more goes. With `background` it
/// runs on its own and returns at once, as the tray icon starts it from inside a service.
pub fn migrate(sys: &Sys, background: bool) -> Outcome {
    if background {
        let mut args = sys.program.clone();
        args.push("migrate".into());
        let r = service::detach(sys, &args, service::HAND_OVER_TASK);
        return if r.ok { Ok(json!({ "background": true, "message": format!("Moving the services in the background ({}).", r.how) })) } else { Err(r.how) };
    }
    let dir = paths::ensure_node_dir().map_err(|e| e.to_string())?;
    let lock = dir.join("migrate.lock");
    if fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        return Ok(json!({ "message": "another copy is moving the old services already" }));
    }
    let mut lines = Vec::new();
    for role in service::old_services(sys) {
        let r = service::move_old_service(sys, role);
        lines.push(format!("{role}: {}", if r.ok { format!("moved; it runs as {}", r.how) } else { format!("not moved ({})", r.how) }));
    }
    lines.extend(to_builtin_engine(sys));
    let _ = fs::remove_file(&lock);
    service::forget_task(sys, service::HAND_OVER_TASK);
    if lines.is_empty() {
        lines.push("nothing to move".into());
    }
    Ok(json!({ "message": lines.join("\n") }))
}

/// Moves what still runs on the Bun engine to the engine built into this program. What it did,
/// a line each.
fn to_builtin_engine(sys: &Sys) -> Vec<String> {
    let mut lines = Vec::new();
    if sys.platform == Platform::Windows {
        let installed = paths::installed_engine();
        if !installed.is_file() || engine::is_builtin_copy(&installed) {
            return lines;
        }
        // an update from 0.18 checks for a few seconds that the engine it restarted stays up, and
        // puts the old version back if it has gone: the move waits until no update is restarting anything
        if !updates_settled(sys, 300, 2000) {
            lines.push("engine: an update is still running, so the move to the built-in engine waits for the next start".into());
            return lines;
        }
        let program = vec![paths::installed_binary().to_string_lossy().into_owned()];
        for role in [Role::Link, Role::Node] {
            if service::has_service(sys, role) {
                // registered again, the task runs the windowless copy, made in place of the Bun engine
                let r = service::restart_service(sys, role, &program);
                lines.push(format!("{role}: {}", if r.ok { "runs on the built-in engine now".to_string() } else { format!("still on the Bun engine ({})", r.how) }));
            }
        }
        if lines.is_empty() {
            if let Err(e) = engine::refresh_copy(&paths::installed_binary()) {
                lines.push(format!("engine: couldn't make the windowless copy ({e})"));
            }
        }
        // it ran: if it didn't take, the tray icon tries again an hour from now at the soonest
        let _ = fs::write(node_dir().join(HAND_OVER_RAN), state::now_ms().to_string());
    } else if let Some(stale) = engine::stale_engine() {
        if fs::remove_file(&stale).is_ok() {
            lines.push(format!("engine: removed {}, which nothing needs any more", stale.display()));
        }
    }
    lines
}

/// When the move to the built-in engine last ran, and when a tray icon last asked for it.
const HAND_OVER_RAN: &str = "engine-hand-over";
const HAND_OVER_ASKED: &str = "engine-hand-over-asked";

/// Whether no update is under way and the jobs that run are on this version, checked `tries`
/// times `every_ms` apart until it's so.
fn updates_settled(sys: &Sys, tries: u32, every_ms: u64) -> bool {
    let settled = || {
        !update::under_way()
            && [Role::Link, Role::Node].into_iter().all(|r| match update::read_running(r) {
                Some(s) => s.version.as_deref() == Some(VERSION) || !s.pid.is_some_and(|p| sys.proc.alive(p)),
                None => true,
            })
    };
    for _ in 1..tries {
        if settled() {
            return true;
        }
        sys.proc.sleep_ms(every_ms);
    }
    settled()
}

/// Whether the tray icon should move this machine to the built-in engine: on Windows, when the
/// logon task still starts the Bun engine (set up by 0.18 or earlier, updated by it). Once an hour
/// at most after a move that ran, in case it fails, and not twice at once (the link's and the hub
/// connection's icons start together).
pub fn wants_hand_over(sys: &Sys) -> bool {
    if sys.platform != Platform::Windows {
        return false;
    }
    let installed = paths::installed_engine();
    if !installed.is_file() || engine::is_builtin_copy(&installed) {
        return false;
    }
    let now = state::now_ms() as u64;
    let since = |name: &str| now.saturating_sub(fs::read_to_string(node_dir().join(name)).ok().and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(0));
    if since(HAND_OVER_RAN) < 3_600_000 || since(HAND_OVER_ASKED) < 900_000 {
        return false;
    }
    let _ = paths::ensure_node_dir();
    let _ = fs::write(node_dir().join(HAND_OVER_ASKED), now.to_string());
    true
}

/// Takes inyon off this machine: every service, the programs, and with `purge` the pairings too.
/// With `ask`, a terminal is there to answer whether tmux that setup installed goes as well.
pub fn uninstall_all(sys: &Sys, purge: bool, ask: bool) -> Outcome {
    let mut lines = Vec::new();
    // the Inyon app stops what it runs and ends, so its files can go
    if crate::app::running(sys.proc).is_some() {
        if let Ok(r) = crate::app::ask(sys.proc, crate::app::Do::Quit, None, None, 10_000) {
            lines.push(r.message);
        }
    }
    // asked first, while the record of what setup installed is still there
    let mux = service::remove_mux(sys, ask);
    for r in ROLES {
        if service::has_service(sys, r) || update::is_running(sys.proc, r) {
            let u = service::uninstall_service(sys, r);
            lines.push(format!("{r}: {}", u.how));
        }
    }
    if let Some(pid) = state::link_state().and_then(|s| s.pid).filter(|p| *p != sys.pid && sys.proc.alive(*p)) {
        sys.proc.signal(pid, sys::Signal::Term);
    }
    // on Windows the engines, the tray icons and the tunnel keep their files open until they stop
    service::stop_programs(sys);
    lines.extend(service::remove_firewall_rule(sys));
    service::unregister_app(sys);
    lines.extend(mux);
    let bin = bin_dir();
    let mut later = Vec::new();
    if bin.exists() {
        let me = std::env::current_exe().ok().and_then(|m| fs::canonicalize(m).ok());
        let mut left = Vec::new();
        for entry in fs::read_dir(&bin).into_iter().flatten().flatten() {
            let p = entry.path();
            if sys.platform == Platform::Windows && me.is_some() && fs::canonicalize(&p).ok() == me {
                // a running .exe can't be deleted: it goes once this command has ended
                later.push(p);
                continue;
            }
            let gone = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
            if gone.is_err() && p.exists() {
                left.push(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
        }
        let _ = fs::remove_dir(&bin);
        if left.is_empty() {
            lines.push(format!("removed {}", bin.display()));
        } else {
            lines.push(format!("couldn't remove {} in {} (in use; close it and run this again)", left.join(", "), bin.display()));
        }
    }
    // the PATH entry the installer added for it
    if sys.platform == Platform::Windows {
        let o = sys::run(&["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", crate::shellpath::WINDOWS_SCRIPT]);
        if o.ok && o.out.contains("removed") {
            lines.push(format!("took {} off your Path", bin.display()));
        }
    } else {
        lines.extend(crate::shellpath::remove_unix(&paths::home(), std::env::var_os("ZDOTDIR").map(PathBuf::from)));
    }
    if purge {
        // Claude Code and Antigravity stop calling Inyon's scripts before they're deleted
        lines.extend(crate::unhook::unhook(&paths::home()));
        let _ = fs::remove_dir_all(node_dir());
        // one entry at a time: a file something still has open (on Windows) stops only itself,
        // and bin, where this program runs from, goes after it has ended
        let mut left = Vec::new();
        for entry in fs::read_dir(home_inyon()).into_iter().flatten().flatten() {
            let p = entry.path();
            if p == bin {
                continue;
            }
            let gone = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
            if gone.is_err() && p.exists() {
                left.push(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
        }
        if !left.is_empty() {
            lines.push(format!("couldn't remove {} in {} (in use; close it and delete the folder)", left.join(", "), home_inyon().display()));
        }
        // what older versions and the SSH connections kept: the ~/.tether link, its folder, sockets
        for old in [".tether", ".tether-node", ".cache/inyon-ssh"] {
            let p = paths::home().join(old);
            if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                let _ = fs::remove_file(&p);
            } else {
                let _ = fs::remove_dir_all(&p);
            }
        }
        // the folder itself, now empty (on Windows it goes with bin once this command has ended)
        if sys.platform != Platform::Windows {
            let _ = fs::remove_dir(home_inyon());
        }
        lines.push("removed the pairings, keys, uploads and logs".into());
    } else {
        lines.push(format!("Your pairings stay in {}; add --purge to remove them too.", node_dir().display()));
    }
    service::delete_after_exit(sys, &later, &if purge { home_inyon() } else { bin });
    Ok(json!({ "message": lines.join("\n") }))
}

fn home_inyon() -> PathBuf {
    paths::home().join(".inyon")
}

// ---------- setup ----------

/// The plan from the flags, for a script or a terminal that can't draw.
pub struct SetupFlags {
    pub yes: bool,
    pub role: Option<String>,
    pub network: Option<String>,
    pub port: Option<u16>,
    pub autostart: Option<bool>,
    pub hub: Option<String>,
    /// no pairing code at the end: the Inyon window runs setup and shows its own
    pub no_code: bool,
}

pub fn plan_from_flags(platform: Platform, nets: Vec<networks::Network>, f: &SetupFlags) -> Result<Plan, String> {
    let mut plan = Plan::new(platform, nets);
    if let Some(r) = &f.role {
        plan.role = RoleChoice::parse(r).ok_or_else(|| format!("--role must be hub, node or join, not {r}"))?;
        if plan.role == RoleChoice::Hub {
            plan.port = "4777".into();
        }
    }
    if let Some(h) = &f.hub {
        // the hub's address, or its whole Add a machine command
        plan.set_join(h)?;
        if f.role.is_none() {
            plan.role = RoleChoice::Join;
        }
    }
    if let Some(n) = &f.network {
        plan.set_network(n)?;
    }
    if let Some(p) = f.port {
        plan.port = p.to_string();
    }
    if let Some(a) = f.autostart {
        plan.autostart = a;
    }
    if plan.role == RoleChoice::Hub && platform == Platform::Windows {
        return Err("The Inyon hub runs on Linux and macOS. On Windows, install it inside WSL.".into());
    }
    if plan.role == RoleChoice::Join && plan.hub_url.is_empty() {
        return Err("--hub <url> says which hub to join (with the token in INYON_TOKEN), or give it the hub's whole Add a machine command.".into());
    }
    Ok(plan)
}

/// `inyon setup`: the screens when there's a terminal, the flags otherwise, then the plan is
/// carried out and the engine shows the pairing code.
pub fn setup(sys: &Sys, theme: Theme, flags: Option<&SetupFlags>, interactive: bool) -> Outcome {
    let nets = networks::detect();
    let plan = match flags {
        Some(f) => plan_from_flags(sys.platform, nets, f)?,
        None => Plan::new(sys.platform, nets),
    };
    let ask = interactive && !flags.is_some_and(|f| f.yes);
    let plan = if ask {
        match ui::setup::run_interactive(plan, theme).map_err(|e| e.to_string())? {
            Some(p) => p,
            None => return Ok(json!({ "message": "Nothing changed." })),
        }
    } else {
        plan
    };
    if !ask {
        for (k, v) in plan.summary() {
            println!("{k:<12}{v}");
        }
    }
    apply(sys, &plan, ask, interactive, !flags.is_some_and(|f| f.no_code))
}

/// Carries the plan out. `ask` says whether to ask before installing tmux; `interactive` whether
/// a terminal is there at all (sudo can ask for a password).
pub fn apply(sys: &Sys, plan: &Plan, ask: bool, interactive: bool, code: bool) -> Outcome {
    let mut lines = vec![service::install_mux(sys, ask, interactive)];
    // Windows lists it under Settings → Apps, with Uninstall there running `inyon uninstall`
    lines.extend(service::register_app(sys));
    // and finds `inyon` by name in new windows, whichever way this copy got into ~/.inyon/bin
    let installed_here = std::env::current_exe().ok().and_then(|e| fs::canonicalize(e).ok()).is_some_and(|e| Some(e) == fs::canonicalize(paths::installed_binary()).ok());
    if sys.platform == Platform::Windows && installed_here && paths::env_var("INYON_NO_MODIFY_PATH").is_none() {
        let o = sys::run(&["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", crate::shellpath::WINDOWS_ADD_SCRIPT]);
        if o.ok && o.out.contains("added") {
            lines.push("✓ Put inyon on your Path: new windows find it by name".into());
        }
    }
    match plan.role {
        RoleChoice::Join => {
            // from the command pasted in setup, else the environment, as the hub's command sets it
            let token = Some(plan.hub_token.clone()).filter(|t| !t.is_empty()).or_else(|| paths::env_var("INYON_TOKEN")).unwrap_or_default();
            let cert = Some(plan.hub_cert.clone()).filter(|c| !c.is_empty()).or_else(|| paths::env_var("INYON_HUB_CERT"));
            let r = join(sys, &plan.hub_url, &token, cert.as_deref(), plan.autostart)?;
            lines.push(r["message"].as_str().unwrap_or("").to_string());
            Ok(json!({ "role": "join", "message": lines.join("\n") }))
        }
        RoleChoice::Hub => {
            let file = hub_env_file();
            let mut env = read_env_file(&file);
            env.insert("PORT".into(), plan.port_number().to_string());
            let make = || {
                let ips = cert_ips(plan.networks.iter().map(|n| n.ip.as_str()), &plan.bind);
                crate::hub_cert::make(&data_dir().join("tls"), &crate::hub_cert::names(&inyon_engine::hostname(), &ips))
            };
            lines.extend(hub_settings(&plan.bind, &mut env, &make)?);
            let _ = fs::create_dir_all(data_dir());
            fs::write(&file, env.iter().map(|(k, v)| format!("{k}={v}\n")).collect::<String>()).map_err(|e| e.to_string())?;
            if !plan.autostart {
                lines.push("Starting the hub here; keep this window open.".into());
                println!("{}", lines.join("\n"));
                engine::forward(&["hub".to_string()]);
            }
            let r = hub_start(sys)?;
            lines.push(r["message"].as_str().unwrap_or("").to_string());
            Ok(json!({ "role": "hub", "address": r["address"], "code": r["code"], "message": lines.join("\n") }))
        }
        RoleChoice::Node => {
            // the link reads its settings as it starts; setup is the one time this program writes them
            // read and written under the node's lock, so a relay `inyon pair` adds meanwhile stays
            let mut before = state::LinkConfig::default();
            let cfg = inyon_engine::link::change_node_json("link.json", |cfg: &mut state::LinkConfig| {
                before = cfg.clone();
                cfg.tunnel = Some(plan.tunnel);
                cfg.lan = Some(plan.lan);
                cfg.vpn = Some(plan.vpn);
                cfg.urls = Some(if plan.own_url.is_empty() { Vec::new() } else { vec![plan.own_url.clone()] });
                cfg.port = Some(plan.port_number());
            })?;
            if plan.tunnel {
                lines.extend(fetch_tunnel());
            }
            if plan.lan || plan.vpn {
                lines.extend(service::allow_link_through_firewall(sys, plan.port_number()));
            }
            if plan.autostart {
                let running = state::link_state().and_then(|s| s.pid).filter(|p| *p != sys.pid).is_some_and(|p| sys.proc.alive(p));
                let (svc, note) = if service::old_services(sys).contains(&Role::Link) {
                    // set up before the rename to Inyon: the link moves to inyon's own service
                    (service::move_old_service(sys, Role::Link), format!("✓ Restarted with version {VERSION}."))
                } else {
                    let stale = running && update::stale_link(state::link_state().as_ref(), &engine::version());
                    // the link reads its settings as it starts, so new ones need a restart
                    let changed = running && before != cfg;
                    if running && !stale && !changed {
                        (Installed { ok: true, how: String::new() }, "✓ Already running in the background".to_string())
                    } else {
                        let r = service::install_service(sys, Role::Link, stale || changed);
                        let note = if stale {
                            "✓ Restarted with the new version.".to_string()
                        } else if changed {
                            "✓ Restarted with the new settings.".to_string()
                        } else {
                            format!("✓ Runs as {}.", r.how)
                        };
                        (r, note)
                    }
                };
                if svc.ok {
                    lines.push(note);
                    for _ in 0..20 {
                        if state::link_state().and_then(|s| s.pid).is_some_and(|p| sys.proc.alive(p)) {
                            break;
                        }
                        sys.proc.sleep_ms(500);
                    }
                } else {
                    lines.push(format!("! Couldn't set up a background service ({}). Keep this window open instead.", svc.how));
                }
            }
            println!("{}", lines.join("\n"));
            if !code {
                return Ok(json!({ "role": "node", "message": "" }));
            }
            // the engine makes the secret and publishes it; the code is drawn here
            pair(&[], false)?;
            if plan.autostart {
                println!("Paired phones connect by themselves from now on. To pair another phone later, run `inyon pair`.");
            }
            Ok(json!({ "role": "node", "message": "" }))
        }
    }
}

/// The hub's BIND and how it's reached, for setup's network choice. Plain http only on a Tailscale
/// or NetBird address (INYON_ALLOW_HTTP), the ones a machine sends its token to over plain http; on
/// the LAN, ZeroTier, WireGuard or any other address, the hub serves HTTPS on TLS_CERT and TLS_KEY,
/// a certificate `make` makes unless you named your own. The lines to show: the certificate's
/// fingerprint, for comparing in the browser.
/// The addresses a hub's own certificate is made for: the private networks found, and the address
/// it was told to listen on, which may be none of them (a public one, typed in setup).
fn cert_ips<'a>(networks: impl Iterator<Item = &'a str>, bind: &Bind) -> Vec<std::net::IpAddr> {
    let mut ips: Vec<std::net::IpAddr> = networks.filter_map(|ip| ip.parse().ok()).collect();
    if let Bind::Vpn(a) | Bind::Own(a) = bind {
        if let Ok(ip) = a.trim().trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>() {
            if !ip.is_unspecified() && !ips.contains(&ip) {
                ips.push(ip);
            }
        }
    }
    ips
}

pub fn hub_settings(bind: &Bind, env: &mut std::collections::BTreeMap<String, String>, make: &dyn Fn() -> Result<crate::hub_cert::Made, String>) -> Result<Vec<String>, String> {
    let address = match bind {
        Bind::Localhost => "127.0.0.1".to_string(),
        Bind::Lan => "0.0.0.0".to_string(),
        Bind::Vpn(ip) => ip.clone(),
        Bind::Own(a) => a.clone(),
    };
    env.insert("BIND".into(), address.clone());
    let ip: Option<std::net::IpAddr> = address.parse().ok();
    let local = ip.is_some_and(|ip| ip.is_loopback()) || address == "localhost";
    // the same rule machines hold plain http to: a ZeroTier or WireGuard address gets a certificate too
    let vpn = !local && ip.is_some_and(|ip| !ip.is_unspecified() && inyon_engine::hub::plain_http_ip(ip));
    // set by an earlier setup for the LAN, it would serve plain http again
    if !vpn {
        env.remove("INYON_ALLOW_HTTP");
    }
    let has_tls = env.get("TLS_CERT").is_some_and(|v| !v.is_empty()) && env.get("TLS_KEY").is_some_and(|v| !v.is_empty());
    let mut lines = Vec::new();
    if vpn && !has_tls {
        env.insert("INYON_ALLOW_HTTP".into(), "1".into());
    } else if !local && !has_tls {
        let made = make()?;
        env.insert("TLS_CERT".into(), made.cert.to_string_lossy().into_owned());
        env.insert("TLS_KEY".into(), made.key.to_string_lossy().into_owned());
    }
    if !local && !vpn {
        if let Some(fp) = env.get("TLS_CERT").and_then(|c| crate::hub_cert::fingerprint(std::path::Path::new(c))) {
            lines.push(format!("✓ HTTPS, on a certificate made for this machine. The browser warns about it once; check that its SHA-256 fingerprint is\n  {fp}"));
        }
    }
    Ok(lines)
}

/// Everything the engine still has to say, for `inyon hub …` forms this program doesn't handle.
pub fn hub_other(args: &[String]) -> ! {
    if cfg!(windows) {
        eprintln!("The Inyon hub runs on Linux and macOS. On Windows, install it inside WSL.");
        std::process::exit(1);
    }
    let mut all = vec!["hub".to_string()];
    all.extend(args.iter().cloned());
    engine::forward(&all)
}

/// The saved hub, for the dashboard and `doctor`.
pub fn joined() -> bool {
    saved_hub().is_some() || saved_env().contains_key("INYON_HUB")
}

/// Writes a file only you can read, from its first byte: it holds the hub token.
fn write_secret(file: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        o.mode(0o600);
        // an older file keeps its mode when opened, so set it too
        let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o600));
    }
    o.open(file)?.write_all(content.as_bytes())
}

// ---------- skill ----------

/// `~/…` for a path under the home folder, so the lines stay short.
fn home_short(p: &std::path::Path) -> String {
    let home = paths::home();
    match p.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()).replace('\\', "/"),
        Err(_) => p.display().to_string(),
    }
}

fn skill_dirs(to: Option<PathBuf>) -> Vec<PathBuf> {
    to.map(|d| vec![d]).unwrap_or_else(|| crate::skills::targets(&paths::home()))
}

pub fn skill_list() -> Outcome {
    let (_, sums) = crate::skills::release(Platform::current())?;
    let dirs = skill_dirs(None);
    let names = crate::skills::available(&sums);
    if names.is_empty() {
        return Ok(json!({ "skills": [], "message": "This release has no skills to install." }));
    }
    let rows: Vec<Value> = names
        .iter()
        .map(|n| {
            let at: Vec<String> = crate::skills::installed(&dirs, n).iter().map(|p| home_short(p)).collect();
            json!({ "name": n, "installed": at })
        })
        .collect();
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let at: Vec<&str> = r["installed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            format!("  {:<12} {}", r["name"].as_str().unwrap_or(""), if at.is_empty() { "not installed".to_string() } else { format!("in {}", at.join(", ")) })
        })
        .collect();
    Ok(json!({ "skills": rows, "message": format!("Skills in this release:\n{}\nInstall one with: inyon skill add <name>", lines.join("\n")) }))
}

pub fn skill_add(name: &str, to: Option<PathBuf>, force: bool, json: bool) -> Outcome {
    if !crate::skills::valid_name(name) {
        return Err(format!("\"{name}\" isn't a skill name. See them with: inyon skill list"));
    }
    let (base, sums) = crate::skills::release(Platform::current())?;
    let log = |l: &str| {
        if !json {
            eprint!("{l}");
        }
    };
    let gz = crate::skills::download(&base, &sums, name, &log)?;
    let files = crate::skills::read_tarball(&gz, name)?;
    let version = update::latest_version(&base).unwrap_or_default();
    let mut done = Vec::new();
    for dir in skill_dirs(to) {
        done.push(home_short(&crate::skills::install(&dir, name, &files, &version, force)?));
    }
    Ok(json!({
        "name": name,
        "installed": done,
        "message": format!("Installed the {name} skill in:\n  {}\nYour agents pick it up in their next session.", done.join("\n  ")),
    }))
}

pub fn skill_remove(name: &str, to: Option<PathBuf>) -> Outcome {
    let mut gone = Vec::new();
    for dir in skill_dirs(to) {
        if crate::skills::remove(&dir, name)? {
            gone.push(home_short(&dir.join(name)));
        }
    }
    if gone.is_empty() {
        return Err(format!("No copy of {name} that Inyon installed was found. A skill you added yourself stays where you put it."));
    }
    Ok(json!({ "name": name, "removed": gone, "message": format!("Removed {name} from:\n  {}", gone.join("\n  ")) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hub_certificate_covers_the_address_it_was_told_to_listen_on() {
        let nets = ["192.168.1.20", "100.70.1.2"];
        let ips = |bind: &Bind| cert_ips(nets.iter().copied(), bind).iter().map(|i| i.to_string()).collect::<Vec<_>>();
        assert_eq!(ips(&Bind::Lan), ["192.168.1.20", "100.70.1.2"]);
        assert_eq!(ips(&Bind::Own("203.0.113.7".into())), ["192.168.1.20", "100.70.1.2", "203.0.113.7"]);
        assert_eq!(ips(&Bind::Vpn("10.147.17.5".into())), ["192.168.1.20", "100.70.1.2", "10.147.17.5"]);
        assert_eq!(ips(&Bind::Own("192.168.1.20".into())), ["192.168.1.20", "100.70.1.2"]);
        assert_eq!(ips(&Bind::Own("0.0.0.0".into())), ["192.168.1.20", "100.70.1.2"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_variable_that_is_not_utf8_does_not_stop_the_hub_commands() {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: a name nothing else reads
        unsafe { std::env::set_var("INYON_TEST_LATIN1", std::ffi::OsStr::from_bytes(b"caf\xe9")) };
        assert!(!hub_env().contains_key("INYON_TEST_LATIN1"));
    }

    #[test]
    fn plain_http_goes_only_to_this_machine_or_a_vpn() {
        let no_dns = |_: &str, _: u16| -> Result<(), Option<String>> { panic!("an address isn't looked up") };
        for h in ["localhost", "127.0.0.1", "100.100.1.1", "100.64.0.1", "[fd7a:115c:a1e0::1]", "[::1]"] {
            assert!(check_hub_url_with(&format!("http://{h}:4777"), &no_dns).is_ok(), "{h}");
        }
        // a LAN can be listened to, and anyone on it can answer as the hub
        for h in ["10.1.2.3", "192.168.0.4", "172.20.0.1", "8.8.8.8", "100.200.1.1", "[fd00::1]", "[fe80::1]", "[2001:db8::1]"] {
            let e = check_hub_url_with(&format!("http://{h}:4777"), &no_dns).unwrap_err();
            assert!(e.starts_with("Refusing to send the token over plain http") && e.contains("INYON_ALLOW_INSECURE=1"), "{h}: {e}");
        }
        assert!(check_hub_url("https://192.168.1.2:4777").is_ok());
        assert!(check_hub_url("https://hub.example.com").is_ok());
        assert!(check_hub_url("hub.example.com").unwrap_err().contains("isn't a web address"));
        // the host is the one connected to, whatever comes after an @ in the query or fragment
        for tricky in ["http://8.8.8.8:4777?@100.100.1.1", "http://8.8.8.8:4777/#@100.100.1.1", "http://8.8.8.8:4777?x=1@[::1]"] {
            assert!(check_hub_url_with(tricky, &no_dns).unwrap_err().starts_with("Refusing to send the token over plain http to 8.8.8.8"), "{tricky}");
        }
        assert!(check_hub_url_with("http://me:pw@100.100.1.1:4777/?next=@8.8.8.8", &no_dns).is_ok());
        // a name is looked up, and every address it has must pass
        let seen = std::cell::RefCell::new(Vec::new());
        let dns = |h: &str, p: u16| -> Result<(), Option<String>> {
            seen.borrow_mut().push(format!("{h}:{p}"));
            match h {
                "hub.tail1234.ts.net" => Ok(()),
                "box.local" => Err(Some(inyon_engine::hub::plain_refusal(h))),
                _ => Err(None),
            }
        };
        assert!(check_hub_url_with("http://hub.tail1234.ts.net:4777", &dns).is_ok());
        assert!(check_hub_url_with("http://box.local", &dns).unwrap_err().starts_with("Refusing"));
        assert!(check_hub_url_with("http://10.0.0.1.attacker.com:8080", &dns).unwrap_err().starts_with("Couldn't look up 10.0.0.1.attacker.com"));
        assert_eq!(*seen.borrow(), ["hub.tail1234.ts.net:4777", "box.local:80", "10.0.0.1.attacker.com:8080"]);
        // and the real lookup
        assert!(check_hub_url("http://localhost:4777").is_ok());
    }

    #[test]
    fn s_on_the_status_screen_opens_the_setup_wizard() {
        assert_eq!(named("setup"), Some(Named::Setup { wizard: true }));
        assert_eq!(named("pair"), Some(Named::Pair));
        assert_eq!(named("nope"), None);
    }

    #[test]
    fn doctor_says_windows_is_tunnel_only_and_checks_the_port_once_the_lan_or_vpn_is_on() {
        let cfg = |lan: Option<bool>, vpn: Option<bool>| state::LinkConfig { lan, vpn, ..Default::default() };
        let names = |checks: &[(bool, String, String)]| checks.iter().map(|c| c.1.clone()).collect::<Vec<_>>();
        let yes = |_: u16| true;
        let no = |_: u16| false;
        // Windows by default: the tunnel only, said plainly, with how to change it
        let w = link_reach(Platform::Windows, &cfg(None, None), &yes, &no);
        assert_eq!(names(&w), ["reach"]);
        assert!(w[0].0 && w[0].2.contains("only through the tunnel") && w[0].2.contains("firewall rule"), "{:?}", w);
        // LAN on: the port is checked, and the firewall rule with it
        let w = link_reach(Platform::Windows, &cfg(Some(true), None), &yes, &no);
        assert_eq!(names(&w), ["link port", "firewall"]);
        assert!(!w[1].0 && w[1].2.contains("New-NetFirewallRule") && w[1].2.contains("-LocalPort 4780"), "{:?}", w[1]);
        let w = link_reach(Platform::Windows, &cfg(None, Some(true)), &no, &yes);
        assert_eq!(w[0].2, "4780: nothing listens");
        assert!(w[1].0);
        // elsewhere: the port as before, and the VPN is on unless turned off
        assert_eq!(names(&link_reach(Platform::Linux, &cfg(None, None), &yes, &no)), ["link port"]);
        assert_eq!(names(&link_reach(Platform::Linux, &cfg(None, Some(false)), &yes, &no)), ["link port", "reach"]);
    }

    #[test]
    fn without_a_terminal_pair_shows_one_code_and_ends() {
        let none: Vec<String> = vec![];
        assert_eq!(pair_args(&none, false, false), vec!["--once".to_string()]);
        // a person at a terminal, and a program reading JSON lines, get the code again with the tunnel
        assert!(pair_args(&none, false, true).is_empty());
        assert!(pair_args(&none, true, false).is_empty());
    }

    #[test]
    fn the_pairing_code_is_drawn_from_the_engines_answer() {
        let v = json!({ "link": "inyon://pair?m=abc&s=secret&n=box", "qr": "https://inyon.tech/#pair?m=abc&s=secret&n=box", "short": { "id": "K7Q4MX2T", "code": "493812" }, "urls": ["wss://a.example/link"], "relays": [], "service": true });
        let out = render_pair(&v);
        assert!(out.contains("Scan this with your phone's camera"));
        assert!(out.contains("    ID    K7Q4-MX2T\n    Code  493 812\n"));
        assert!(out.contains("Or paste this link into the app:\ninyon://pair?m=abc&s=secret&n=box\n"));
        assert!(out.contains('█') && out.contains('▄'));
        assert!(!out.contains("No address yet"));
        assert!(!out.contains("Keep this running"));
        // no short code when the relays didn't take it; no address yet; the link runs here
        let v = json!({ "link": "inyon://pair?m=abc", "qr": "https://x/#pair?m=abc", "urls": [], "relays": [], "service": false });
        let out = render_pair(&v);
        assert!(!out.contains("Type a code"));
        assert!(out.contains("No address yet"));
        assert!(out.ends_with("Keep this running so the phone can connect, or run `inyon service install` to keep it running in the background.\n"));
    }

    #[test]
    fn while_the_tunnel_starts_the_terminal_says_so_instead_of_a_code_that_cant_connect() {
        let out = render_pair(&json!({ "waiting": "tunnel", "service": false }));
        assert!(out.contains("Starting the tunnel") && !out.contains("Scan this"), "{out}");
        // half a minute on, still nothing to reach: no code yet either
        let out = render_pair(&json!({ "waiting": "tunnel", "link": "inyon://pair?m=a", "qr": "https://x/#pair?m=a", "urls": [], "relays": [], "service": false }));
        assert!(out.contains("still starting") && !out.contains("Scan this"), "{out}");
        // a VPN address works meanwhile: the code, and that it comes again with the tunnel
        let out = render_pair(
            &json!({ "waiting": "tunnel", "link": "inyon://pair?m=a", "qr": "https://x/#pair?m=a", "urls": ["ws://100.64.0.9:4780/link"], "relays": [], "service": false }),
        );
        assert!(out.contains("Scan this") && out.contains("same network or VPN") && !out.contains("No address yet"), "{out}");
    }

    #[test]
    fn reach_advice_names_each_private_network() {
        let none = reach_advice(&[], 4777, "/h/.inyon/hub.env");
        assert!(none[0].contains("answers on this machine only"));
        let nets = vec![networks::Network { kind: networks::Kind::Tailscale, ip: "100.64.0.9".into(), name: Some("laptop.tail1.ts.net".into()) }];
        let lines = reach_advice(&nets, 4777, "/h/.inyon/hub.env");
        assert!(lines[1].contains("BIND=100.64.0.9"));
        assert!(lines[1].contains("http://laptop.tail1.ts.net:4777"));
    }

    #[test]
    fn setup_serves_plain_http_only_on_a_vpn_and_https_on_its_own_certificate_everywhere_else() {
        use std::collections::BTreeMap;
        let dir = std::env::temp_dir().join(format!("inyon-setup-tls-{}", std::process::id()));
        let made = std::cell::Cell::new(0);
        let make = || {
            made.set(made.get() + 1);
            crate::hub_cert::make(&dir, &crate::hub_cert::names("studio", &["192.168.1.20".parse().unwrap()]))
        };
        let run = |bind: Bind, mut env: BTreeMap<String, String>| {
            let lines = hub_settings(&bind, &mut env, &make).unwrap();
            (env, lines)
        };
        let keys = |env: &BTreeMap<String, String>| env.keys().cloned().collect::<Vec<_>>().join(" ");

        // the LAN, as `inyon setup --role hub --network lan --yes` picks it: HTTPS, never plain http
        let (env, lines) = run(Bind::Lan, BTreeMap::from([("INYON_ALLOW_HTTP".into(), "1".into())]));
        assert_eq!(keys(&env), "BIND TLS_CERT TLS_KEY");
        assert_eq!(env["BIND"], "0.0.0.0");
        assert_eq!(env["TLS_CERT"], dir.join("hub.crt").to_string_lossy());
        let fp = crate::hub_cert::fingerprint(&dir.join("hub.crt")).unwrap();
        assert!(lines.len() == 1 && lines[0].contains(&fp), "{lines:?}");
        // set up again: the same certificate, so the machines added with it keep working
        let (again, _) = run(Bind::Lan, env.clone());
        assert_eq!((again, made.get()), (env, 1));
        // an address of your own on the LAN, or a public one, is the same; and so are ZeroTier and
        // WireGuard, which machines don't send their token to over plain http
        for bind in
            [Bind::Own("192.168.1.20".into()), Bind::Own("203.0.113.7".into()), Bind::Own("0.0.0.0".into()), Bind::Vpn("10.147.17.5".into()), Bind::Own("10.147.17.5".into())]
        {
            let (env, _) = run(bind.clone(), BTreeMap::from([("INYON_ALLOW_HTTP".into(), "1".into())]));
            assert_eq!(keys(&env), "BIND TLS_CERT TLS_KEY", "{bind:?}");
        }
        // Tailscale and NetBird encrypt on their own: plain http, as before
        for bind in [Bind::Vpn("100.64.0.9".into()), Bind::Own("100.70.1.2".into()), Bind::Vpn("fd7a:115c:a1e0::9".into())] {
            let (env, lines) = run(bind.clone(), BTreeMap::new());
            assert_eq!((keys(&env), env["INYON_ALLOW_HTTP"].as_str(), lines.len()), ("BIND INYON_ALLOW_HTTP".into(), "1", 0), "{bind:?}");
        }
        // this machine only: neither
        let (env, _) = run(Bind::Localhost, BTreeMap::from([("INYON_ALLOW_HTTP".into(), "1".into())]));
        assert_eq!(keys(&env), "BIND");
        // a certificate of your own is kept, on the LAN and on a VPN
        let own = BTreeMap::from([("TLS_CERT".to_string(), "/etc/hub.pem".to_string()), ("TLS_KEY".to_string(), "/etc/hub.key".to_string())]);
        for bind in [Bind::Lan, Bind::Vpn("100.64.0.9".into())] {
            let (env, _) = run(bind, own.clone());
            assert_eq!((keys(&env), env["TLS_CERT"].as_str()), ("BIND TLS_CERT TLS_KEY".into(), "/etc/hub.pem"));
        }
        assert_eq!(made.get(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hub_on_its_own_certificate_is_trusted_only_with_the_sha256_its_command_named() {
        let dir = std::env::temp_dir().join(format!("inyon-join-cert-{}", std::process::id()));
        let made = crate::hub_cert::make(&dir, &crate::hub_cert::names("studio", &[])).unwrap();
        let pem = fs::read(&made.cert).unwrap();
        let sha = crate::hub_cert::fingerprint(&made.cert).unwrap().replace(':', "").to_lowercase();
        let asked = std::cell::RefCell::new(Vec::new());
        let serve = |body: Vec<u8>| {
            let asked = &asked;
            move |url: &str| -> Result<http::Response, String> {
                asked.borrow_mut().push(url.to_string());
                Ok(http::Response { status: 200, location: None, body: body.clone() })
            }
        };
        let file = dir.join("node-hub.crt");
        trust_hub_cert("https://192.168.1.20:4777", Some(&sha), &serve(pem.clone()), &file).unwrap();
        assert_eq!(*asked.borrow(), ["https://192.168.1.20:4777/node/hub.crt"]);
        // rcgen ends lines with \r\n on Windows, and the copy kept with \n
        let flat = |s: &str| s.replace(['\r', '\n'], "");
        assert_eq!(flat(&fs::read_to_string(&file).unwrap()), flat(&String::from_utf8_lossy(&pem)));
        // someone else's certificate on the way: refused, and the token isn't saved (join stops here)
        let other = crate::hub_cert::make(&dir.join("other"), &crate::hub_cert::names("studio", &[])).unwrap();
        let e = trust_hub_cert("https://192.168.1.20:4777", Some(&sha), &serve(fs::read(&other.cert).unwrap()), &file).unwrap_err();
        assert!(e.contains("wasn't added"), "{e}");
        assert!(trust_hub_cert("http://100.64.0.9:4777", Some(&sha), &serve(pem.clone()), &file).unwrap_err().contains("isn't an https address"));
        // joining a hub without one forgets the old hub's certificate
        trust_hub_cert("https://hub.example.com", None, &serve(Vec::new()), &file).unwrap();
        assert!(!file.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn join_keeps_the_token_the_hub_hands_out_once_and_an_older_hubs_page_changes_nothing() {
        use inyon_engine::http::Response;
        let answer = |status: u16, body: &str| {
            let body = body.as_bytes().to_vec();
            move |url: &str, token: &str| {
                assert_eq!(url, "https://hub.example.com/node/claim");
                assert_eq!(token, "pasted-token");
                Ok(Response { status, body: body.clone() })
            }
        };
        let fresh = "Zm9yLXRoZS1qb2luLXRlc3QtMDEyMzQ1Njc4OWFiY2RlZg";
        let claim = |post: &dyn Fn(&str, &str) -> Result<Response, String>| claim_token("https://hub.example.com", "pasted-token", post);
        assert_eq!(claim(&answer(200, &format!(r#"{{"token":"{fresh}"}}"#))), Claim::Swapped(fresh.into()));
        // a hub from before 0.20.3 serves its web page, a machine an older inyon paired keeps its token
        assert_eq!(claim(&answer(200, "<!doctype html><title>Inyon</title>")), Claim::Kept);
        assert_eq!(claim(&answer(409, "This machine is paired already")), Claim::Kept);
        assert_eq!(claim(&answer(421, "")), Claim::Kept);
        assert_eq!(claim(&|_: &str, _: &str| Err("connection refused".into())), Claim::Kept);
        // a token the hub could put on a line of the env file, or nothing like one: not taken
        assert_eq!(claim(&answer(200, r#"{"token":"short"}"#)), Claim::Kept);
        assert_eq!(claim(&answer(200, &format!(r#"{{"token":"{fresh}\nINYON_HUB=http://evil"}}"#))), Claim::Kept);
        let Claim::Refused(why) = claim(&answer(403, "Unknown token; the command was used already or replaced. Get a new one from the hub.\n")) else { panic!() };
        assert!(why.contains("used already") && why.contains("Add a machine"), "{why}");
    }

    #[test]
    fn flags_make_the_same_plan_as_the_screens() {
        let f = SetupFlags { yes: true, role: Some("hub".into()), network: Some("lan".into()), port: Some(8443), autostart: Some(false), hub: None, no_code: false };
        let p = plan_from_flags(Platform::Linux, Vec::new(), &f).unwrap();
        assert_eq!(p.role, RoleChoice::Hub);
        assert_eq!(p.bind, Bind::Lan);
        assert_eq!(p.port_number(), 8443);
        assert!(!p.autostart);
        let f = SetupFlags { yes: true, role: None, network: Some("tunnel,vpn,wss://me.example/link".into()), port: None, autostart: None, hub: None, no_code: false };
        let p = plan_from_flags(Platform::Linux, Vec::new(), &f).unwrap();
        assert!(p.tunnel && p.vpn && !p.lan);
        assert_eq!(p.own_url, "wss://me.example/link");
        let f = SetupFlags { yes: true, role: None, network: None, port: None, autostart: None, hub: Some("https://hub".into()), no_code: false };
        assert_eq!(plan_from_flags(Platform::Linux, Vec::new(), &f).unwrap().role, RoleChoice::Join);
        let f = SetupFlags { yes: true, role: Some("join".into()), network: None, port: None, autostart: None, hub: None, no_code: false };
        assert!(plan_from_flags(Platform::Linux, Vec::new(), &f).is_err());
        let f = SetupFlags { yes: true, role: Some("hub".into()), network: None, port: None, autostart: None, hub: None, no_code: false };
        assert!(plan_from_flags(Platform::Windows, Vec::new(), &f).unwrap_err().contains("WSL"));
    }
}
