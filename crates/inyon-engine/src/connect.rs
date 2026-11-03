//! The hub connection: one outbound WebSocket to the hub, running what it asks for as this user.
//! (inyon.ts's connectMode, hubUrl, join and leave)

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::output::{emit, error, fail, log, say};
use crate::paths::{self, env_var, node_dir, running_file, saved_env, Role};
use crate::{ops, VERSION, WIN};

/// Checks a hub address for `join`, run by hand; refuses to send a token over plain http to
/// anything but this machine or a VPN address. True when its name has to be looked up and checked
/// before each connection.
pub fn hub_url(hub: &str) -> (Url, bool) {
    checked_hub(hub, env_var("INYON_ALLOW_INSECURE").is_some()).unwrap_or_else(|e| fail(&e, 2))
}

/// What `join` writes to the env file after the hub and token: the version that joined, so
/// `connect` knows the address passed the check this version makes (a file without it was saved
/// before 0.20); and INYON_ALLOW_INSECURE when the address needed it, so the connection keeps the
/// choice made when joining.
pub fn join_marks(hub: &str) -> Vec<String> {
    let mut lines = vec![format!("INYON_JOINED={VERSION}")];
    if env_var("INYON_ALLOW_INSECURE").is_some() && strict_refusal(hub).is_some() {
        lines.push("INYON_ALLOW_INSECURE=1".into());
    }
    lines
}

/// Why plain http to `hub` would be refused, looking its name up if it has one; None when it may go.
fn strict_refusal(hub: &str) -> Option<String> {
    match checked_hub(hub, false) {
        Err(why) => Some(why),
        Ok((url, true)) => crate::hub::check_name_now(url.host_str().unwrap_or(""), url.port_or_known_default().unwrap_or(80)).err().flatten(),
        Ok(_) => None,
    }
}

/// What the log says when NODE_TLS_REJECT_UNAUTHORIZED=0 is set (crate::http::insecure_tls_for).
pub fn insecure_warning(host: &str) -> String {
    format!("NODE_TLS_REJECT_UNAUTHORIZED=0 is set, so the certificate of {host} isn't checked and anyone on the network could answer in its place. Only the hub's: relays, push services and downloads are still checked. Unset it once the hub has a real certificate, or run setup's LAN choice on the hub and add this machine again.")
}

/// What the log, `inyon doctor` and `inyon status` say about a machine joined to a hub on plain
/// http before 0.20 refused it: it still connects, and how to move to https.
pub fn plain_warning(host: &str) -> String {
    format!(
        "Sending the token over plain http to {host}, where anyone on that network could read it or answer in the hub's place. This machine was added before Inyon 0.20, so it still connects. To move to https, run `inyon setup` on the hub and choose LAN (it makes a certificate), then run the hub's new Add a machine command here; or add this machine through the hub's Tailscale or NetBird address."
    )
}

/// What the saved hub setting means for the connection: Ok(None) when all is well, Ok(Some(warning))
/// for a hub on plain http joined before 0.20 (it connects anyway), Err(why) when it can't connect.
/// For `inyon doctor`, which looks names up as the connection would.
pub fn saved_hub_check() -> Option<Result<Option<String>, String>> {
    let saved = saved_env();
    let hub = env_var("INYON_HUB").or_else(|| saved.get("INYON_HUB").cloned())?.trim_end_matches('/').to_string();
    if hub.is_empty() {
        return None;
    }
    let from_saved = env_var("INYON_HUB").is_none();
    let insecure_ok = env_var("INYON_ALLOW_INSECURE").is_some() || (from_saved && saved.get("INYON_ALLOW_INSECURE").is_some_and(|v| !v.is_empty()));
    let legacy = from_saved && !saved.contains_key("INYON_JOINED") && Url::parse(&hub).is_ok_and(|u| crate::hub::plain_before_0_20(&u));
    if insecure_ok {
        return Some(Url::parse(&hub).map(|_| None).map_err(|_| format!("{hub} isn't a web address.")));
    }
    Some(match (strict_refusal(&hub), Url::parse(&hub)) {
        (None, _) => Ok(None),
        (Some(_), Ok(url)) if legacy => Ok(Some(plain_warning(&host_of(&url)))),
        (Some(why), _) => Err(why),
    })
}

