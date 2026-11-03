//! The engine's commands, as inyon-engine answered them (inyon.ts). The `inyon` command runs them
//! in-process; a program started under the engine's name (`inyon-engine`, which old commands,
//! services and the Windows sign-in task run) answers them the same way. Only `hub` isn't here:
//! the caller hands it to the hub (crates/inyon-hub).
//!
//!   pair [options]      make a pairing code (starts the link here if none runs)
//!   link [options]      run the phone link
//!   phones              list paired phones
//!   forget <name|key>   unpair a phone
//!   push …              the browsers this machine sends notifications to
//!   join <hub-url>      save a hub's address and the token in INYON_TOKEN
//!   connect             run the connection to that hub
//!   version             print the version
//!   hidden <program> …  run a program with no window and wait for it

use std::io::IsTerminal;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use inyon_link::PairInfo;
use serde_json::{json, Value};

use crate::link::{self, LinkConfig};
use crate::output::{emit, fail, log, pair_result, PairOutput};
use crate::paths::{self, env_var, node_dir, Role};
use crate::{ops, push, tidy, usage, INSTALL_URL, VERSION, WEB_URL, WIN};

static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// Run in the background (a service, the sign-in task) there's no console to print to, so log
/// lines go to a file. Commands people run (pair, phones…) always print, even from a script, so
/// the pairing code shows, and so does what the inyon command runs in a terminal (it sets
/// INYON_CONSOLE).
pub fn background() -> bool {
    BACKGROUND.load(Ordering::Relaxed)
}

const INTERACTIVE: [&str; 11] = ["pair", "join", "phones", "forget", "push", "update", "version", "--version", "help", "--help", "-h"];

/// Notes whether this runs in the background on Windows, and sends the log there if so.
pub fn setup_background(first: Option<&str>) {
    let bg = WIN && !std::io::stdout().is_terminal() && env_var("INYON_CONSOLE").is_none() && !first.is_some_and(|f| INTERACTIVE.contains(&f));
    BACKGROUND.store(bg, Ordering::Relaxed);
    if bg {
        crate::output::log_to(paths::log_file(if first == Some("link") { Role::Link } else { Role::Node }));
    }
}

/// Ends the process the way the Bun engine did on a signal: what it started stops with it.
pub fn shutdown(code: i32) -> ! {
    link::stop_children();
    ops::kill_everything();
    std::process::exit(code)
}

/// Stops what this process started when it's told to stop (Ctrl-C, a service stopping).
fn on_signals() {
    tokio::spawn(async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let (Ok(mut int), Ok(mut term), Ok(mut hup)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate()), signal(SignalKind::hangup())) else { return };
            tokio::select! {
                _ = int.recv() => {}
                _ = term.recv() => {}
                _ = hup.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        shutdown(0);
    });
}

