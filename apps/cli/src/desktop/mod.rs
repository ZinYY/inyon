//! The Inyon app's window on Windows: a WebView2 window showing the pages in assets/app (This
//! computer, Phones, Hub, Pages, Temp files, Activity, Settings), and a second view inside it for a
//! hub's web app or a page. The pages ask for everything through `call(cmd, args)`; this module
//! answers, and only [`win`] knows about Win32 and WebView2. Everything here works the same on
//! every system and is tested on every system; tests/app/check.mjs clicks through the pages with a
//! stand-in for it.
//!
//! The window's own settings (theme, the hubs it shows, what closing it does) are
//! ~/.inyon/node/desktop.json. What it does goes to ~/.inyon/node/window.log.

pub mod pairing;
#[cfg(windows)]
pub mod win;

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths;
use crate::service::Role;
use crate::sys::RealProc;
use pairing::Pairing;

/// The window's class: the app finds the open window by it.
pub const CLASS: &str = "InyonWindow";
/// Where the window's pages are served from, and where a page in Pages is.
pub const APP_ORIGIN: &str = "http://inyon.localhost";
pub const PAGE_ORIGIN: &str = "http://inyonpage.localhost";

/// Whether `url` is one of the window's own pages: the origin itself, or a path on it. Not
/// inyon.localhost.example.com, nor another port.
pub fn own_page(url: &str) -> bool {
    url.strip_prefix(APP_ORIGIN).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// A line in window.log, with the time; on stderr too, for a run from a terminal.
pub fn trace(what: &str) {
    use std::io::Write;
    eprintln!("window: {what}");
    let dir = paths::node_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("window.log")) {
        let _ = writeln!(f, "{} {what}", crate::state::format_time(crate::state::now_ms()));
    }
}

static DESK: std::sync::OnceLock<Arc<Desk>> = std::sync::OnceLock::new();

/// The app's one [`Desk`], started the first time it's asked for (the window, the tray menu).
pub fn desk() -> Arc<Desk> {
    DESK.get_or_init(|| {
        let d = Desk::new();
        d.start();
        d
    })
    .clone()
}

/// Whether the theme setting comes out dark: `system` follows Windows' setting for apps.
pub fn looks_dark(theme: &str) -> bool {
    match theme {
        "dark" => true,
        "light" => false,
        _ => system_dark(),
    }
}

#[cfg(windows)]
fn system_dark() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let mut v: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            w("AppsUseLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut v as *mut u32 as *mut _,
            &mut size,
        )
    };
    r == 0 && v == 0
}

#[cfg(not(windows))]
fn system_dark() -> bool {
    false
}

// ---------- the window's settings ----------

/// A hub the window shows: one this computer joined, or any other you opened by its address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hub {
    pub url: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// system, light or dark
    pub theme: String,
    /// closing the window leaves Inyon running in the notification area (else it quits)
    pub close_to_tray: bool,
    /// the window opens at sign-in, not only the icon
    pub open_at_sign_in: bool,
    pub check_updates: bool,
    pub active_hub: Option<String>,
    pub last_page: String,
    pub hubs: Vec<Hub>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: "system".into(), close_to_tray: true, open_at_sign_in: false, check_updates: true, active_hub: None, last_page: "home".into(), hubs: Vec::new() }
    }
}

pub fn settings_file() -> PathBuf {
    paths::node_dir().join("desktop.json")
}

pub fn load_settings() -> Settings {
    load_settings_at(&settings_file())
}

fn load_settings_at(file: &Path) -> Settings {
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_settings(s: &Settings) -> Result<(), String> {
    save_settings_at(&settings_file(), s)
}

fn save_settings_at(file: &Path, s: &Settings) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| e.to_string())
}

/// Changes what `patch` names, refusing anything the window doesn't have or a value it can't take.
pub fn apply_patch(s: &mut Settings, patch: &Value) -> Result<(), String> {
    let Some(map) = patch.as_object() else { return Err("Settings come as an object.".into()) };
    for (k, v) in map {
        let flag = || v.as_bool().ok_or_else(|| format!("{k} is on or off."));
        match k.as_str() {
            "theme" => match v.as_str() {
                Some(t @ ("system" | "light" | "dark")) => s.theme = t.to_string(),
                _ => return Err("The theme is system, light or dark.".into()),
            },
            "closeToTray" => s.close_to_tray = flag()?,
            "openAtSignIn" => s.open_at_sign_in = flag()?,
            "checkUpdates" => s.check_updates = flag()?,
            "activeHub" => s.active_hub = v.as_str().map(String::from),
            "lastPage" => s.last_page = v.as_str().unwrap_or("home").chars().take(32).collect(),
            _ => return Err(format!("There's no setting called {k}.")),
        }
    }
    Ok(())
}

/// A hub's address as the window keeps it: http or https, with a host, without a trailing slash.
/// An address without a scheme gets https, or http on a VPN or this computer.
pub fn hub_url(text: &str) -> Result<String, String> {
    let has_scheme = text.contains("://");
    let t = text.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err("Enter the hub's address.".into());
    }
    let with_scheme = if has_scheme {
        t.to_string()
    } else {
        let host = t.split([':', '/']).next().unwrap_or(t);
        let plain = host == "localhost" || host.starts_with("127.") || host.starts_with("100.");
        format!("{}://{t}", if plain { "http" } else { "https" })
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| format!("{t} isn't a web address."))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(format!("{t} isn't a hub's web address (it starts with https:// or http://)."));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// What a hub is called when you didn't name it: its host (and port).
pub fn hub_name(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .map(|u| match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => url.to_string(),
        })
        .unwrap_or_else(|| url.to_string())
}