/// Whether the hub is reached with TLS: https and wss are, http and ws aren't.
fn secure(url: &Url) -> bool {
    matches!(url.scheme(), "https" | "wss")
}

fn checked_hub(hub: &str, insecure_ok: bool) -> Result<(Url, bool), String> {
    use crate::hub::Plain;
    let url = Url::parse(hub).map_err(|_| format!("{hub} isn't a web address."))?;
    if secure(&url) || insecure_ok {
        return Ok((url, false));
    }
    match crate::hub::plain_host(&url) {
        Plain::Ok => Ok((url, false)),
        Plain::LookUp => Ok((url, true)),
        Plain::Refused => Err(crate::hub::plain_refusal(url.host_str().unwrap_or(""))),
    }
}

/// The hub's WebSocket and its plain address for `/node`, from any of http, https, ws or wss.
fn node_urls(url: &Url) -> (String, String) {
    let host = host_of(url);
    let (ws, http) = if secure(url) { ("wss", "https") } else { ("ws", "http") };
    (format!("{ws}://{host}/node"), format!("{http}://{host}/node"))
}

/// `host[:port]` as `URL.host` gives it (no port when it's the scheme's own).
fn host_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }
}

/// The connection to a hub. Never returns: it ends the process when the hub removes this machine
/// or rejects its token. A setting it can't connect with (no hub saved, an address plain http
/// mustn't go to) is said in the log and in node.json, which `inyon status` and `inyon doctor`
/// show, and tried again now and then: run as a service, exiting would only have it started again
/// every few seconds, and nobody would see why. Run in a terminal, it ends with the reason.
pub async fn connect_mode(hub_arg: Option<String>, token_arg: Option<String>) {
    use std::io::IsTerminal;
    let by_hand = std::io::stdin().is_terminal();
    let mut attempt: u32 = 0;
    loop {
        let why = match settings(hub_arg.clone(), token_arg.clone()) {
            Ok(s) => run(s).await,
            Err(why) => why,
        };
        if by_hand {
            fail(&why, 2);
        }
        record(Some(&why), None);
        // a setting changes when someone runs join again, which restarts this anyway
        let delay = crate::backoff(attempt, Duration::from_secs(30), Duration::from_secs(300));
        attempt = attempt.saturating_add(1);
        error(&format!("{why} Trying again in {}s.", (delay.as_millis() + 500) / 1000));
        tokio::time::sleep(delay).await;
    }
}

/// The hub, token and what plain http to it is allowed, as `connect` reads them each time.
struct Settings {
    url: Url,
    token: String,
    /// look the name up and check it before each connection
    look_up: bool,
    /// joined before 0.20: plain http that would be refused now still goes, with a warning
    legacy: bool,
}

