//! Notifications for the web app while it's closed. The page has no server, so this machine sends
//! them: a paired browser hands over its push registration (`inyon push set`, run by the app over
//! the encrypted link), and when an agent's hook says it needs you or finished, this sends an
//! encrypted message straight to that browser's push service. Nothing goes through a server of
//! Inyon's, and the push service only sees ciphertext.
//!
//! The hooks write to ~/.inyon/events.jsonl, one JSON line per event with the tmux pane it came
//! from (see the hub's claude/integration.ts). (push.ts)

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::link::{list_phones, node_files, read_node_json, write_node_json};
use crate::output::{emit, error, log};
use crate::webpush::{self, Keys, Sent, Subscription, Vapid};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// the phone's link key, as phones.json has it
    pub phone: String,
    /// this machine's id in that phone's app, for the notification's tag and link
    pub host: String,
    pub subscription: Subscription,
    pub vapid: Vapid,
    pub done: bool,
}

const FILE: &str = "push.json";

pub fn events_file() -> PathBuf {
    crate::paths::home().join(".inyon").join("events.jsonl")
}

pub fn read_entries() -> Vec<Entry> {
    match read_node_json::<Value>(FILE) {
        Some(Value::Array(all)) => all.into_iter().filter_map(|e| serde_json::from_value(e).ok()).collect(),
        _ => Vec::new(),
    }
}