/// Adds a hub to the list (or renames it when it's there) and shows it.
pub fn add_hub(s: &mut Settings, url: &str, name: &str) -> Result<String, String> {
    let url = hub_url(url)?;
    let name = if name.trim().is_empty() { hub_name(&url) } else { name.trim().chars().take(60).collect() };
    match s.hubs.iter_mut().find(|h| h.url == url) {
        Some(h) => h.name = name,
        None => s.hubs.push(Hub { url: url.clone(), name }),
    }
    s.active_hub = Some(url.clone());
    Ok(url)
}

pub fn remove_hub(s: &mut Settings, url: &str) {
    s.hubs.retain(|h| h.url != url);
    if s.active_hub.as_deref() == Some(url) {
        s.active_hub = s.hubs.first().map(|h| h.url.clone());
    }
}

/// The hubs the Hub screen lists: the one this computer joined first (it's always there), then the
/// others, each saying whether it's the joined one.
pub fn hubs_view(s: &Settings, joined: Option<&str>) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(j) = joined {
        let name = s.hubs.iter().find(|h| h.url == j).map(|h| h.name.clone()).unwrap_or_else(|| hub_name(j));
        out.push(json!({ "url": j, "name": name, "joined": true }));
    }
    for h in s.hubs.iter().filter(|h| Some(h.url.as_str()) != joined) {
        out.push(json!({ "url": h.url, "name": h.name, "joined": false }));
    }
    out
}

// ---------- the pages' files ----------

/// The window's own files, built in.
pub fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match path.trim_start_matches('/') {
        "" | "index.html" => (include_bytes!("../../assets/app/index.html"), "text/html; charset=utf-8"),
        "app.css" => (include_bytes!("../../assets/app/app.css"), "text/css; charset=utf-8"),
        "app.js" => (include_bytes!("../../assets/app/app.js"), "text/javascript; charset=utf-8"),
        "geist.woff2" => (include_bytes!("../../assets/app/geist.woff2"), "font/woff2"),
        "geist-mono.woff2" => (include_bytes!("../../assets/app/geist-mono.woff2"), "font/woff2"),
        _ => return None,
    })
}

/// The content type for a file a page or the temp files screen shows.
pub fn mime_for(path: &Path) -> &'static str {
    match path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("pdf") => "application/pdf",
        Some("txt" | "md" | "log" | "csv") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The file `request_path` (/<slug>/<path inside the site>) asks for, in the folder of the page
/// with that slug: index.html for a folder. Never outside the site's folder: `..` is refused, and
/// so is a symlink that leads out of it.
pub fn page_file(request_path: &str, pages: &[Value]) -> Result<PathBuf, &'static str> {
    let path = request_path.split(['?', '#']).next().unwrap_or_default();
    let decoded = percent_decode(path.trim_start_matches('/'));
    let (slug, rest) = decoded.split_once('/').unwrap_or((decoded.as_str(), ""));
    let page = pages.iter().find(|p| p.get("slug").and_then(Value::as_str) == Some(slug)).ok_or("no such page")?;
    let root = PathBuf::from(page.get("path").and_then(Value::as_str).ok_or("no such page")?);
    let mut file = root.clone();
    for part in Path::new(rest).components() {
        match part {
            Component::Normal(p) => file.push(p),
            Component::CurDir => {}
            _ => return Err("outside the page's folder"),
        }
    }
    if file.is_dir() {
        file.push("index.html");
    }
    let (real_root, real_file) = (root.canonicalize().map_err(|_| "no such page")?, file.canonicalize().map_err(|_| "not found")?);
    if !real_file.starts_with(&real_root) {
        return Err("outside the page's folder");
    }
    Ok(real_file)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(n) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(n);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The pages in Pages on this computer, each with where the window opens it.
pub fn local_pages() -> Vec<Value> {
    let Ok(list) = crate::pages::read_list(&crate::pages::pages_file()) else { return Vec::new() };
    let mut seen = std::collections::HashSet::new();
    list.get("pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.get("slug").and_then(Value::as_str).is_some_and(|s| seen.insert(s.to_string())) && p.get("path").and_then(Value::as_str).is_some())
        .map(|p| {
            let slug = p["slug"].as_str().unwrap_or_default().to_string();
            let project = p.get("project").and_then(Value::as_str).map(|x| paths::basename(x.trim_end_matches(['/', '\\'])).to_string()).unwrap_or_default();
            json!({ "slug": slug, "name": p.get("name").and_then(Value::as_str).unwrap_or(&slug), "path": p["path"], "project": project, "url": format!("{PAGE_ORIGIN}/{slug}/") })
        })
        .collect()
}

// ---------- the QR code ----------

/// The pairing link as a QR code in SVG, dark on white with the quiet zone, one path.
pub fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let n = code.width();
    let colors = code.to_colors();
    let mut d = String::new();
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if colors[y * n + x] == qrcode::Color::Dark {
                let start = x;
                while x < n && colors[y * n + x] == qrcode::Color::Dark {
                    x += 1;
                }
                d.push_str(&format!("M{} {}h{}v1h-{}z", start + 4, y + 4, x - start, x - start));
            } else {
                x += 1;
            }
        }
    }
    let size = n + 8;
    Some(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" shape-rendering="crispEdges"><rect width="{size}" height="{size}" fill="#fff"/><path fill="#1d1c1a" d="{d}"/></svg>"##
    ))
}