fn settings(hub_arg: Option<String>, token_arg: Option<String>) -> Result<Settings, String> {
    // the installers keep the hub address and token in the node's folder (readable only by you), so
    // the token never sits in the process list where other users could read it
    let saved = saved_env();
    let from_saved = hub_arg.is_none() && env_var("INYON_HUB").is_none();
    let hub = hub_arg.or_else(|| env_var("INYON_HUB")).or_else(|| saved.get("INYON_HUB").cloned()).unwrap_or_default().trim_end_matches('/').to_string();
    let token = token_arg.clone().or_else(|| env_var("INYON_TOKEN")).or_else(|| saved.get("INYON_TOKEN").cloned()).unwrap_or_default();
    if env_var("INYON_SH").is_none() {
        if let Some(sh) = saved.get("INYON_SH").filter(|s| !s.is_empty()) {
            paths::set_env("INYON_SH", sh);
        }
    }
    if hub.is_empty() || token.is_empty() {
        return Err(format!("This machine has no hub saved. Run the hub's Add a machine command here.\n{}", crate::usage()));
    }
    if token_arg.is_some() {
        error("Note: a token given as an argument shows in the process list. Prefer INYON_TOKEN=<token> inyon join <hub-url>.");
    }
    let insecure_ok = env_var("INYON_ALLOW_INSECURE").is_some() || (from_saved && saved.get("INYON_ALLOW_INSECURE").is_some_and(|v| !v.is_empty()));
    // saved before 0.20, to an address that version sent the token to: it keeps connecting
    let legacy = from_saved && !saved.contains_key("INYON_JOINED") && Url::parse(&hub).is_ok_and(|u| crate::hub::plain_before_0_20(&u));
    let (url, look_up) = match (checked_hub(&hub, insecure_ok), Url::parse(&hub)) {
        (Ok(c), _) => c,
        (Err(_), Ok(url)) if legacy => (url, false),
        (Err(why), _) => return Err(why),
    };
    Ok(Settings { url, token, look_up, legacy })
}

/// Writes node.json: the pid and version `inyon update` and the hub's update script wait for, and
/// what's wrong (`problem`) or worth knowing (`warning`), for `inyon status` and `inyon doctor`.
fn record(problem: Option<&str>, warning: Option<&str>) {
    if std::fs::create_dir_all(node_dir()).is_ok() {
        let mut r = json!({ "pid": std::process::id(), "version": VERSION });
        if let Some(p) = problem {
            r["problem"] = json!(p);
        }
        if let Some(w) = warning {
            r["warning"] = json!(w);
        }
        let _ = std::fs::write(running_file(Role::Node), r.to_string());
    }
}

/// Connects and reconnects to the hub; returns only with a reason it can't go on (the name now
/// points where plain http mustn't go).
async fn run(s: Settings) -> String {
    let host = host_of(&s.url);
    let (ws_url, http_url) = node_urls(&s.url);
    let mut warning = (s.legacy && strict_refusal(s.url.as_str()).is_some()).then(|| plain_warning(&host));
    if let Some(w) = &warning {
        error(&format!("Warning: {w}"));
    }
    record(None, warning.as_deref());
    if crate::http::insecure_tls() {
        error(&format!("Warning: {}", insecure_warning(&host)));
    }
    let bearer = format!("Bearer {}", s.token);
    let name = s.url.host_str().unwrap_or("").to_string();
    let port = s.url.port_or_known_default().unwrap_or(80);
    let mut attempt: u32 = 0;
    loop {
        // a name on plain http may point elsewhere by now (an answer from someone on the network):
        // it's looked up once, and the connection goes to the addresses that passed, nowhere else
        let (code, up, checked) = if s.look_up {
            match crate::hub::check_name(&name, port).await {
                Ok(addrs) => {
                    let (code, up) = session(&ws_url, &host, &bearer, Some(addrs.clone()), warning.as_deref()).await;
                    (code, up, Some(Some(addrs)))
                }
                Err(Some(_)) if s.legacy => {
                    if warning.is_none() {
                        let w = plain_warning(&host);
                        error(&format!("Warning: {w}"));
                        record(None, Some(&w));
                        warning = Some(w);
                    }
                    let (code, up) = session(&ws_url, &host, &bearer, None, warning.as_deref()).await;
                    (code, up, Some(None))
                }
                Err(Some(why)) => return why,
                Err(None) => {
                    log(&format!("couldn't look up {name}"));
                    (1006, None, None)
                }
            }
        } else {
            let (code, up) = session(&ws_url, &host, &bearer, None, warning.as_deref()).await;
            (code, up, Some(None))
        };
        let opened = up.is_some();
        let removed = match (&checked, opened) {
            (Some(addrs), false) => removed_there(&http_url, &bearer, addrs.clone()).await,
            _ => false,
        };
        if code == 4001 || removed {
            leave(&format!("This machine was removed from {host}.")).await;
        }
        if code == 4003 {
            error("The hub rejected the token. Pair this machine again from the hub.");
            std::process::exit(1);
        }
        // a hub that takes the connection and drops it at once isn't tried every second
        attempt = crate::next_attempt(attempt, up);
        let delay = crate::backoff(attempt.saturating_sub(1), Duration::from_secs(1), Duration::from_secs(30));
        log(&format!("disconnected ({code}); retrying in {}s", (delay.as_millis() + 500) / 1000));
        tokio::time::sleep(delay).await;
    }
}