/// The VAPID keys let anyone notify that browser: readable by this user only.
fn write_entries(all: &[Entry]) -> Result<(), String> {
    write_node_json(FILE, &all)
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// `String(x ?? '')` for the few fields read that way.
fn js_string(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Replaces everything one phone gave, after checking its shape; returns how many it gave.
pub fn set_entries(input: &Value) -> Result<usize, String> {
    let phone = js_string(input.get("phone"));
    let host = js_string(input.get("host"));
    if phone.len() != 64 || !phone.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("push set: bad phone key".into());
    }
    if host.is_empty() || host.encode_utf16().count() > 200 {
        return Err("push set: bad host id".into());
    }
    let regs = input.get("registrations").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut mine = Vec::new();
    for r in &regs {
        let s = r.get("subscription").unwrap_or(&Value::Null);
        let j = r.pointer("/vapid/privateJwk").unwrap_or(&Value::Null);
        let endpoint = text(s, "endpoint").filter(|e| e.starts_with("https://")).ok_or("push set: bad endpoint")?;
        let keys = s.get("keys").unwrap_or(&Value::Null);
        let (Some(p256dh), Some(auth)) = (text(keys, "p256dh"), text(keys, "auth")) else { return Err("push set: bad keys".into()) };
        let (Some("EC"), Some("P-256"), Some(d), Some(x), Some(y)) = (text(j, "kty"), text(j, "crv"), text(j, "d"), text(j, "x"), text(j, "y")) else {
            return Err("push set: bad VAPID key".into());
        };
        let subject = js_string(r.pointer("/vapid/subject"));
        if !(subject.starts_with("mailto:") || subject.starts_with("https://")) {
            return Err("push set: bad VAPID subject".into());
        }
        mine.push(Entry {
            phone: phone.clone(),
            host: host.clone(),
            subscription: Subscription { endpoint: endpoint.into(), keys: Keys { p256dh: p256dh.into(), auth: auth.into() } },
            vapid: Vapid { private_jwk: webpush::EcJwk { kty: "EC".into(), crv: "P-256".into(), d: d.into(), x: x.into(), y: y.into() }, subject },
            done: r.get("done") != Some(&Value::Bool(false)),
        });
    }
    let count = mine.len();
    let _files = node_files()?;
    let mut all: Vec<Entry> = read_entries().into_iter().filter(|e| e.phone != phone).collect();
    all.extend(mine);
    write_entries(&all)?;
    Ok(count)
}

/// A phone that's forgotten gets no more notifications from here.
pub fn forget_pushes(phone: &str) {
    let _files = match node_files() {
        Ok(f) => f,
        Err(e) => return error(&e),
    };
    let all = read_entries();
    let kept: Vec<Entry> = all.iter().filter(|e| e.phone != phone).cloned().collect();
    if kept.len() != all.len() {
        if let Err(e) = write_entries(&kept) {
            error(&e);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Waiting,
    Done,
    Test,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Waiting => "waiting",
            Kind::Done => "done",
            Kind::Test => "test",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: Kind,
    pub pane: String,
    pub title: String,
    pub body: String,
}

/// How a message reaches a push service (a stand-in in tests).
pub type PushSender = Arc<dyn Fn(Subscription, Value, Vapid) -> Pin<Box<dyn Future<Output = Result<Sent, String>> + Send>> + Send + Sync>;

pub fn web_push() -> PushSender {
    Arc::new(|s, p, v| Box::pin(async move { webpush::send_push(&s, &p, &v).await }))
}

/// `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn endpoint_host(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .map(|u| match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

/// Sends a notice to every paired browser that wants it; returns how many push services took it.
pub async fn deliver(n: &Notice, only: Option<&str>, send: &PushSender) -> usize {
    let paired = list_phones();
    let all = read_entries();
    let wanted: Vec<&Entry> =
        all.iter().filter(|e| paired.contains_key(&e.phone) && only.is_none_or(|o| e.subscription.endpoint == o) && (n.kind != Kind::Done || e.done)).collect();
    let sends = wanted.iter().map(|e| {
        let machine = n.pane.is_empty();
        let host = encode_uri_component(&e.host);
        // the same tag and link as the open app, so one replaces the other (apps/web/src/lib/store.ts)
        let tag = if n.kind == Kind::Test {
            "test".to_string()
        } else if machine {
            format!("machine:{}", e.host)
        } else {
            format!("{}:{}", e.host, n.pane)
        };
        let href = if n.kind == Kind::Test {
            "/settings#notifications".to_string()
        } else if machine {
            format!("/machines/{host}")
        } else {
            format!("/m/{host}/p/{}", n.pane.strip_prefix('%').unwrap_or(&n.pane))
        };
        let payload = json!({ "title": n.title, "body": n.body, "kind": n.kind.name(), "tag": tag, "href": href });
        let fut = send(e.subscription.clone(), payload, e.vapid.clone());
        let endpoint = e.subscription.endpoint.clone();
        async move { (endpoint, fut.await) }
    });
    let results = futures_util::future::join_all(sends).await;
    let mut sent = 0;
    let mut gone = HashSet::new();
    for (endpoint, r) in results {
        match r {
            Ok(r) if r.ok => sent += 1,
            Ok(r) if r.gone => {
                gone.insert(endpoint);
            }
            Ok(r) => error(&format!("push: {} answered {}", endpoint_host(&endpoint), r.status)),
            Err(e) => error(&format!("push: {e}")),
        }
    }
    // unsubscribed or expired: the browser subscribes again and the app hands it over. Only then is
    // the lock taken, and off the async threads, since it may wait for another process
    if !gone.is_empty() {
        let _ = tokio::task::spawn_blocking(move || drop_gone(&gone)).await;
    }
    sent
}

/// Drops the subscriptions whose browsers are gone, under the lock; skipped (and said) when the
/// files stay busy, as the next notice finds them gone again.
fn drop_gone(gone: &HashSet<String>) {
    let _files = match node_files() {
        Ok(f) => f,
        Err(e) => return error(&e),
    };
    let kept: Vec<Entry> = read_entries().into_iter().filter(|e| !gone.contains(&e.subscription.endpoint)).collect();
    if let Err(e) = write_entries(&kept) {
        error(&e);
    }
}

/// What a hook event means for a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookNotice {
    pub kind: Kind,
    pub pane: String,
    pub agent: String,
    pub body: String,
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// At most `n` UTF-16 units of `s`, as `String.slice` counts them.
fn js_slice(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// What an agent's hook event means for a notification. Claude Code: PermissionRequest when it asks
/// to run something, Stop when it's done answering. Its Notification event only repeats those
/// (a permission prompt) or nags after a minute idle, so it's left out.
pub fn notice_for(line: &str) -> Option<HookNotice> {
    static SPACE: OnceLock<Regex> = OnceLock::new();
    let ev: Value = serde_json::from_str(line).ok()?;
    let pane = js_string(ev.get("pane"));
    let d = ev.get("data");
    let pane_ok = pane.len() > 1 && pane.starts_with('%') && pane[1..].bytes().all(|b| b.is_ascii_digit());
    if !pane_ok || !truthy(d) {
        return None;
    }
    let d = d?;
    let event = d.get("hook_event_name").and_then(Value::as_str);
    if event == Some("PermissionRequest") {
        let body = match d.get("tool_name") {
            t if truthy(t) => format!("Claude wants to use {}", js_string(t)),
            _ => "Claude needs your input".into(),
        };
        return Some(HookNotice { kind: Kind::Waiting, pane, agent: "Claude".into(), body });
    }
    if event == Some("Stop") && !truthy(d.get("stop_hook_active")) {
        let said = match d.get("last_assistant_message") {
            Some(Value::String(s)) => SPACE.get_or_init(|| Regex::new(r"\s+").unwrap()).replace_all(s, " ").trim().to_string(),
            _ => String::new(),
        };
        let body = if said.is_empty() {
            "Claude finished".to_string()
        } else if said.encode_utf16().count() > 140 {
            format!("{}…", js_slice(&said, 139))
        } else {
            said
        };
        return Some(HookNotice { kind: Kind::Done, pane, agent: "Claude".into(), body });
    }
    None
}

async fn tmux(args: &[&str]) -> Option<(bool, String)> {
    let mut c = tokio::process::Command::new("tmux");
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    crate::ops::hide_window(&mut c);
    let out = tokio::time::timeout(Duration::from_secs(10), c.output()).await.ok()?.ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// The pane's title as the app shows it (Claude's conversation title), else session:window.
pub async fn title_of(pane: String) -> String {
    static MARK: OnceLock<Regex> = OnceLock::new();
    let Some((_, out)) = tmux(&["display-message", "-p", "-t", &pane, "#{pane_title}\t#{session_name}:#{window_index}"]).await else { return pane };
    let out = out.trim();
    let mut parts = out.splitn(2, '\t');
    let (title, place) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    // the same clean-up as the app (registry.ts paneTitle): a leading spinner or mark goes
    let clean = MARK.get_or_init(|| Regex::new(r"^[^A-Za-z0-9_\s]\s+").unwrap()).replace(title, "").trim().to_string();
    if !clean.is_empty() && clean != crate::hostname() {
        clean
    } else if !place.is_empty() {
        place.to_string()
    } else {
        pane
    }
}

/// What the pane's footer lists as still running ("1 shell, 1 agent"), read the way the hub reads
/// it: `Some(None)` for nothing, `None` when it can't tell.
pub async fn background_of(pane: String) -> Option<Option<String>> {
    let (ok, text) = tmux(&["capture-pane", "-p", "-t", &pane]).await?;
    if !ok {
        return None;
    }
    crate::screen::background_on_screen(&text.split('\n').collect::<Vec<_>>())
}

/// Something that names a file across a move: its inode (its file index on Windows).
#[cfg(unix)]
fn identity(m: &std::fs::Metadata, _: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.ino()
}

#[cfg(windows)]
fn identity(_: &std::fs::Metadata, p: &Path) -> u64 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let Ok(f) = std::fs::File::open(p) else { return 0 };
    // SAFETY: a zeroed plain struct the call fills in, for a handle that stays open meanwhile
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) == 0 {
            return 0;
        }
        ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64
    }
}

fn stat(p: &Path) -> (u64, u64) {
    match std::fs::metadata(p) {
        Ok(m) => (m.len(), identity(&m, p)),
        Err(_) => (0, 0),
    }
}

/// Follows the hook log from its current end. It's checked once a second; the hook moves it aside
/// past 2 MB (see the hub's claude/integration.ts), which shows here as a new file (another inode,
/// or none for a moment), read from its top. Aborting the task stops it.
pub fn follow_events(mut on_line: impl FnMut(String) + Send + 'static, events: PathBuf, every: Duration) -> tokio::task::JoinHandle<()> {
    // where the log ends now, not when the task first runs
    let (mut pos, mut ino) = stat(&events);
    tokio::spawn(async move {
        let mut rest: Vec<u8> = Vec::new();
        let mut tick = tokio::time::interval(every);
        tick.tick().await;
        loop {
            tick.tick().await;
            let (size, now_ino) = stat(&events);
            if now_ino != ino || size < pos {
                pos = 0;
                rest.clear();
                ino = now_ino;
            }
            if size == pos {
                continue;
            }
            // moved aside or busy between the look and the read: the next tick starts over
            let Ok(mut f) = std::fs::File::open(&events) else { continue };
            let mut buf = vec![0u8; (size - pos).min(1 << 20) as usize];
            if f.seek(SeekFrom::Start(pos)).is_err() {
                continue;
            }
            let Ok(n) = f.read(&mut buf) else { continue };
            pos += n as u64;
            rest.extend_from_slice(&buf[..n]);
            while let Some(at) = rest.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = rest.drain(..=at).collect();
                let l = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                if !l.trim().is_empty() {
                    on_line(l);
                }
            }
        }
    })
}

type Look = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Option<Option<String>>> + Send>> + Send + Sync>;
type Title = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync>;

/// What `start_push` uses; tests swap in stand-ins.
pub struct PushOptions {
    pub events: PathBuf,
    pub every: Duration,
    pub check: Duration,
    pub background: Look,
    pub title: Title,
    pub send: PushSender,
}

impl Default for PushOptions {
    fn default() -> Self {
        PushOptions {
            events: events_file(),
            every: Duration::from_secs(1),
            check: Duration::from_secs(5),
            background: Arc::new(|p| Box::pin(background_of(p))),
            title: Arc::new(|p| Box::pin(title_of(p))),
            send: web_push(),
        }
    }
}

/// The link service's part: notify paired browsers when an agent needs you or finishes. A turn
/// that ends with agents or shells still running isn't finished: Claude's Stop fires all the
/// same, so the finish waits until the pane's footer lists nothing (checked every few seconds,
/// for half an hour at most).
pub fn start_push(o: PushOptions) -> tokio::task::JoinHandle<()> {
    // one notice per pane and kind every few seconds: a hook can fire twice for one prompt
    let mut recent: HashMap<String, u64> = HashMap::new();
    // the finish on hold per pane; a newer event for the pane takes over
    let held: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut tokens: u64 = 0;
    let PushOptions { events, every, check, background, title, send } = o;
    let notify = move |n: HookNotice| {
        let (title, send) = (title.clone(), send.clone());
        tokio::spawn(async move {
            let t = title(n.pane.clone()).await;
            deliver(&Notice { kind: n.kind, pane: n.pane, title: t, body: n.body }, None, &send).await;
        });
    };
    follow_events(
        move |line| {
            let Some(n) = notice_for(&line) else { return };
            if read_entries().is_empty() {
                return;
            }
            let key = format!("{} {}", n.pane, n.kind.name());
            let now = crate::now_ms();
            if now.saturating_sub(recent.get(&key).copied().unwrap_or(0)) < 5000 {
                return;
            }
            recent.insert(key, now);
            // a question while a finish waits: the turn went on, so that finish is no longer news
            held.lock().unwrap_or_else(|e| e.into_inner()).remove(&n.pane);
            if n.kind != Kind::Done {
                return notify(n);
            }
            tokens += 1;
            let token = tokens;
            held.lock().unwrap_or_else(|e| e.into_inner()).insert(n.pane.clone(), token);
            let (held, background, notify) = (held.clone(), background.clone(), notify.clone());
            tokio::spawn(async move {
                let mine = || held.lock().unwrap_or_else(|e| e.into_inner()).get(&n.pane) == Some(&token);
                let started = crate::now_ms();
                while background(n.pane.clone()).await.flatten().is_some() {
                    if crate::now_ms() - started > 30 * 60_000 {
                        break;
                    }
                    tokio::time::sleep(check).await;
                    if !mine() {
                        return;
                    }
                }
                if !mine() {
                    return;
                }
                held.lock().unwrap_or_else(|e| e.into_inner()).remove(&n.pane);
                notify(n);
            });
        },
        events,
        every,
    )
}

fn stdin_json() -> Value {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    serde_json::from_str(&s).unwrap_or_else(|e| {
        error(&format!("error: {e}"));
        std::process::exit(1)
    })
}

/// `inyon push set|test|list`: what the app runs here over the link, and a look for you.
pub async fn push_command(args: &[String]) {
    match args.first().map(String::as_str) {
        Some("set") => match set_entries(&stdin_json()) {
            Ok(n) => log(&n.to_string()),
            Err(e) => {
                error(&format!("error: {e}"));
                std::process::exit(1);
            }
        },
        Some("test") => {
            let endpoint = js_string(stdin_json().get("endpoint"));
            let n =
                Notice { kind: Kind::Test, pane: String::new(), title: "Inyon".into(), body: format!("Notifications from {} work, even with Inyon closed.", crate::hostname()) };
            log(&deliver(&n, Some(&endpoint), &web_push()).await.to_string());
        }
        Some("list") | None => {
            let phones = list_phones();
            let all = read_entries();
            let browsers: Vec<Value> = all
                .iter()
                .map(|e| {
                    let mut b = json!({ "phone": e.phone });
                    if let Some(p) = phones.get(&e.phone) {
                        b["name"] = json!(p.name);
                    }
                    b["host"] = json!(e.host);
                    b["endpoint"] = json!(endpoint_host(&e.subscription.endpoint));
                    b["done"] = json!(e.done);
                    b
                })
                .collect();
            if all.is_empty() {
                return emit(json!({ "browsers": browsers }), Some("No browser gets notifications from this machine yet."));
            }
            let lines: Vec<String> = all
                .iter()
                .map(|e| {
                    let name = phones.get(&e.phone).map(|p| p.name.clone()).unwrap_or_else(|| "forgotten phone".into());
                    format!("{} {}{}", pad_end(&name, 24), endpoint_host(&e.subscription.endpoint), if e.done { "" } else { " (needs-you only)" })
                })
                .collect();
            emit(json!({ "browsers": browsers }), Some(&lines.join("\n")));
        }
        _ => {
            error("usage: inyon push [list|set|test]");
            std::process::exit(2);
        }
    }
}

/// `String.padEnd`, counting UTF-16 units as JavaScript does.
pub fn pad_end(s: &str, n: usize) -> String {
    let len = s.encode_utf16().count();
    format!("{s}{}", " ".repeat(n.saturating_sub(len)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::testing;
    use std::fs;

    fn phone(c: char) -> String {
        c.to_string().repeat(64)
    }

    // the browser key is the public one from RFC 8291's example, as the Bun engine's push tests had it
    fn reg(n: u32, done: bool) -> Value {
        json!({
            "subscription": { "endpoint": format!("https://push.example.net/{n}"), "keys": { "p256dh": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4", "auth": "BTBZMqHH6r4Tts7J_aSIgg" } },
            "vapid": { "privateJwk": { "kty": "EC", "crv": "P-256", "d": "d", "x": "x", "y": "y" }, "subject": "https://example.com/" },
            "done": done,
        })
    }

    fn line(pane: &str, data: Value) -> String {
        json!({ "pane": pane, "ts": 1, "data": data }).to_string()
    }

    fn phones(list: &[(char, &str)]) {
        let mut m = serde_json::Map::new();
        for (c, name) in list {
            m.insert(phone(*c), json!({ "name": name, "pairedAt": 1 }));
        }
        fs::create_dir_all(crate::paths::node_dir()).unwrap();
        fs::write(crate::paths::node_dir().join("phones.json"), Value::Object(m).to_string()).unwrap();
    }

    type Log = Arc<Mutex<Vec<(String, Value)>>>;

    fn recorder(gone_suffix: Option<&'static str>) -> (PushSender, Log) {
        let sent: Log = Arc::new(Mutex::new(Vec::new()));
        let s = sent.clone();
        let send: PushSender = Arc::new(move |sub, payload, _| {
            s.lock().unwrap().push((sub.endpoint.clone(), payload));
            let gone = gone_suffix.is_some_and(|g| sub.endpoint.ends_with(g));
            Box::pin(async move { Ok(Sent { ok: !gone, status: if gone { 410 } else { 201 }, gone }) })
        });
        (send, sent)
    }

    #[test]
    fn claude_asking_to_run_something_needs_you_and_finishing_is_done() {
        let n = |data| notice_for(&line("%3", data));
        assert_eq!(
            n(json!({ "hook_event_name": "PermissionRequest", "tool_name": "Bash" })),
            Some(HookNotice { kind: Kind::Waiting, pane: "%3".into(), agent: "Claude".into(), body: "Claude wants to use Bash".into() })
        );
        assert_eq!(n(json!({ "hook_event_name": "Stop" })), Some(HookNotice { kind: Kind::Done, pane: "%3".into(), agent: "Claude".into(), body: "Claude finished".into() }));
        assert_eq!(n(json!({ "hook_event_name": "Stop", "last_assistant_message": "Fixed the  test.\nAll green." })).unwrap().body, "Fixed the test. All green.");
        let long = "é".repeat(200);
        assert_eq!(n(json!({ "hook_event_name": "Stop", "last_assistant_message": long })).unwrap().body, format!("{}…", "é".repeat(139)));
    }

    #[test]
    fn reminders_a_stop_hook_running_again_other_panes_and_junk_are_no_notice() {
        assert_eq!(notice_for(&line("%3", json!({ "hook_event_name": "Notification", "message": "Claude is waiting for your input" }))), None);
        assert_eq!(notice_for(&line("%3", json!({ "hook_event_name": "Stop", "stop_hook_active": true }))), None);
        assert_eq!(notice_for(&line("", json!({ "hook_event_name": "Stop" }))), None);
        assert_eq!(notice_for("not json"), None);
    }

    #[tokio::test]
    async fn set_deliver_and_forget() {
        let _h = testing::home("push-set");
        // set replaces only what that phone gave, and checks it
        phones(&[('a', "Pixel"), ('b', "iPhone")]);
        set_entries(&json!({ "phone": phone('a'), "host": "laptop", "registrations": [reg(1, true)] })).unwrap();
        set_entries(&json!({ "phone": phone('b'), "host": "my-laptop", "registrations": [reg(2, false)] })).unwrap();
        set_entries(&json!({ "phone": phone('a'), "host": "laptop", "registrations": [reg(3, true)] })).unwrap();
        let seen: Vec<(String, String, String)> =
            read_entries().iter().map(|e| (e.phone[..1].to_string(), e.host.clone(), e.subscription.endpoint[e.subscription.endpoint.len() - 1..].to_string())).collect();
        assert_eq!(seen, [("b".into(), "my-laptop".into(), "2".into()), ("a".into(), "laptop".into(), "3".into())]);
        assert_eq!(set_entries(&json!({ "phone": "nope", "host": "x", "registrations": [] })).unwrap_err(), "push set: bad phone key");
        let mut bad = reg(4, true);
        bad["subscription"] = json!({ "endpoint": "http://insecure", "keys": {} });
        assert_eq!(set_entries(&json!({ "phone": phone('a'), "host": "x", "registrations": [bad] })).unwrap_err(), "push set: bad endpoint");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(crate::paths::node_dir().join("push.json")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // the file reads as the Bun engine wrote it
        let text = fs::read_to_string(crate::paths::node_dir().join("push.json")).unwrap();
        assert!(text.starts_with("[\n  {\n    \"phone\": \"bbbb"), "{text}");
        assert!(text.contains("\"privateJwk\": {\n        \"kty\": \"EC\""), "{text}");

        // each browser gets its own tag and link; finished notices only where wanted; gone ones are dropped
        let (send, sent) = recorder(Some("/3"));
        let n = |kind, pane: &str| Notice { kind, pane: pane.into(), title: "Fix the login".into(), body: "b".into() };
        assert_eq!(deliver(&n(Kind::Waiting, "%7"), None, &send).await, 1);
        let got: Vec<(String, Value, Value)> = sent.lock().unwrap().iter().map(|(e, p)| (e[e.len() - 1..].to_string(), p["tag"].clone(), p["href"].clone())).collect();
        assert_eq!(got, [("2".into(), json!("my-laptop:%7"), json!("/m/my-laptop/p/7")), ("3".into(), json!("laptop:%7"), json!("/m/laptop/p/7"))]);
        // the browser behind /3 unsubscribed
        assert_eq!(read_entries().iter().map(|e| e.subscription.endpoint.clone()).collect::<Vec<_>>(), ["https://push.example.net/2"]);
        sent.lock().unwrap().clear();
        // a notice about the machine itself opens its page, as the hub's do
        assert_eq!(deliver(&n(Kind::Waiting, ""), None, &send).await, 1);
        let got: Vec<(Value, Value)> = sent.lock().unwrap().iter().map(|(_, p)| (p["tag"].clone(), p["href"].clone())).collect();
        assert_eq!(got, [(json!("machine:my-laptop"), json!("/machines/my-laptop"))]);
        sent.lock().unwrap().clear();
        // the iPhone asked for needs-you only
        assert_eq!(deliver(&n(Kind::Done, "%7"), None, &send).await, 0);
        assert!(sent.lock().unwrap().is_empty());

        // a forgotten phone gets nothing, and forget removes what it gave
        phones(&[('a', "Pixel")]);
        assert_eq!(deliver(&n(Kind::Waiting, "%1"), None, &send).await, 0);
        forget_pushes(&phone('b'));
        assert!(read_entries().is_empty());
    }

    #[tokio::test]
    async fn a_notice_with_no_browser_gone_never_waits_for_the_files() {
        let _h = testing::home("push-no-lock");
        phones(&[('a', "Pixel")]);
        set_entries(&json!({ "phone": phone('a'), "host": "laptop", "registrations": [reg(1, true)] })).unwrap();
        // another writer holds the files for as long as the notice takes
        let _held = node_files().unwrap();
        let (send, _sent) = recorder(None);
        let t = std::time::Instant::now();
        let n = Notice { kind: Kind::Waiting, pane: "%7".into(), title: "t".into(), body: "b".into() };
        assert_eq!(deliver(&n, None, &send).await, 1);
        assert!(t.elapsed() < Duration::from_secs(1), "waited {:?}", t.elapsed());
    }

    #[tokio::test]
    async fn a_finish_with_agents_still_running_waits_for_them_and_a_question_meanwhile_takes_over() {
        let h = testing::home("push-hold");
        let log = h.dir.join("events-bg.jsonl");
        fs::write(&log, "").unwrap();
        phones(&[('a', "Pixel")]);
        set_entries(&json!({ "phone": phone('a'), "host": "laptop", "registrations": [reg(1, true)] })).unwrap();
        let (send, sent) = recorder(None);
        let footer: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some("1 agent".into())));
        let f = footer.clone();
        let stop = start_push(PushOptions {
            events: log.clone(),
            every: Duration::from_millis(20),
            check: Duration::from_millis(30),
            background: Arc::new(move |_| {
                let v = f.lock().unwrap().clone();
                Box::pin(async move { Some(v) })
            }),
            title: Arc::new(|p| Box::pin(async move { p })),
            send,
        });
        let append = |l: String| {
            use std::io::Write;
            writeln!(fs::OpenOptions::new().append(true).open(&log).unwrap(), "{l}").unwrap();
        };
        let wait = |ms| tokio::time::sleep(Duration::from_millis(ms));
        let kinds = || sent.lock().unwrap().iter().map(|(_, p)| format!("{} {}", p["kind"].as_str().unwrap(), p["tag"].as_str().unwrap())).collect::<Vec<_>>();
        let until = |n: usize| {
            let s = sent.clone();
            async move {
                for _ in 0..300 {
                    if s.lock().unwrap().len() >= n {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        };
        // the main turn ends while a subagent runs: not finished yet
        append(line("%5", json!({ "hook_event_name": "Stop" })));
        wait(150).await;
        assert!(kinds().is_empty());
        *footer.lock().unwrap() = None;
        until(1).await;
        assert_eq!(kinds(), ["done laptop:%5"]);
        // another turn ends with an agent running, then Claude asks something: the question is the news
        *footer.lock().unwrap() = Some("1 agent".into());
        append(line("%6", json!({ "hook_event_name": "Stop" })));
        wait(80).await;
        append(line("%6", json!({ "hook_event_name": "PermissionRequest", "tool_name": "Bash" })));
        wait(80).await;
        *footer.lock().unwrap() = None;
        until(2).await;
        // and the held finish for %6 never follows
        wait(150).await;
        assert_eq!(kinds(), ["done laptop:%5", "waiting laptop:%6"]);
        stop.abort();
    }

    #[tokio::test]
    async fn follows_the_hook_log_from_its_end_across_the_2_mb_move() {
        let dir = std::env::temp_dir().join(format!("inyon-follow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("events.jsonl");
        let old = dir.join("events.jsonl.old");
        fs::write(&log, "old line\n").unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = seen.clone();
        let stop = follow_events(move |l| s.lock().unwrap().push(l), log.clone(), Duration::from_millis(20));
        let tick = || tokio::time::sleep(Duration::from_millis(60));
        let append = |t: &str| {
            use std::io::Write;
            fs::OpenOptions::new().append(true).open(&log).unwrap().write_all(t.as_bytes()).unwrap();
        };
        tick().await;
        append("one\ntw");
        tick().await;
        append("o\n");
        tick().await;
        fs::rename(&log, &old).unwrap();
        fs::write(&log, "three\n").unwrap();
        tick().await;
        // moved aside with nothing in its place for a while: no crash, and the next file is picked up
        fs::rename(&log, &old).unwrap();
        tick().await;
        tick().await;
        fs::write(&log, "four\n").unwrap();
        tick().await;
        // the new file is already longer than the old one was read to: still from its top
        fs::rename(&log, &old).unwrap();
        fs::write(&log, format!("{}\nfive\n", "x".repeat(200))).unwrap();
        tick().await;
        stop.abort();
        assert_eq!(*seen.lock().unwrap(), ["one", "two", "three", "four", &"x".repeat(200), "five"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_escape_the_host_as_the_app_does() {
        assert_eq!(encode_uri_component("my laptop/é"), "my%20laptop%2F%C3%A9");
        assert_eq!(encode_uri_component("a-b_c.d!~*'()"), "a-b_c.d!~*'()");
    }
}