// ---------- logs ----------

/// The log a tab of Activity shows.
pub fn log_path(which: &str) -> Option<PathBuf> {
    Some(match which {
        "app" => paths::node_dir().join("app.log"),
        "link" => crate::service::log_file(Role::Link),
        "node" => crate::service::log_file(Role::Node),
        "update" => paths::node_dir().join("update.log"),
        "window" => paths::node_dir().join("window.log"),
        _ => return None,
    })
}

/// The last `lines` lines of `file`, reading at most its last 512 KB.
pub fn tail(file: &Path, lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(file) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(512 * 1024);
    let _ = f.seek(SeekFrom::Start(from));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().skip(usize::from(from > 0)).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

// ---------- what the window knows ----------

/// What the update check found.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Looking,
    Current,
    Available(String),
    Updating(String),
    Done(String),
    Failed,
}

/// The slow things, found on a thread of their own and kept here: Task Scheduler, the networks,
/// the firewall, WSL. The pages ask for the state twice a second; it never waits for these.
#[derive(Debug, Default)]
struct Slow {
    link_task: bool,
    node_task: bool,
    vpns: Vec<String>,
    lan_ip: Option<String>,
    firewall: Option<bool>,
    wsl: Option<String>,
    found: bool,
}

/// a day of the slow loop's 5-second rounds
const DAY_ROUNDS: u64 = 24 * 3600 / 5;

/// What the pages' state needs from the system: which jobs run (a tasklist each on Windows) and
/// whether the link answers.
#[derive(Clone)]
struct Probes {
    app: Option<crate::app::AppState>,
    link_running: bool,
    node_running: bool,
}

/// A value kept for `ttl` ms, so the pages' 1.5-second refresh doesn't ask the system each time.
struct Cached<T> {
    value: Option<(f64, T)>,
    ttl: f64,
}

impl<T: Clone> Cached<T> {
    const fn new(ttl: f64) -> Self {
        Cached { value: None, ttl }
    }

    fn get(&mut self, now: f64, make: impl FnOnce() -> T) -> T {
        if let Some((at, v)) = &self.value {
            if now - at < self.ttl && now >= *at {
                return v.clone();
            }
        }
        let v = make();
        self.value = Some((now, v.clone()));
        v
    }

    fn clear(&mut self) {
        self.value = None;
    }
}

pub struct Desk {
    slow: Mutex<Slow>,
    /// kept 5 s, and dropped by anything the window does so its result shows at once
    probes: Mutex<Cached<Probes>>,
    pairing: Mutex<Pairing>,
    update: Mutex<Update>,
    /// the reach being applied (LAN, VPN, tunnel), and why the last change failed
    applying: Mutex<Option<(bool, bool, bool)>>,
    reach_error: Mutex<Option<String>>,
    pub settings: Mutex<Settings>,
    /// the page to open on, and whether to start a pairing code
    pub start_page: Mutex<Option<String>>,
}

impl Desk {
    pub fn new() -> Arc<Desk> {
        Arc::new(Desk {
            slow: Mutex::new(Slow::default()),
            probes: Mutex::new(Cached::new(5000.0)),
            pairing: Mutex::new(Pairing::default()),
            update: Mutex::new(Update::Looking),
            applying: Mutex::new(None),
            reach_error: Mutex::new(None),
            settings: Mutex::new(load_settings()),
            start_page: Mutex::new(None),
        })
    }