/// One connection, from opening to closing: the close code, and how long it was open (None: it never opened).
async fn session(ws_url: &str, host: &str, bearer: &str, addrs: Option<Vec<std::net::SocketAddr>>, warning: Option<&str>) -> (u16, Option<Duration>) {
    let socket = match crate::ws::connect_to(ws_url, &[("authorization", bearer)], addrs).await {
        Ok(s) => s,
        Err((code, _)) => return (code, None),
    };
    let opened = tokio::time::Instant::now();
    log(&format!("connected to {host}"));
    let (mut write, mut read) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let send: ops::Sender = {
        let tx = tx.clone();
        Arc::new(move |m: Value| {
            let _ = tx.send(Message::Text(m.to_string().into()));
        })
    };
    let ops = ops::Ops::new(send.clone());
    let mut hello = json!({ "type": "hello", "version": VERSION, "os": format!("{} {}", crate::platform(), crate::arch()), "hostname": crate::hostname(), "platform": crate::platform(), "mux": if WIN { "psmux" } else { "tmux" } });
    // shown on the hub's machine page
    if let Some(w) = warning {
        hello["warning"] = json!(w);
    }
    send(hello);
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if write.send(m).await.is_err() {
                break;
            }
        }
        let _ = write.close().await;
    });
    let mut code = 1006;
    while let Some(next) = read.next().await {
        match next {
            Ok(Message::Text(t)) => {
                if let Ok(m) = serde_json::from_str::<Value>(t.as_str()) {
                    ops.message(m);
                }
            }
            Ok(Message::Binary(b)) => {
                if let Ok(m) = serde_json::from_slice::<Value>(&b) {
                    ops.message(m);
                }
            }
            Ok(Message::Close(frame)) => {
                code = frame.map(|f| u16::from(f.code)).unwrap_or(1005);
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    ops.close();
    drop(tx);
    drop(ops);
    drop(send);
    writer.abort();
    (code, Some(opened.elapsed()))
}

/// A hub answers 410 to a machine removed while it was off (a token it never knew gets 403). With
/// `addrs`, only to those (the name's checked addresses).
async fn removed_there(target: &str, bearer: &str, addrs: Option<Vec<std::net::SocketAddr>>) -> bool {
    let headers = vec![("authorization".to_string(), bearer.to_string())];
    let r = match addrs {
        Some(addrs) => crate::http::fetch_to(target.to_string(), headers, Duration::from_secs(30), addrs).await,
        None => crate::http::fetch("GET", target.to_string(), headers, None, Duration::from_secs(30)).await,
    };
    r.is_ok_and(|r| r.status == 410)
}

/// The hub removed this machine. `inyon disconnect` takes the background service down and forgets
/// the hub, so nothing starts this again and calls the hub for ever. It runs in a session of its
/// own: stopping the service may stop everything started from this process.
async fn leave(why: &str) -> ! {
    log(&format!("{why} Removing the background service."));
    let Some(cli) = paths::inyon_command() else {
        for name in ["env", ".env"] {
            let _ = std::fs::remove_file(node_dir().join(name));
        }
        std::process::exit(0)
    };
    let mut c = tokio::process::Command::new(cli);
    c.arg("disconnect");
    ops::hide_window(&mut c);
    #[cfg(unix)]
    // SAFETY: setsid is async-signal-safe and touches nothing but the child
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Ok(mut child) = c.spawn() {
        let _ = child.wait().await;
    }
    std::process::exit(0)
}

/// `join <hub-url>`: saves the hub and token (readable only by you) in the node's folder, where
/// `connect` reads them. The inyon command's own `join` writes the same file and then sets up the
/// background service; this one is for starting the connection by hand.
pub fn join(hub_arg: Option<&str>) {
    let hub = hub_arg.unwrap_or("").trim_end_matches('/').to_string();
    let token = env_var("INYON_TOKEN").unwrap_or_default();
    if hub.is_empty() || token.is_empty() {
        fail("usage: INYON_TOKEN=<token> inyon-engine join <hub-url>", 2);
    }
    let (url, _) = hub_url(&hub);
    if env_var("INYON_ALLOW_INSECURE").is_none() {
        if let Some(why) = strict_refusal(&hub) {
            fail(&why, 2);
        }
    }
    if let Err(e) = ops::ensure_shell() {
        say(&format!("! {e}"));
    }
    let dir = node_dir();
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let mut lines = vec![format!("INYON_HUB={hub}"), format!("INYON_TOKEN={token}")];
    lines.extend(join_marks(&hub));
    if WIN {
        if let Some(sh) = env_var("INYON_SH") {
            lines.push(format!("INYON_SH={sh}"));
        }
    }
    let file: PathBuf = dir.join(if WIN { ".env" } else { "env" });
    if let Err(e) = write_private(&file, format!("{}\n", lines.join("\n")).as_bytes()) {
        fail(&e.to_string(), 1);
    }
    // the other file name may hold a token from an earlier pairing
    let _ = std::fs::remove_file(dir.join(if WIN { "env" } else { ".env" }));
    if WIN {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let mut c = std::process::Command::new("icacls");
        c.arg(&file).args(["/inheritance:r", "/grant:r", &format!("{user}:(R,W)"), "SYSTEM:(F)"]).stdout(Stdio::null()).stderr(Stdio::null());
        ops::hide_window_std(&mut c);
        let _ = c.status();
    }
    emit(json!({ "hub": hub }), Some(&format!("Saved the hub {}. Run `inyon connect` to connect, or `inyon service install node` to connect in the background.", host_of(&url))));
}

/// A file only this user can read (mode 0600 from the start on Unix).
pub fn write_private(file: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    use std::io::Write;
    o.open(file)?.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ws_address_is_plain_and_a_wss_one_is_tls_like_http_and_https() {
        let urls = |h: &str| node_urls(&checked_hub(h, false).unwrap().0);
        assert_eq!(urls("https://hub.example.com"), ("wss://hub.example.com/node".into(), "https://hub.example.com/node".into()));
        assert_eq!(urls("wss://hub.example.com"), ("wss://hub.example.com/node".into(), "https://hub.example.com/node".into()));
        assert_eq!(urls("ws://100.64.1.2:4777"), ("ws://100.64.1.2:4777/node".into(), "http://100.64.1.2:4777/node".into()));
        // plain ws to a public address carries the token in the clear, as http would
        for plain in ["http://8.8.8.8", "ws://8.8.8.8"] {
            assert!(checked_hub(plain, false).unwrap_err().starts_with("Refusing to send the token over plain http"), "{plain}");
            assert!(checked_hub(plain, true).is_ok());
        }
    }

    #[test]
    fn plain_http_goes_only_to_this_machine_or_a_vpn_and_a_name_is_checked_each_time() {
        assert_eq!(checked_hub("http://100.70.1.2:4777", false).map(|(_, l)| l), Ok(false));
        assert_eq!(checked_hub("http://127.0.0.1:4777", false).map(|(_, l)| l), Ok(false));
        // a LAN address, as hubs set up with "LAN" before 0.20 were joined
        let lan = checked_hub("http://192.168.1.5:4777", false).unwrap_err();
        assert!(lan.contains("192.168.1.5") && lan.contains("INYON_ALLOW_INSECURE=1") && lan.contains("https"), "{lan}");
        assert_eq!(checked_hub("http://192.168.1.5:4777", true).map(|(_, l)| l), Ok(false));
        assert_eq!(checked_hub("http://hub.local:4777", false).map(|(_, l)| l), Ok(true));
        assert_eq!(checked_hub("https://hub.local:4777", false).map(|(_, l)| l), Ok(false));
    }

    #[test]
    fn a_machine_joined_over_plain_http_before_0_20_is_warned_about_and_one_joined_since_is_refused() {
        let _home = crate::paths::testing::home("saved-hub-check");
        // SAFETY: tests that touch the environment hold testing::home's lock
        unsafe {
            for k in ["INYON_HUB", "TETHER_HUB", "INYON_ALLOW_INSECURE", "TETHER_ALLOW_INSECURE"] {
                std::env::remove_var(k);
            }
        }
        let save = |lines: &[&str]| {
            std::fs::create_dir_all(crate::paths::node_dir()).unwrap();
            std::fs::write(crate::paths::node_dir().join(if crate::WIN { ".env" } else { "env" }), format!("{}\n", lines.join("\n"))).unwrap();
        };
        assert_eq!(saved_hub_check(), None);
        // saved before 0.20: it connects, and says how to move to https
        save(&["INYON_HUB=http://192.168.1.5:4777", "INYON_TOKEN=t"]);
        let warning = saved_hub_check().unwrap().unwrap().unwrap();
        assert!(warning.starts_with("Sending the token over plain http to 192.168.1.5:4777"), "{warning}");
        assert!(warning.contains("inyon setup"), "{warning}");
        // joined by this version, the same address can only have been changed since: refused
        save(&["INYON_HUB=http://192.168.1.5:4777", "INYON_TOKEN=t", "INYON_JOINED=0.20.0"]);
        assert!(saved_hub_check().unwrap().unwrap_err().starts_with("Refusing to send the token over plain http to 192.168.1.5"));
        // unless the join was told to send it anyway
        save(&["INYON_HUB=http://192.168.1.5:4777", "INYON_TOKEN=t", "INYON_JOINED=0.20.0", "INYON_ALLOW_INSECURE=1"]);
        assert_eq!(saved_hub_check(), Some(Ok(None)));
        // a VPN address and https are fine, whenever they were joined
        save(&["INYON_HUB=http://100.70.1.2:4777", "INYON_TOKEN=t", "INYON_JOINED=0.20.0"]);
        assert_eq!(saved_hub_check(), Some(Ok(None)));
        save(&["INYON_HUB=https://192.168.1.5:4777", "INYON_TOKEN=t"]);
        assert_eq!(saved_hub_check(), Some(Ok(None)));
        // a public address, which 0.19 refused too, stays refused however old the file
        for hub in ["http://203.0.113.7:4777", "http://[2001:db8::1]:4777"] {
            save(&[&format!("INYON_HUB={hub}"), "INYON_TOKEN=t"]);
            let why = saved_hub_check().unwrap().unwrap_err();
            assert!(why.starts_with("Refusing to send the token over plain http") && why.contains("https"), "{why}");
        }
        // join notes INYON_ALLOW_INSECURE only for an address that needed it
        let joined = format!("INYON_JOINED={VERSION}");
        assert_eq!(join_marks("http://192.168.1.5:4777"), vec![joined.clone()]);
        // SAFETY: as above
        unsafe { std::env::set_var("INYON_ALLOW_INSECURE", "1") };
        let marks = (join_marks("http://192.168.1.5:4777"), join_marks("https://192.168.1.5:4777"), join_marks("http://100.70.1.2:4777"));
        // SAFETY: as above
        unsafe { std::env::remove_var("INYON_ALLOW_INSECURE") };
        assert_eq!(marks.0, vec![joined.clone(), "INYON_ALLOW_INSECURE=1".to_string()]);
        assert_eq!(marks.1, vec![joined.clone()]);
        assert_eq!(marks.2, vec![joined]);
    }
}