/// The Inyon icon in the Windows notification area while this runs in the background, so you can
/// see it runs and reach it without a terminal. The inyon command draws it and removes it once this
/// process has gone.
fn show_tray(role: &str) {
    let Some(cli) = paths::inyon_command() else { return };
    // started by the Inyon app (from 0.20.3), whose own icon shows what runs
    if !background() || env_var("INYON_APP").is_some() {
        return;
    }
    let mut c = std::process::Command::new(cli);
    c.args(["tray", role, "--pid", &std::process::id().to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    ops::hide_window_std(&mut c);
    if let Err(e) = c.spawn() {
        log(&format!("! no tray icon: {e}"));
    }
}

/// Reads link options; what you pass is saved, so the service keeps them on its next start.
pub fn link_options(args: &[String]) -> LinkConfig {
    let mut c = link::load_config();
    let (mut urls_given, mut relays_given) = (false, false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = args.get(i + 1).filter(|n| !n.is_empty());
        match a {
            "--lan" | "--no-lan" => c.lan = a == "--lan",
            "--vpn" | "--no-vpn" => c.vpn = a == "--vpn",
            "--url" if next.is_some() => {
                i += 1;
                let u = &args[i];
                if !link_url(u) {
                    fail(&format!("--url needs a WebSocket address ending in /link, like wss://laptop.tail1234.ts.net:8443/link, not {u}"), 2);
                }
                if !urls_given {
                    c.urls.clear();
                }
                urls_given = true;
                c.urls.push(u.clone());
            }
            "--no-url" => c.urls.clear(),
            "--tunnel" | "--no-tunnel" => c.tunnel = a == "--tunnel",
            "--port" if next.is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) && next.and_then(|n| n.parse::<u16>().ok()).is_some() => {
                i += 1;
                c.port = args[i].parse().unwrap_or(4780);
            }
            "--relay" if next.is_some() => {
                i += 1;
                let r = args[i].trim_end_matches('/').to_string();
                if !(r.starts_with("http://") || r.starts_with("https://")) {
                    fail(&format!("--relay needs an http(s) address, not {r}"), 2);
                }
                if !relays_given {
                    c.relays.clear();
                }
                relays_given = true;
                c.relays.push(r);
            }
            "--no-relay" => c.relays.clear(),
            _ => fail(&format!("unknown option {a}"), 2),
        }
        i += 1;
    }
    link::save_config(&c).unwrap_or_else(|e| fail(&e, 1));
    c
}

/// An address you publish yourself: `^wss?://[^/]+/link$`, as the Bun engine checked it.
fn link_url(u: &str) -> bool {
    let rest = u.strip_prefix("wss://").or_else(|| u.strip_prefix("ws://"));
    rest.and_then(|r| r.strip_suffix("/link")).is_some_and(|host| !host.is_empty() && !host.contains('/'))
}

/// How long `pair` waits for an address and how often it looks: a moment before it says the
/// tunnel is starting, `first` before it shows a code with what it has (a LAN or VPN address
/// works already), and `until` (the pairing's lifetime) before it stops watching for the tunnel.
pub struct Patience {
    pub quick: Duration,
    pub first: Duration,
    pub tick: Duration,
    pub until: Duration,
}

impl Patience {
    pub const REAL: Patience =
        Patience { quick: Duration::from_millis(1500), first: Duration::from_secs(30), tick: Duration::from_millis(500), until: Duration::from_secs(link::PAIR_MINUTES * 60) };
}

/// Makes a pairing code, starting the link here first if it isn't running yet, and hands each
/// answer to `show` as `pair --json` prints it. The first may say the tunnel is still starting
/// (`"waiting": "tunnel"`); a later one carries the code again with the tunnel's address. Prints
/// nothing itself. Returns whether the link runs elsewhere, so this process has nothing left to do.
pub async fn make_pairing(args: &[String], show: &mut dyn FnMut(&Value)) -> bool {
    // `--once`: nobody can watch for a later answer (an installer with no terminal), so the code
    // after `first` is the last one, as before 0.20
    let once = args.iter().any(|a| a == "--once");
    let args: Vec<String> = args.iter().filter(|a| *a != "--once").cloned().collect();
    let config = link_options(&args);
    let service = link::link_running(Some(config.port)).await;
    if !service {
        // a program reads this one's output (the inyon command, its window): what the link it
        // started has to say goes to link.log, as the service's does, and only the answers come out
        if crate::output::json() {
            crate::output::log_to(paths::log_file(Role::Link));
        }
        if let Err(e) = link::start_link(config.clone()) {
            fail(&e, 1);
        }
        tidy::start_tidying(paths::home());
    }
    let pairing = link::new_pairing().unwrap_or_else(|e| fail(&e, 1));
    let id = link::identity().id;
    let publish = |urls: Vec<String>, relays: Vec<String>| {
        let (short, id) = (pairing.short.clone(), id.clone());
        // the typed-in code needs the relays to know this ID; without them there's still the QR code
        async move { tokio::time::timeout(Duration::from_secs(8), link::publish_short_code(&short, &id, &crate::hostname(), &urls, &relays)).await.is_ok_and(|r| r.is_ok()) }
    };
    pairing_answers(&config, service, &pairing, &id, &Patience::REAL, once, publish, show).await;
    service
}

/// The pairing as `pair --json` answers it, for these addresses.
fn full_answer(pairing: &link::Pairing, id: &str, urls: Vec<String>, relays: Vec<String>, typed: bool, service: bool) -> Value {
    let info = PairInfo { id: id.to_string(), secret: pairing.secret.clone(), name: crate::hostname(), urls: urls.clone(), relays: relays.clone() };
    let link_text = inyon_link::pair_link(&info, None);
    // the QR code opens the web app when scanned with a phone's camera; the Inyon app reads it too.
    // INYON_WEB_URL points it at a copy of the web app you host yourself
    let web = env_var("INYON_WEB_URL").unwrap_or_else(|| WEB_URL.into());
    let qr = inyon_link::pair_link(&info, Some(&web));
    let out = PairOutput { link: link_text, qr, short: Some((pairing.short.id.clone(), pairing.short.code.clone())), urls, relays, service };
    pair_result(&out, typed)
}

/// Waits for an address the phone can use and shows the code once it's there. The tunnel takes a
/// few seconds to come up, longer when cloudflared has to be downloaded first; a private network's
/// ws:// address comes first, but browsers only open wss://, so it waits for the tunnel too.
/// Until then it says the tunnel is starting; after `first` it shows the code with what it has,
/// and again with the tunnel's address once that arrives. With `once` the code after `first` is
/// the last answer: the phone looks the machine up by its ID once the tunnel is up.
#[allow(clippy::too_many_arguments)]
async fn pairing_answers<F, Fut>(config: &LinkConfig, service: bool, pairing: &link::Pairing, id: &str, patience: &Patience, once: bool, publish: F, show: &mut dyn FnMut(&Value))
where
    F: Fn(Vec<String>, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let wanted = config.tunnel || config.lan || config.vpn || !config.urls.is_empty() || !config.relays.is_empty();
    let enough = |s: &link::State| !s.relays.is_empty() || if config.tunnel { s.urls.iter().any(|u| u.starts_with("wss://")) } else { !s.urls.is_empty() };
    // a link started here counts once it has written its own state
    let current = || link::current_state().filter(|s| service || s.pid == std::process::id());
    let ready = || current().filter(|s| enough(s));
    let started = tokio::time::Instant::now();
    let addresses = |s: Option<link::State>| s.map(|s| (s.urls, s.relays)).unwrap_or_default();
    while wanted && ready().is_none() && started.elapsed() < patience.quick {
        tokio::time::sleep(patience.tick).await;
    }
    if !wanted {
        let (urls, relays) = addresses(current());
        let typed = publish(urls.clone(), relays.clone()).await;
        show(&full_answer(pairing, id, urls, relays, typed, service));
        return;
    }
    let mut up = ready();
    if up.is_none() {
        show(&json!({ "waiting": "tunnel", "service": service }));
        while ready().is_none() && started.elapsed() < patience.first {
            tokio::time::sleep(patience.tick).await;
        }
        up = ready();
    }
    if up.is_none() {
        // still no tunnel: the code with what there is, and again once the tunnel is up
        let (urls, relays) = addresses(current());
        let typed = !urls.is_empty() && publish(urls.clone(), relays.clone()).await;
        let mut v = full_answer(pairing, id, urls, relays, typed, service);
        if once {
            show(&v);
            return;
        }
        v["waiting"] = json!("tunnel");
        show(&v);
        while up.is_none() {
            // paired already (over the LAN, say), or a newer code took its place
            if link::pairing_secret().as_deref() != Some(pairing.secret.as_str()) {
                return;
            }
            if started.elapsed() >= patience.until {
                show(&json!({ "ok": false, "error": crate::output::PAIR_EXPIRED, "expired": true }));
                return;
            }
            tokio::time::sleep(patience.tick).await;
            up = ready();
        }
    }
    let Some(s) = up else { return };
    let (urls, relays) = (s.urls, s.relays);
    let typed = publish(urls.clone(), relays.clone()).await;
    show(&full_answer(pairing, id, urls, relays, typed, service));
}

/// `pair`: shows each answer as it comes, drawn for a person, or as JSON lines with --json. True
/// when the link runs elsewhere.
async fn pair(args: &[String]) -> bool {
    let json = crate::output::json();
    let hide_link = crate::output::link_hidden(args.iter().any(|a| a == "--show-link"));
    let args: Vec<String> = args.iter().filter(|a| *a != "--show-link").cloned().collect();
    make_pairing(&args, &mut |v| {
        if json {
            emit(v.clone(), None);
        } else {
            log(&crate::output::render_pair_with(v, hide_link));
        }
    })
    .await
}

fn phones() {
    let all = link::list_phones().0;
    let list: Vec<Value> = all
        .iter()
        .map(|(key, p)| {
            let mut v = json!({ "key": key, "name": p.name, "pairedAt": p.paired_at });
            if let Some(seen) = &p.last_seen {
                v["lastSeen"] = json!(seen);
            }
            v
        })
        .collect();
    if all.is_empty() {
        return emit(json!({ "phones": list }), Some("No phones paired yet. Run `inyon pair`."));
    }
    let when = |n: &serde_json::Number| crate::iso(n.as_f64().unwrap_or(0.0) as u64);
    let lines: Vec<String> = all
        .iter()
        .map(|(key, p)| {
            let seen = p.last_seen.as_ref().map(|s| when(s)[..16].replace('T', " ")).unwrap_or_else(|| "never".into());
            format!("{} {}  paired {}, last seen {seen}", push::pad_end(&p.name, 24), &key[..key.len().min(12)], &when(&p.paired_at)[..10])
        })
        .collect();
    emit(json!({ "phones": list }), Some(&lines.join("\n")));
}

/// `forget <name|key>`: unpairs one phone. The answer is its key and name.
pub fn forget(which: Option<&str>) -> Result<(String, String), String> {
    let Some(which) = which.filter(|w| !w.is_empty()) else { return Err("usage: inyon forget <name|key>".into()) };
    let lower = which.to_lowercase();
    let matches: Vec<(String, link::Phone)> = link::list_phones().0.into_iter().filter(|(key, p)| key.starts_with(&lower) || p.name == which).collect();
    if matches.len() != 1 {
        return Err(if matches.is_empty() { format!("No paired phone matches \"{which}\"") } else { format!("\"{which}\" matches {} phones; use more of the key", matches.len()) });
    }
    let Some((key, phone)) = matches.into_iter().next() else { return Err(format!("No paired phone matches \"{which}\"")) };
    link::forget_phone(&key)?;
    Ok((key, phone.name))
}

/// `update`: the inyon command owns updates since 0.14, so this only hands the call on. It's still
/// answered because the app asks the program it knows: the hub runs "$INYON_EXE" update
/// --background on a machine, and INYON_EXE names the engine on older installs.
fn update_shim(args: &[String]) -> ! {
    if let Some(cli) = paths::inyon_command() {
        let mut c = std::process::Command::new(cli);
        c.arg("update").args(args);
        if crate::output::json() {
            c.arg("--json");
        }
        // the hub asks a windowless engine for this: without a console of its own, the inyon
        // command would open a window
        ops::console_child_std(&mut c);
        let code = c.status().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
        std::process::exit(code);
    }
    let how = if WIN { format!("irm {INSTALL_URL}/get.ps1 | iex") } else { format!("curl -fsSL {INSTALL_URL}/get.sh | sh") };
    let message = format!("This copy of inyon can't update itself. Install the new version with: {how}");
    if std::fs::create_dir_all(node_dir()).is_ok() {
        let _ = std::fs::write(node_dir().join("update-status"), format!("failed: {message}\n"));
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(node_dir().join("update.log")) {
            let _ = writeln!(f, "{} failed: {message}", crate::iso_now());
        }
    }
    fail(&message, 1)
}

/// Runs for as long as the process does (the link, the hub connection).
async fn forever() -> ! {
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("couldn't start the async runtime")
}

/// Answers one engine command. `args` come without the program's name; `--json` may be anywhere.
/// `hub` goes to `hub`, which runs the hub. Returns the exit code for commands that end.
pub fn run(args: &[String], hub: impl FnOnce(&[String]) -> i32) -> i32 {
    let argv = crate::output::take_json(args);
    let cmd = argv.first().map(String::as_str);
    let rest: Vec<String> = argv.iter().skip(1).cloned().collect();
    setup_background(cmd);
    let joined = paths::saved_hub().is_some();
    match cmd {
        Some("pair") => runtime().block_on(async {
            if let Err(e) = ops::ensure_shell() {
                fail(&e, 1);
            }
            on_signals();
            if pair(&rest).await {
                std::process::exit(0);
            }
            forever().await
        }),
        Some("link") => runtime().block_on(async {
            let config = link_options(&rest);
            if link::link_running(Some(config.port)).await {
                let pid = link::current_state().map(|s| s.pid).filter(|p| *p != 0);
                fail(&format!("The link is already running{}.", pid.map(|p| format!(" (pid {p})")).unwrap_or_else(|| format!(" on port {}", config.port))), 2);
            }
            if let Err(e) = ops::ensure_shell() {
                fail(&e, 1);
            }
            on_signals();
            if let Err(e) = link::start_link(config) {
                fail(&e, 1);
            }
            tidy::start_tidying(paths::home());
            show_tray("link");
            forever().await
        }),
        Some("join") => {
            crate::connect::join(rest.first().map(String::as_str));
            0
        }
        // tether-node's installers started the hub connection with no arguments
        Some("connect") | None if cmd.is_some() || joined => runtime().block_on(async {
            if let Err(e) = ops::ensure_shell() {
                log(&format!("! {e}"));
            }
            on_signals();
            tidy::start_tidying(paths::home());
            show_tray("node");
            crate::connect::connect_mode(rest.first().cloned(), None).await;
            0
        }),
        Some("hub") => hub(&rest),
        Some("update") => update_shim(&rest),
        // a program run with no window, which this one has none of on Windows either: the inyon
        // command's one-off update task starts `inyon update` through it
        Some("hidden") => {
            if rest.is_empty() {
                fail("usage: inyon-engine hidden <program> [arguments…]", 2);
            }
            let mut c = std::process::Command::new(&rest[0]);
            c.args(&rest[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            ops::hide_window_std(&mut c);
            match c.status() {
                Ok(s) => s.code().unwrap_or(1),
                Err(e) => fail(&e.to_string(), 1),
            }
        }
        Some("phones") => {
            phones();
            0
        }
        Some("forget") => match forget(rest.first().map(String::as_str)) {
            Ok((key, name)) => {
                emit(json!({ "forgotten": { "key": key, "name": name } }), Some(&format!("Forgot {name}. It can't connect until it pairs again.")));
                0
            }
            Err(e) => fail(&e, 2),
        },
        Some("push") => {
            runtime().block_on(push::push_command(&rest));
            0
        }
        Some("version" | "--version") => {
            emit(json!({ "version": VERSION, "engine": true }), Some(VERSION));
            0
        }
        Some("help" | "--help" | "-h") => {
            log(&usage());
            0
        }
        // the form tether-node's installers used: a hub address, with the token in the environment
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
            let (url, token) = (url.to_string(), rest.first().cloned());
            runtime().block_on(async move {
                on_signals();
                tidy::start_tidying(paths::home());
                crate::connect::connect_mode(Some(url), token).await;
                0
            })
        }
        _ => fail(&usage(), 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answers `pair` gives while the link's state goes through `states` (after each delay),
    /// with short waits so the test takes a second or two.
    fn answers_while(config: LinkConfig, states: Vec<(u64, Vec<&'static str>)>) -> Vec<Value> {
        answers_with(config, states, false)
    }

    fn answers_with(config: LinkConfig, states: Vec<(u64, Vec<&'static str>)>, once: bool) -> Vec<Value> {
        let _h = crate::paths::testing::home(&format!("pair-wait-{}-{once}", states.len()));
        let pairing = link::new_pairing().unwrap();
        let patience = Patience { quick: Duration::from_millis(100), first: Duration::from_millis(600), tick: Duration::from_millis(20), until: Duration::from_secs(4) };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut answers = Vec::new();
        rt.block_on(async {
            tokio::spawn(async move {
                for (after, urls) in states {
                    tokio::time::sleep(Duration::from_millis(after)).await;
                    let s =
                        link::State { urls: urls.into_iter().map(String::from).collect(), relays: vec![], updated_at: 0, pid: std::process::id(), version: None, last_seq: None };
                    link::write_node_json("state.json", &s).unwrap();
                }
            });
            let publish = |_urls: Vec<String>, _relays: Vec<String>| async { true };
            pairing_answers(&config, false, &pairing, "machine-id", &patience, once, publish, &mut |v| answers.push(v.clone())).await;
        });
        answers
    }

    fn urls(v: &Value) -> Vec<String> {
        v.get("urls").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
    }

    #[test]
    fn a_link_already_up_answers_once_with_its_address() {
        let config = LinkConfig { vpn: false, ..LinkConfig::default() };
        let a = answers_while(config, vec![(0, vec!["wss://a.trycloudflare.com/link"])]);
        assert_eq!(a.len(), 1, "{a:?}");
        assert_eq!(urls(&a[0]), ["wss://a.trycloudflare.com/link"]);
        assert!(a[0].get("waiting").is_none() && a[0].get("qr").is_some());
    }

    #[test]
    fn pair_says_the_tunnel_is_starting_and_shows_the_code_once_its_address_is_there() {
        // the tunnel's address comes after the first look but before the code would be shown
        // without one (in 0.19.0 that code came after 30 s, with no address in it)
        let config = LinkConfig { vpn: false, ..LinkConfig::default() };
        let a = answers_while(config, vec![(0, vec![]), (300, vec!["wss://a.trycloudflare.com/link"])]);
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0], json!({ "waiting": "tunnel", "service": false }));
        assert_eq!(urls(&a[1]), ["wss://a.trycloudflare.com/link"]);
        assert!(a[1].get("waiting").is_none());
    }

    #[test]
    fn a_slow_tunnel_gets_the_code_shown_with_what_there_is_and_again_once_it_is_up() {
        let a = answers_while(LinkConfig::default(), vec![(0, vec!["ws://100.64.0.9:4780/link"]), (1200, vec!["wss://a.trycloudflare.com/link", "ws://100.64.0.9:4780/link"])]);
        assert_eq!(a.len(), 3, "{a:?}");
        assert_eq!(a[0]["waiting"], "tunnel");
        // the VPN works meanwhile, so that code is shown, marked as waiting for the tunnel
        assert_eq!((a[1]["waiting"].as_str(), urls(&a[1])), (Some("tunnel"), vec!["ws://100.64.0.9:4780/link".to_string()]));
        // then the same pairing again, now with the tunnel
        assert_eq!(urls(&a[2]), ["wss://a.trycloudflare.com/link", "ws://100.64.0.9:4780/link"]);
        assert!(a[2].get("waiting").is_none());
        let secret = |v: &Value| v["link"].as_str().unwrap().split(['?', '&']).find(|p| p.starts_with("s=")).map(String::from);
        assert!(secret(&a[1]).is_some());
        assert_eq!(secret(&a[1]), secret(&a[2]));
    }

    #[test]
    fn a_tunnel_that_never_comes_stops_the_wait_when_the_pairing_ends() {
        let config = LinkConfig { vpn: false, ..LinkConfig::default() };
        let started = std::time::Instant::now();
        let a = answers_while(config, vec![(0, vec![])]);
        assert_eq!(a.len(), 3, "{a:?}");
        assert!(a[1]["waiting"] == "tunnel" && urls(&a[1]).is_empty());
        // the last answer says so, for the window and `pair --json`, which may show nothing else
        assert_eq!(a[2], json!({ "ok": false, "error": crate::output::PAIR_EXPIRED, "expired": true }));
        assert!(crate::output::render_pair(&a[2]).contains("expired before the tunnel came up"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn with_once_the_code_after_the_first_wait_is_the_last_answer() {
        // an installer with no terminal: no one waits ten minutes for a tunnel that can't start
        let config = LinkConfig { vpn: false, ..LinkConfig::default() };
        let a = answers_with(config, vec![(0, vec![])], true);
        // two answers and done: waiting on, it would end with the code expiring (a third)
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0]["waiting"], "tunnel");
        assert!(a[1].get("waiting").is_none() && a[1].get("qr").is_some() && urls(&a[1]).is_empty());
        assert!(crate::output::render_pair(&a[1]).contains("No address yet"));
    }

    #[test]
    fn an_own_address_must_be_a_websocket_ending_in_link() {
        for good in ["wss://laptop.tail1234.ts.net:8443/link", "ws://10.0.0.2:4780/link"] {
            assert!(link_url(good), "{good}");
        }
        for bad in ["wss://link", "ws:///link", "wss://h/x/link", "https://h/link", "wss://h/links", "wss://"] {
            assert!(!link_url(bad), "{bad}");
        }
    }

    #[test]
    fn the_usage_names_every_command_and_the_version() {
        let u = usage();
        assert!(u.starts_with(&format!("inyon-engine {VERSION}: ")));
        for c in ["pair", "link", "phones", "forget", "push", "join", "connect", "hub", "version"] {
            assert!(u.contains(&format!("inyon-engine {c}")), "{c}");
        }
    }

    #[test]
    fn link_options_are_saved_for_the_service() {
        let _h = crate::paths::testing::home("cli-options");
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let c = link_options(&s(&[
            "--no-tunnel",
            "--lan",
            "--port",
            "4791",
            "--url",
            "wss://a.example:8443/link",
            "--url",
            "ws://10.0.0.2:4780/link",
            "--relay",
            "https://relay.example/",
        ]));
        assert_eq!((c.tunnel, c.lan, c.port), (false, true, 4791));
        assert_eq!(c.urls, ["wss://a.example:8443/link", "ws://10.0.0.2:4780/link"]);
        assert_eq!(c.relays, ["https://relay.example"]);
        assert_eq!(link::load_config(), c);
        // a new --url replaces the saved ones; nothing given keeps them
        let c = link_options(&s(&["--url", "wss://b.example/link"]));
        assert_eq!(c.urls, ["wss://b.example/link"]);
        assert_eq!(link_options(&[]).urls, ["wss://b.example/link"]);
    }
}