    /// Starts the threads that keep the slow things fresh, and the update check.
    pub fn start(self: &Arc<Self>) {
        let d = self.clone();
        std::thread::spawn(move || {
            let mut round = 0u64;
            loop {
                d.find_slow(round);
                // and once a day, as Settings says
                if round > 0 && round.is_multiple_of(DAY_ROUNDS) && lock(&d.settings).check_updates && !matches!(*lock(&d.update), Update::Updating(_)) {
                    d.check_update();
                }
                round += 1;
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
        if lock(&self.settings).check_updates {
            let d = self.clone();
            std::thread::spawn(move || d.check_update());
        } else {
            *lock(&self.update) = Update::Current;
        }
    }

    fn find_slow(&self, round: u64) {
        let proc = RealProc;
        let sys = crate::service::Sys::new(&proc);
        // the tasks every 15 s, the rest every 30 s (and all of it first)
        if round.is_multiple_of(3) {
            let (l, n) = (crate::service::task_exists(&sys, Role::Link), crate::service::task_exists(&sys, Role::Node));
            let mut s = lock(&self.slow);
            s.link_task = l;
            s.node_task = n;
        }
        if round.is_multiple_of(6) {
            let nets = crate::networks::detect();
            let vpns = nets.iter().filter(|n| n.kind != crate::networks::Kind::Lan).map(|n| format!("{} {}", n.kind.label(), n.ip)).collect();
            let lan_ip = nets.iter().find(|n| n.kind == crate::networks::Kind::Lan).map(|n| n.ip.clone());
            let firewall = cfg!(windows).then(|| crate::service::firewall_allows(crate::state::link_config().port.unwrap_or(4780)));
            let wsl = if round == 0 { wsl_distro() } else { lock(&self.slow).wsl.clone() };
            let mut s = lock(&self.slow);
            s.vpns = vpns;
            s.lan_ip = lan_ip;
            s.firewall = firewall;
            s.wsl = wsl;
        }
        lock(&self.slow).found = true;
    }

    fn check_update(&self) {
        *lock(&self.update) = Update::Looking;
        let found = match crate::update::latest_version(crate::update::RELEASES) {
            Ok(v) if crate::update::is_newer(&v, crate::ui::VERSION) => Update::Available(v),
            Ok(_) => Update::Current,
            Err(e) => {
                trace(&format!("no update check: {e}"));
                Update::Failed
            }
        };
        *lock(&self.update) = found;
    }

    /// Everything the pages show, in one object.
    pub fn snapshot(&self) -> Value {
        let proc = RealProc;
        let now = crate::state::now_ms();
        let today = crate::state::format_date(now);
        let cfg = crate::state::link_config();
        let port = cfg.port.unwrap_or(4780);
        let Probes { app, link_running, node_running } = lock(&self.probes).get(now, || {
            let app = crate::app::running(&proc);
            let alive = |role: Role| {
                app.as_ref().and_then(|a| a.job(role)).and_then(|j| j.pid).is_some_and(|p| crate::sys::Proc::alive(&proc, p))
                    || crate::update::read_running(role).and_then(|r| r.pid).is_some_and(|p| crate::sys::Proc::alive(&proc, p))
            };
            let (link_running, node_running) = (alive(Role::Link) || crate::service::link_answers(port), alive(Role::Node));
            Probes { app, link_running, node_running }
        });
        let addresses: Vec<String> = if link_running { crate::state::link_state().map(|s| s.urls.into_iter().chain(s.relays).collect()).unwrap_or_default() } else { Vec::new() };
        let phones: Vec<crate::ui::dashboard::PairedPhone> = crate::state::phones()
            .into_iter()
            .map(|(key, p)| crate::ui::dashboard::PairedPhone {
                key: key.chars().take(12).collect(),
                name: p.name,
                paired: crate::state::format_date(p.paired_at),
                last_seen: p.last_seen.map(crate::state::format_time),
            })
            .collect();
        let pairing = {
            let mut p = lock(&self.pairing);
            p.watch(&phones, crate::state::pairing_open(), now);
            p.to_json(&qr_svg)
        };
        let slow = lock(&self.slow);
        let (lan, vpn, tunnel) = pairing::reach(&cfg, cfg!(windows));
        let applying = *lock(&self.applying);
        let tunnel_url =
            addresses.iter().find(|a| a.starts_with("wss://") && a.contains("trycloudflare.com")).map(|a| a.trim_start_matches("wss://").trim_end_matches("/link").to_string());
        let joined = paths::saved_hub();
        let problem = joined.as_ref().filter(|_| node_running).and_then(|_| crate::commands::connection_note()).filter(|(bad, _)| *bad).map(|(_, p)| p);
        let settings = lock(&self.settings).clone();
        let update = match &*lock(&self.update) {
            _ if crate::update::under_way() => json!({ "state": "updating", "latest": "" }),
            Update::Looking => json!({ "state": "looking" }),
            Update::Current => json!({ "state": "current", "latest": crate::ui::VERSION }),
            Update::Available(v) => json!({ "state": "available", "latest": v }),
            Update::Updating(v) => json!({ "state": "updating", "latest": v }),
            Update::Done(v) => json!({ "state": "done", "latest": v }),
            Update::Failed => json!({ "state": "failed" }),
        };
        let set_up = crate::state::identity().is_some() || slow.link_task || link_running;
        json!({
            "version": crate::ui::VERSION,
            "host": inyon_engine::hostname(),
            "platformName": platform_name(),
            "app": { "running": app.as_ref().map(|a| a.running.clone()).unwrap_or_default(), "pid": app.as_ref().map(|a| a.pid) },
            "update": update,
            "link": { "setUp": set_up, "running": link_running, "autostart": slow.link_task, "port": port, "addresses": addresses },
            "reach": {
                "tunnel": tunnel, "lan": lan, "vpn": vpn,
                "applying": applying.map(|(l, v, t)| json!({ "lan": l, "vpn": v, "tunnel": t })),
                "error": lock(&self.reach_error).clone(),
                "firewall": slow.firewall,
                "tunnelUrl": tunnel_url,
                "lanIp": slow.lan_ip,
                "vpns": slow.vpns,
            },
            "phones": phones.iter().map(|p| json!({ "key": p.key, "name": p.name, "paired": pairing::paired_short(&p.paired, &today), "lastSeen": p.last_seen.as_deref().map(|t| pairing::seen_short(t, &today)) })).collect::<Vec<_>>(),
            "pairing": pairing,
            "joined": joined.as_ref().map(|u| json!({ "url": u, "running": node_running, "problem": problem, "autostart": slow.node_task })),
            "hubs": hubs_view(&settings, joined.as_deref()),
            "pages": local_pages(),
            "settings": settings,
            "wsl": slow.wsl,
            "startPage": lock(&self.start_page).take(),
            "notice": Value::Null,
        })
    }

    /// Answers one of the pages' calls; slow ones (setup, joining a hub) take their time, since
    /// each call runs on a thread of its own.
    pub fn handle(self: &Arc<Self>, cmd: &str, args: &Value) -> Result<Value, String> {
        let proc = RealProc;
        let sys = crate::service::Sys::new(&proc);
        let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(false);
        if cmd != "state" {
            lock(&self.probes).clear();
        }
        match cmd {
            "state" => Ok(self.snapshot()),
            "remember" => {
                let mut s = lock(&self.settings);
                apply_patch(&mut s, &json!({ "lastPage": text("page") }))?;
                save_settings(&s)?;
                Ok(Value::Null)
            }
            "settings" => {
                let mut s = lock(&self.settings);
                let mut next = s.clone();
                apply_patch(&mut next, args.get("patch").unwrap_or(&Value::Null))?;
                save_settings(&next)?;
                *s = next.clone();
                Ok(serde_json::to_value(next).unwrap_or_default())
            }
            "reach" | "setup" => {
                let (lan, vpn, tunnel) = (flag("lan"), flag("vpn"), flag("tunnel"));
                let Some(network) = pairing::network_flag(lan, vpn, tunnel) else {
                    return Err("Keep at least one way in, or phones can't reach this computer.".into());
                };
                {
                    let mut a = lock(&self.applying);
                    if a.is_some() {
                        return Err("A change is being applied; try again in a moment.".into());
                    }
                    *a = Some((lan, vpn, tunnel));
                }
                *lock(&self.reach_error) = None;
                trace(&format!("{cmd} --network {network}"));
                let r = run_setup(&network);
                *lock(&self.applying) = None;
                match r {
                    None => Ok(Value::Null),
                    Some(e) => {
                        trace(&format!("! setup: {e}"));
                        *lock(&self.reach_error) = Some(e.clone());
                        Err(e)
                    }
                }
            }
            "job" => {
                let role = match text("role").as_str() {
                    "link" => Role::Link,
                    "node" => Role::Node,
                    r => return Err(format!("There's no job called {r}.")),
                };
                let what = match text("action").as_str() {
                    "start" => crate::app::Do::Start,
                    "stop" => crate::app::Do::Stop,
                    "restart" => crate::app::Do::Restart,
                    a => return Err(format!("A job can't {a}.")),
                };
                trace(&format!("{what:?} the {role}"));
                let r = crate::app::ask(&proc, what, Some(role), None, 10_000)?;
                if r.ok {
                    Ok(json!(r.message))
                } else {
                    Err(r.message)
                }
            }
            "pair" => {
                self.pair();
                Ok(Value::Null)
            }
            "pairCancel" => {
                lock(&self.pairing).cancel();
                Ok(Value::Null)
            }
            "forget" => {
                let key = text("key");
                trace(&format!("forgetting the phone {key}"));
                crate::commands::forget(&key).map(|v| v.get("message").cloned().unwrap_or_default())
            }
            "join" => self.join(&sys, &text("text")),
            "leave" => {
                trace("leaving the hub");
                let _ = crate::app::ask(&proc, crate::app::Do::Stop, Some(Role::Node), None, 10_000);
                let v = crate::commands::disconnect(&sys)?;
                Ok(v.get("message").cloned().unwrap_or_else(|| json!("This computer left the hub.")))
            }
            "hubAdd" => {
                let mut s = lock(&self.settings);
                let mut next = s.clone();
                add_hub(&mut next, &text("url"), &text("name"))?;
                save_settings(&next)?;
                *s = next.clone();
                Ok(json!({ "hubs": hubs_view(&next, paths::saved_hub().as_deref()), "settings": next }))
            }
            "hubRemove" => {
                let mut s = lock(&self.settings);
                remove_hub(&mut s, &text("url"));
                save_settings(&s)?;
                Ok(json!({ "hubs": hubs_view(&s, paths::saved_hub().as_deref()), "settings": *s }))
            }
            "hubCreate" => self.create_hub(),
            "pageAdd" => {
                let v = crate::pages::add_command(Path::new(&text("path")), None, None)?;
                let name = v.get("page").and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("The page");
                Ok(json!(format!("\"{name}\" is in Pages.")))
            }
            "pageRemove" => crate::pages::remove_command(&text("slug")).map(|v| v.get("message").cloned().unwrap_or_default()),
            "temp" => Ok(serde_json::to_value(crate::temp::list(crate::state::now_ms())).unwrap_or_default()),
            "tempFiles" => crate::temp::files(&text("path")).map(|f| serde_json::to_value(f).unwrap_or_default()),
            "tempPreview" => crate::temp::preview(&text("path"), 256 * 1024).map(|p| serde_json::to_value(p).unwrap_or_default()),
            "tempRemove" => {
                let paths: Vec<String> = args.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
                if paths.is_empty() {
                    return Err("Nothing was picked to delete.".into());
                }
                trace(&format!("deleting {} temp item(s)", paths.len()));
                Ok(serde_json::to_value(crate::temp::remove(&paths)).unwrap_or_default())
            }
            "checks" => crate::commands::doctor(&sys).map(|v| json!(pairing::check_list(&v))),
            "log" => {
                let file = log_path(&text("which")).ok_or("There's no such log.")?;
                let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(300).min(5000) as usize;
                Ok(json!(tail(&file, lines)))
            }
            "autostart" => self.autostart(&sys, flag("on")),
            "update" => {
                if flag("install") {
                    let v = match &*lock(&self.update) {
                        Update::Available(v) => v.clone(),
                        _ => String::new(),
                    };
                    trace(&format!("updating to {v}"));
                    *lock(&self.update) = Update::Updating(v);
                    let mut c = std::process::Command::new(inyon_command());
                    c.args(["update", "--background"]);
                    crate::sys::quiet(&mut c).spawn().map_err(|e| format!("Couldn't start the update ({e})."))?;
                } else {
                    self.check_update();
                }
                Ok(Value::Null)
            }
            "quit" => {
                trace("quit from the window");
                let _ = crate::app::send(&crate::app::Request::new(crate::app::Do::Quit, None, None));
                #[cfg(windows)]
                crate::app::win::wake();
                Ok(Value::Null)
            }
            "uninstall" => {
                trace("uninstalling from the window");
                let v = crate::commands::uninstall_all(&sys, true, false)?;
                Ok(v.get("message").cloned().unwrap_or_else(|| json!("Inyon is uninstalled.")))
            }
            other => Err(format!("The app doesn't know {other}.")),
        }
    }

    /// Whether the phone link or the hub connection starts at sign-in (as last found).
    pub fn sign_in(&self) -> bool {
        let s = lock(&self.slow);
        s.link_task || s.node_task
    }

    /// How phones reach this computer now: (LAN, VPN, tunnel), and whether a change is under way.
    pub fn reach_now(&self) -> ((bool, bool, bool), bool) {
        (pairing::reach(&crate::state::link_config(), cfg!(windows)), lock(&self.applying).is_some())
    }

    /// Turns one way in (`lan`, `vpn` or `tunnel`) on or off, keeping the others.
    pub fn flip_reach(self: &Arc<Self>, which: &str) -> Result<Value, String> {
        let ((lan, vpn, tunnel), _) = self.reach_now();
        let next = match which {
            "lan" => (!lan, vpn, tunnel),
            "vpn" => (lan, !vpn, tunnel),
            "tunnel" => (lan, vpn, !tunnel),
            w => return Err(format!("There's no way in called {w}.")),
        };
        self.handle("reach", &json!({ "lan": next.0, "vpn": next.1, "tunnel": next.2 }))
    }

    /// Starts a pairing code, from the link the app runs.
    pub fn pair(self: &Arc<Self>) {
        let phones: Vec<crate::ui::dashboard::PairedPhone> = crate::state::phones()
            .into_keys()
            .map(|key| crate::ui::dashboard::PairedPhone { key: key.chars().take(12).collect(), name: String::new(), paired: String::new(), last_seen: None })
            .collect();
        let Some(n) = lock(&self.pairing).ask(&phones, crate::state::now_ms()) else { return };
        trace("asking the engine for a pairing code");
        let d = self.clone();
        std::thread::spawn(move || {
            // the icon shows the pairing while it's made, and the app doesn't end under it
            let _pairing = crate::app::Pairing::start();
            if crate::app::in_app() {
                ensure_link();
            }
            let mut last_starting = None;
            let r = crate::engine::pair(&[], &mut |v| {
                last_starting = Some(matches!(pairing::code_card(v), pairing::CodeCard::Starting(_)));
                lock(&d.pairing).answer(n, v.clone());
            });
            if let Err(e) = &r {
                trace(&format!("the pairing's engine ended: {e}"));
            }
            if let Some(why) = pairing::pairing_ended(r, last_starting) {
                lock(&d.pairing).answer(n, json!({ "error": why }));
            }
        });
    }

    fn join(&self, sys: &crate::service::Sys, text: &str) -> Result<Value, String> {
        let m = crate::commands::parse_add_machine(text)?;
        let Some(token) = m.token.as_deref() else {
            return Err("That's the hub's address without its token. On the hub, open Machines, choose Add a machine and paste the whole command.".into());
        };
        crate::commands::check_hub_url(&m.hub)?;
        trace(&format!("joining {}", m.hub));
        let v = crate::commands::join(sys, &m.hub, token, m.cert.as_deref(), true)?;
        if crate::app::in_app() {
            let proc = RealProc;
            let _ = crate::app::ask(&proc, crate::app::Do::Start, Some(Role::Node), None, 10_000);
        }
        let mut s = lock(&self.settings);
        let mut next = s.clone();
        add_hub(&mut next, &m.hub, "")?;
        save_settings(&next)?;
        *s = next;
        Ok(v.get("message").cloned().unwrap_or_else(|| json!(format!("This computer is on {} now.", hub_name(&m.hub)))))
    }

    fn autostart(&self, sys: &crate::service::Sys, on: bool) -> Result<Value, String> {
        let mut roles = Vec::new();
        if crate::state::identity().is_some() {
            roles.push(Role::Link);
        }
        if paths::saved_hub().is_some() {
            roles.push(Role::Node);
        }
        if roles.is_empty() {
            return Err("Set up the phone link or add this computer to a hub first.".into());
        }
        for role in roles {
            crate::service::set_sign_in(sys, role, on)?;
        }
        let mut s = lock(&self.slow);
        s.link_task = on && crate::state::identity().is_some();
        s.node_task = on && paths::saved_hub().is_some();
        Ok(json!(if on { "Inyon starts when you sign in." } else { "Inyon no longer starts when you sign in. It runs until you quit it." }))
    }

    fn create_hub(&self) -> Result<Value, String> {
        let distro = lock(&self.slow).wsl.clone().ok_or("WSL isn't installed here. Install it with `wsl --install` in PowerShell as administrator, then restart.")?;
        trace(&format!("creating a hub in WSL ({distro})"));
        let script = "set -e; command -v curl >/dev/null || { echo 'curl is missing in WSL: sudo apt install curl' >&2; exit 1; }; curl -fsSL https://inyon.tech/install.sh | sh -s -- --hub --yes";
        let mut c = std::process::Command::new("wsl.exe");
        c.args(["-d", &distro, "--", "sh", "-lc", script]);
        let o = crate::sys::quiet(&mut c).output().map_err(|e| format!("Couldn't start WSL ({e})."))?;
        let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        for l in said.lines().filter(|l| !l.trim().is_empty()) {
            trace(&format!("wsl: {l}"));
        }
        if !o.status.success() {
            return Err(said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("The hub's installer stopped in WSL.").trim().to_string());
        }
        let url = "http://localhost:4777".to_string();
        let mut s = lock(&self.settings);
        let mut next = s.clone();
        add_hub(&mut next, &url, &format!("This computer ({distro})"))?;
        save_settings(&next)?;
        *s = next;
        Ok(json!(url))
    }
}

/// Holds a lock even when a thread that held it panicked.
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn platform_name() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// The WSL distribution a hub would go into, when WSL is installed.
fn wsl_distro() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let mut c = std::process::Command::new("wsl.exe");
    c.args(["-l", "-q"]);
    let o = crate::sys::quiet(&mut c).output().ok().filter(|o| o.status.success())?;
    // wsl.exe answers in UTF-16
    let words: Vec<u16> = o.stdout.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
    String::from_utf16_lossy(&words)
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .find(|l| !l.is_empty() && !l.starts_with("docker-desktop"))
        .map(String::from)
}

/// inyon.exe, for what the window runs: setup, an update.
fn inyon_command() -> PathBuf {
    let me = std::env::current_exe().unwrap_or_else(|_| paths::installed_binary());
    if crate::app::started_as_app(&me.to_string_lossy()) {
        crate::app::command_beside(&me)
    } else {
        me
    }
}

/// Runs `inyon setup` with no window, changing only how phones reach this computer: None when it
/// went well, else why not.
fn run_setup(network: &str) -> Option<String> {
    let mut c = std::process::Command::new(inyon_command());
    c.args(["setup", "--yes", "--role", "node", "--no-code", "--network", network]);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    match crate::sys::quiet(&mut c).output() {
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            for l in text.lines().filter(|l| !l.trim().is_empty()) {
                trace(&format!("setup: {l}"));
            }
            pairing::setup_result(o.status.success(), &text)
        }
        Err(e) => Some(format!("Couldn't run setup ({e}).")),
    }
}

/// Has the app run the link and waits (up to 20 s) until it answers, so `pair` asks that link for
/// the code instead of starting one of its own.
fn ensure_link() {
    let proc = RealProc;
    if let Err(e) = crate::app::ask(&proc, crate::app::Do::Start, Some(Role::Link), None, 5000) {
        trace(&format!("! the app didn't start the link: {e}"));
        return;
    }
    let port = crate::state::link_config().port.unwrap_or(4780);
    for _ in 0..40 {
        if crate::service::link_answers(port) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    trace("the link isn't answering yet; asking for the code anyway");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_windows_own_pages_count_as_its_own() {
        for url in ["http://inyon.localhost", "http://inyon.localhost/", "http://inyon.localhost/index.html?page=hub"] {
            assert!(own_page(url), "{url}");
        }
        for url in ["http://inyon.localhost.example.com/", "http://inyon.localhost:8080/", "http://inyon.localhostx/", "https://inyon.localhost/", "http://inyonpage.localhost/a/"]
        {
            assert!(!own_page(url), "{url}");
        }
    }

    #[test]
    fn the_system_is_asked_again_only_after_a_while_or_after_an_action() {
        let mut c = Cached::new(5000.0);
        let mut asked = 0;
        let mut ask = |now: f64, c: &mut Cached<u32>| {
            c.get(now, || {
                asked += 1;
                asked
            })
        };
        assert_eq!(ask(1000.0, &mut c), 1);
        assert_eq!(ask(2500.0, &mut c), 1);
        assert_eq!(ask(6100.0, &mut c), 2);
        c.clear();
        assert_eq!(ask(6200.0, &mut c), 3);
        // a clock that went back doesn't keep an old answer
        assert_eq!(ask(100.0, &mut c), 4);
    }

    #[test]
    fn settings_take_only_what_the_window_has() {
        let mut s = Settings::default();
        apply_patch(&mut s, &json!({ "theme": "dark", "closeToTray": false, "openAtSignIn": true })).unwrap();
        assert_eq!((s.theme.as_str(), s.close_to_tray, s.open_at_sign_in), ("dark", false, true));
        assert!(apply_patch(&mut s, &json!({ "theme": "neon" })).is_err());
        assert!(apply_patch(&mut s, &json!({ "closeToTray": "yes" })).is_err());
        assert!(apply_patch(&mut s, &json!({ "password": "x" })).is_err());
        // what isn't in the file comes from the defaults
        let dir = std::env::temp_dir().join(format!("inyon-desk-{}", std::process::id()));
        let file = dir.join("desktop.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, r#"{"theme":"light","hubs":[{"url":"https://h","name":"h"}]}"#).unwrap();
        let s = load_settings_at(&file);
        assert_eq!((s.theme.as_str(), s.close_to_tray, s.hubs.len()), ("light", true, 1));
        save_settings_at(&file, &s).unwrap();
        assert_eq!(load_settings_at(&file), s);
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(load_settings_at(&file), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hub_address_is_kept_tidy_and_the_joined_hub_comes_first() {
        assert_eq!(hub_url("https://hub.example.net/").unwrap(), "https://hub.example.net");
        assert_eq!(hub_url("100.97.188.112:4777").unwrap(), "http://100.97.188.112:4777");
        assert_eq!(hub_url("hub.example.net").unwrap(), "https://hub.example.net");
        assert_eq!(hub_url(" localhost:4777 ").unwrap(), "http://localhost:4777");
        assert!(hub_url("").is_err());
        assert!(hub_url("ftp://x").is_err());
        assert!(hub_url("https://").is_err());
        assert_eq!(hub_name("http://100.97.188.112:4777"), "100.97.188.112:4777");
        let mut s = Settings::default();
        add_hub(&mut s, "https://a.example", "").unwrap();
        add_hub(&mut s, "http://100.64.0.5:4777", "Home").unwrap();
        add_hub(&mut s, "https://a.example/", "Work").unwrap();
        assert_eq!(s.hubs, vec![Hub { url: "https://a.example".into(), name: "Work".into() }, Hub { url: "http://100.64.0.5:4777".into(), name: "Home".into() }]);
        assert_eq!(s.active_hub.as_deref(), Some("https://a.example"));
        let v = hubs_view(&s, Some("https://joined.example"));
        assert_eq!(v[0], json!({ "url": "https://joined.example", "name": "joined.example", "joined": true }));
        assert_eq!(v.len(), 3);
        let v = hubs_view(&s, Some("http://100.64.0.5:4777"));
        assert_eq!(v[0]["name"], "Home");
        assert_eq!(v.len(), 2, "the joined hub isn't listed twice");
        remove_hub(&mut s, "https://a.example");
        assert_eq!(s.active_hub.as_deref(), Some("http://100.64.0.5:4777"));
    }

    #[test]
    fn a_page_serves_its_own_files_and_nothing_outside_its_folder() {
        let dir = std::env::temp_dir().join(format!("inyon-page-{}", std::process::id()));
        let site = dir.join("site");
        std::fs::create_dir_all(site.join("kit")).unwrap();
        std::fs::write(site.join("index.html"), "<h1>plan</h1>").unwrap();
        std::fs::write(site.join("kit/app.css"), "body{}").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        let pages = vec![json!({ "slug": "plan", "path": site.to_string_lossy() })];
        let real = |p: &Path| p.canonicalize().unwrap();
        assert_eq!(page_file("/plan/", &pages).unwrap(), real(&site.join("index.html")));
        assert_eq!(page_file("/plan/kit/app.css?v=2", &pages).unwrap(), real(&site.join("kit/app.css")));
        assert_eq!(page_file("/plan/kit%2Fapp.css", &pages).unwrap(), real(&site.join("kit/app.css")));
        assert!(page_file("/plan/../secret.txt", &pages).is_err());
        assert!(page_file("/plan/%2E%2E/secret.txt", &pages).is_err());
        assert!(page_file("/other/index.html", &pages).is_err());
        assert!(page_file("/plan/missing.html", &pages).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("secret.txt"), site.join("link.txt")).unwrap();
            assert_eq!(page_file("/plan/link.txt", &pages), Err("outside the page's folder"));
        }
        assert_eq!(mime_for(Path::new("a.SVG")), "image/svg+xml");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_windows_files_are_built_in() {
        let (html, kind) = asset("/").unwrap();
        assert!(kind.starts_with("text/html"));
        let html = String::from_utf8_lossy(html);
        assert!(html.contains("app.js") && html.contains("app.css"));
        assert!(asset("app.js").is_some() && asset("geist.woff2").is_some());
        assert!(asset("../Cargo.toml").is_none());
    }

    #[test]
    fn the_qr_code_is_one_svg_path_that_a_scanner_reads() {
        let svg = qr_svg("https://inyon.tech/app/#pair?x=1").unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("<path fill=\"#1d1c1a\" d=\"M"));
        // the finder pattern's top row starts at the quiet zone
        assert!(svg.contains("M4 4h7v1h-7z"), "{}", &svg[..300]);
    }

    #[test]
    fn a_log_tail_is_the_last_lines() {
        let file = std::env::temp_dir().join(format!("inyon-tail-{}.log", std::process::id()));
        std::fs::write(&file, (1..=50).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        assert_eq!(tail(&file, 3), "line 48\nline 49\nline 50");
        assert_eq!(tail(Path::new("/nonexistent/log"), 3), "");
        let _ = std::fs::remove_file(&file);
        assert!(log_path("app").is_some() && log_path("../etc").is_none());
    }
}
