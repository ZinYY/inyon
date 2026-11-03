//! What a hub or a phone can ask a machine to do: run a shell command, stream one's output line
//! by line, or run one in a terminal. Each connection (the hub's socket, or one phone session)
//! gets its own set, so closing it ends everything it started. (ops.ts)
//!
//!   hub/phone → machine   { id, op: 'exec' | 'stream' | 'pty' | 'write' | 'resize' | 'kill', … }
//!   machine → hub/phone   { id, type: 'exit' | 'line' | 'data' | 'error', … }

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::paths::{self, bin_dir, env_var, node_dir};
use crate::WIN;

/// Sourced before every command on Windows (busybox-w32 sh): psmux ids and what busybox lacks.
pub const WIN_PRELUDE: &str = include_str!("win-prelude.sh");

/// How long output may stay open after the shell exited (a child that escaped its group can hold it).
const OUTPUT_GRACE: Duration = Duration::from_millis(1000);

/// Where a frame goes: the hub's socket or a phone's channel.
pub type Sender = Arc<dyn Fn(Value) + Send + Sync>;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run in the background on Windows there is no console, so a console program started from here
/// (psmux, PowerShell, the shell) would open a window of its own. Every start hides it.
pub fn hide_window(c: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// `hide_window` for a std Command.
pub fn hide_window_std(c: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// For a child whose output goes to this terminal: it shares this program's console when there is
/// one to see, and gets a hidden one when this runs without (the windowless copy, the app).
pub fn console_child_std(c: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    // SAFETY: takes nothing and only reads this process's console
    if unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null() {
        hide_window_std(c);
    }
    c
}

/// Starts a command in a session (and process group) of its own, as Node's `detached` does, so
/// ending it ends everything it started.
fn own_session(c: &mut tokio::process::Command) {
    #[cfg(unix)]
    // SAFETY: setsid is async-signal-safe and touches nothing but the child
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = c;
}

/// busybox-w32, the small sh Windows runs Inyon's commands with (pinned, as the hub's installer).
pub const BUSYBOX: [(&str, &str, &str); 2] = [
    ("x64", "busybox-w64u-FRP-6075-g169694ebd.exe", "6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c"),
    ("arm64", "busybox-w64a-FRP-6075-g169694ebd.exe", "e67f873d19d58c535cc9f0c4965ffd622e19b7bab87e3da89cb2185fb54464d7"),
];

/// A program on PATH, as `Bun.which` finds it.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> =
        if WIN { std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into()).split(';').map(|e| e.to_lowercase()).collect() } else { vec![String::new()] };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if std::fs::metadata(&p).map(|m| m.permissions().mode() & 0o111 == 0).unwrap_or(true) {
                        continue;
                    }
                }
                return Some(p);
            }
        }
    }
    None
}

/// The lowercase hex sha256 of some bytes.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// On Windows, finds busybox (or fetches it once) and points INYON_SH at it.
pub fn ensure_shell() -> Result<(), String> {
    if !WIN || env_var("INYON_SH").is_some_and(|s| PathBuf::from(s).exists()) {
        return Ok(());
    }
    let local = bin_dir().join("busybox.exe");
    // install.ps1 put it in the node's folder before the rename
    let found = which("busybox").or_else(|| [local.clone(), node_dir().join("bin").join("busybox.exe")].into_iter().find(|f| f.exists()));
    if let Some(f) = found {
        paths::set_env("INYON_SH", &f.to_string_lossy());
        return Ok(());
    }
    let (_, file, sum) = BUSYBOX.iter().find(|(a, _, _)| *a == if crate::arch() == "arm64" { "arm64" } else { "x64" }).copied().unwrap_or(BUSYBOX[0]);
    crate::output::say("downloading busybox-w32 (the shell Inyon runs commands with)…");
    let res = crate::http::download(&format!("https://frippery.org/files/busybox/{file}"))?;
    if !res.ok() {
        return Err(format!("couldn't download busybox-w32: {}", res.status));
    }
    if sha256_hex(&res.body) != sum {
        return Err("busybox-w32 didn't match its pinned checksum".into());
    }
    std::fs::create_dir_all(bin_dir()).map_err(|e| e.to_string())?;
    let tmp = PathBuf::from(format!("{}.tmp", local.to_string_lossy()));
    std::fs::write(&tmp, &res.body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &local).map_err(|e| e.to_string())?;
    paths::set_env("INYON_SH", &local.to_string_lossy());
    Ok(())
}

fn shell_for(cmd: &str) -> Vec<String> {
    if WIN {
        vec![env_var("INYON_SH").unwrap_or_else(|| "busybox.exe".into()), "sh".into(), "-c".into(), format!("{WIN_PRELUDE}\n{cmd}")]
    } else {
        vec!["/bin/sh".into(), "-c".into(), cmd.into()]
    }
}

/// How a command can run this program, as in "$INYON_EXE" ${INYON_SCRIPT:+"$INYON_SCRIPT"}
/// update: the inyon command. Hubs and apps from before the rename ask with TETHER_NODE_*.
fn self_exe() -> String {
    paths::inyon_command().or_else(|| std::env::current_exe().ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| paths::NAME.into())
}

/// The environment commands run with. On Windows: a Unix-style HOME, the usual per-user install
/// folders on PATH, and no SHELL (psmux would start new panes in busybox instead of PowerShell).
/// Names and values are passed on as they are, so a variable that isn't valid UTF-8 reaches the
/// command unchanged rather than ending this process.
pub fn child_env() -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    // a TETHER_* variable set before the rename counts as its INYON_* name
    let olds: Vec<(OsString, OsString)> = env.iter().filter_map(|(k, v)| k.to_str()?.strip_prefix("TETHER_").map(|r| (OsString::from(format!("INYON_{r}")), v.clone()))).collect();
    for (k, v) in olds {
        if !env.iter().any(|(n, _)| *n == k) {
            env.push((k, v));
        }
    }
    let same = |n: &OsStr, k: &str| if WIN { n.to_str().is_some_and(|n| n.eq_ignore_ascii_case(k)) } else { n == k };
    let mut set = |k: &str, v: OsString| {
        env.retain(|(n, _)| !same(n, k));
        env.push((k.into(), v));
    };
    for (k, v) in paths::set_vars() {
        set(&k, v.into());
    }
    let exe = self_exe();
    set("INYON_EXE", exe.clone().into());
    set("INYON_SCRIPT", OsString::new());
    set("TETHER_NODE_EXE", exe.into());
    set("TETHER_NODE_SCRIPT", OsString::new());
    if WIN {
        let home = paths::home();
        set("HOME", home.to_string_lossy().replace('\\', "/").into());
        env.retain(|(n, _)| !same(n, "SHELL"));
        let mut extra: Vec<OsString> = [home.join(".local").join("bin"), home.join(".bun").join("bin"), home.join("scoop").join("shims"), home.join(".cargo").join("bin")]
            .into_iter()
            .map(PathBuf::into_os_string)
            .collect();
        if let Some(a) = std::env::var_os("APPDATA") {
            extra.push(PathBuf::from(a).join("npm").into_os_string());
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            extra.push(PathBuf::from(l).join("Microsoft").join("WinGet").join("Links").into_os_string());
        }
        let key = env.iter().find(|(n, _)| same(n, "path")).map(|(n, _)| n.clone()).unwrap_or_else(|| "Path".into());
        let current = env.iter().find(|(n, _)| *n == key).map(|(_, v)| v.clone()).unwrap_or_default();
        extra.push(current);
        let mut joined = OsString::new();
        for (i, part) in extra.iter().enumerate() {
            if i > 0 {
                joined.push(";");
            }
            joined.push(part);
        }
        env.retain(|(n, _)| *n != key);
        env.push((key, joined));
    }
    env
}

/// PATH the commands run with, which also covers the usual per-user install folders.
pub fn child_path() -> String {
    child_env().into_iter().find(|(k, _)| k.to_str().is_some_and(|k| k.eq_ignore_ascii_case("path"))).map(|(_, v)| v.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Ends a command and everything it started. Commands run in sessions of their own, so on Unix
/// the whole group goes: killing sh alone leaves a pipeline's other commands running and holding
/// its output open. On Windows, taskkill /T does the same for the process tree.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    // SAFETY: plain signals to a process group, then to the process itself
    unsafe {
        if libc::kill(-(pid as i32), libc::SIGTERM) != 0 {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        hide_window_std(&mut c);
        let _ = c.spawn();
    }
}

/// Where keys for a terminal go. On Unix not portable-pty's own writer: when that one is dropped it
/// types a newline and Ctrl-D, which a tmux client still on its way out passes to the pane as Enter.
fn terminal_input(master: &dyn portable_pty::MasterPty) -> std::io::Result<Box<dyn Write + Send>> {
    #[cfg(unix)]
    {
        use std::os::fd::BorrowedFd;
        let fd = master.as_raw_fd().ok_or_else(|| std::io::Error::other("no terminal"))?;
        // SAFETY: the master is open for this whole call, and the copy is a descriptor of its own
        let own = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
        Ok(Box::new(std::fs::File::from(own)))
    }
    #[cfg(not(unix))]
    master.take_writer().map_err(std::io::Error::other)
}

/// An exit code as Bun reported it: 128 plus the signal for a process a signal ended.
fn exit_code(s: &std::process::ExitStatus) -> i32 {
    if let Some(c) = s.code() {
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return 128 + sig;
        }
    }
    1
}

fn lf(s: String) -> String {
    if WIN {
        s.replace("\r\n", "\n")
    } else {
        s
    }
}

/// psmux's control mode numbers panes per session; tell the hub which session they belong to.
/// The hub's live attach puts options between `tmux` and `-C`, so anything goes there.
pub fn win_line(cmd: &str, win: bool) -> Option<impl Fn(&str) -> String> {
    static ATTACH: OnceLock<Regex> = OnceLock::new();
    static OUTPUT: OnceLock<Regex> = OnceLock::new();
    if !win {
        return None;
    }
    let sid = ATTACH.get_or_init(|| Regex::new(r"\btmux\b.*?-C attach-session -t '?\$(\d+)").unwrap()).captures(cmd)?.get(1)?.as_str().to_string();
    let re = OUTPUT.get_or_init(|| Regex::new(r"^(%(?:extended-)?output) %(\d+) ").unwrap());
    Some(move |l: &str| re.replace(l, |c: &regex::Captures| format!("{} %{sid}.{} ", &c[1], &c[2])).into_owned())
}

// ---------- the Windows process list ----------

/// Windows process list (pid ppid age name args, like `ps -e -o pid=,ppid=,etimes=,comm=,args=`)
/// from one PowerShell kept running, since starting one per probe costs about a second of CPU.
const PS_QUERY: &str = "$n=Get-Date; Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,CreationDate,Name,CommandLine | % { $c=$_.CommandLine; if (-not $c) { $c=$_.Name }; $a=0; if ($_.CreationDate) { $a=[int]($n-$_.CreationDate).TotalSeconds }; '{0} {1} {2} {3} {4}' -f $_.ProcessId,$_.ParentProcessId,$a,($_.Name -replace '\\.exe$',''),($c -replace '[\\r\\n]+',' ') }; '@@DONE'";

struct PsHelper {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    out: Arc<Mutex<String>>,
    wake: Arc<tokio::sync::Notify>,
}

static PS_HELPER: tokio::sync::Mutex<Option<PsHelper>> = tokio::sync::Mutex::const_new(None);

async fn win_procs() -> String {
    let mut guard = PS_HELPER.lock().await;
    let dead = match guard.as_mut() {
        Some(h) => !matches!(h.child.try_wait(), Ok(None)),
        None => true,
    };
    if dead {
        let mut c = tokio::process::Command::new("powershell.exe");
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).env_clear().envs(child_env());
        hide_window(&mut c);
        let Ok(mut child) = c.spawn() else { return String::new() };
        let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else { return String::new() };
        let out = Arc::new(Mutex::new(String::new()));
        let wake = Arc::new(tokio::sync::Notify::new());
        let (o, w) = (out.clone(), wake.clone());
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16384];
            let mut pending: Vec<u8> = Vec::new();
            while let Ok(n) = stdout.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                pending.extend_from_slice(&buf[..n]);
                // whole characters only: the rest waits for the next read
                let cut = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => pending.len(),
                };
                let text = String::from_utf8_lossy(&pending[..cut]).into_owned();
                pending.drain(..cut);
                o.lock().unwrap_or_else(|e| e.into_inner()).push_str(&text);
                w.notify_one();
            }
        });
        let _ = stdin.write_all(b"[Console]::OutputEncoding=[Text.Encoding]::UTF8\n").await;
        *guard = Some(PsHelper { child, stdin, out, wake });
    }
    let Some(h) = guard.as_mut() else { return String::new() };
    h.out.lock().unwrap_or_else(|e| e.into_inner()).clear();
    let _ = h.stdin.write_all(format!("{PS_QUERY}\n").as_bytes()).await;
    let _ = h.stdin.flush().await;
    let done = Regex::new(r"(?m)^@@DONE\r?$").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let text = h.out.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if done.is_match(&text) {
            let text = lf(text);
            let first = text.split("\n@@DONE").next().unwrap_or("").to_string();
            return if first.starts_with("@@DONE") { String::new() } else { first };
        }
        if tokio::time::Instant::now() > deadline || !matches!(h.child.try_wait(), Ok(None)) {
            if let Some(pid) = h.child.id() {
                kill_tree(pid);
            }
            *guard = None;
            return String::new();
        }
        let _ = tokio::time::timeout(Duration::from_millis(250), h.wake.notified()).await;
    }
}

// ---------- the operations ----------

enum Handle {
    /// exec and stream: the process (its group), and a stream's stdin held open
    Process {
        pid: Option<u32>,
        _stdin: Option<tokio::process::ChildStdin>,
    },
    Pty(Arc<Pty>),
}

struct Pty {
    pid: Option<u32>,
    /// keys and pastes, written in order by a thread of the terminal's own: a terminal that isn't
    /// reading would otherwise hold up everything else the connection asks for
    input: std::sync::mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

impl Handle {
    fn kill(&self) {
        match self {
            Handle::Process { pid: Some(pid), .. } => kill_tree(*pid),
            Handle::Process { pid: None, .. } => {}
            Handle::Pty(p) => {
                if let Some(pid) = p.pid {
                    kill_tree(pid);
                }
                let _ = p.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
            }
        }
    }
}

/// What one connection has running, and whether it has closed: a command that starts after
/// the connection closed is ended at once rather than left running with nothing to stop it.
#[derive(Default)]
struct Jobs {
    map: HashMap<String, Handle>,
    closed: bool,
}

type Table = Mutex<Jobs>;

fn jobs(running: &Table) -> std::sync::MutexGuard<'_, Jobs> {
    running.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps `h` under `id` so a kill or the connection closing can end it. False (and `h` ended)
/// when the connection closed first.
fn register(running: &Table, id: &Value, h: Handle) -> bool {
    let mut j = jobs(running);
    if j.closed {
        drop(j);
        h.kill();
        return false;
    }
    j.map.insert(key(id), h);
    true
}
type Running = Arc<Table>;

/// Commands run in sessions of their own, so they would outlive inyon: every connection's are
/// ended as it exits.
static EVERY: Mutex<Vec<Weak<Table>>> = Mutex::new(Vec::new());

/// Ends everything every connection started (this process is about to exit).
pub fn kill_everything() {
    let all: Vec<Running> = EVERY.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect();
    for running in all {
        for h in jobs(&running).map.values() {
            h.kill();
        }
    }
}

/// One connection's commands.
pub struct Ops {
    running: Running,
    send: Sender,
}

fn key(id: &Value) -> String {
    id.to_string()
}

fn text(m: &Value, k: &str) -> String {
    m.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

impl Ops {
    pub fn new(send: Sender) -> Ops {
        let running: Running = Arc::new(Mutex::new(Jobs::default()));
        let mut every = EVERY.lock().unwrap_or_else(|e| e.into_inner());
        every.retain(|w| w.strong_count() > 0);
        every.push(Arc::downgrade(&running));
        Ops { running, send }
    }

    /// One message from the hub or the phone.
    pub fn message(&self, m: Value) {
        let id = m.get("id").cloned().unwrap_or(Value::Null);
        match m.get("op").and_then(Value::as_str).unwrap_or("") {
            "exec" => {
                let (running, send) = (self.running.clone(), self.send.clone());
                tokio::spawn(async move { exec(running, send, id, m).await });
            }
            "stream" => self.stream(id, &m),
            "pty" => self.pty(id, &m),
            "write" => {
                if let Some(p) = self.pty_of(&id) {
                    let _ = p.input.send(text(&m, "data").into_bytes());
                }
            }
            "resize" => {
                if let Some(p) = self.pty_of(&id) {
                    let size = |k: &str| m.get(k).and_then(Value::as_u64).unwrap_or(0) as u16;
                    let _ = p.master.lock().unwrap_or_else(|e| e.into_inner()).resize(portable_pty::PtySize {
                        rows: size("rows"),
                        cols: size("cols"),
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
            "kill" => {
                let h = jobs(&self.running).map.remove(&key(&id));
                if let Some(h) = h {
                    h.kill();
                }
            }
            _ => {}
        }
    }

    fn pty_of(&self, id: &Value) -> Option<Arc<Pty>> {
        match jobs(&self.running).map.get(&key(id)) {
            Some(Handle::Pty(p)) => Some(p.clone()),
            _ => None,
        }
    }

    fn stream(&self, id: Value, m: &Value) {
        let cmd = text(m, "cmd");
        let shell = shell_for(&cmd);
        let mut c = tokio::process::Command::new(&shell[0]);
        c.args(&shell[1..])
            .stdin(if m.get("keepStdin").is_some_and(truthy) { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear()
            .envs(child_env());
        hide_window(&mut c);
        if !WIN {
            own_session(&mut c);
        }
        let mut child = match c.spawn() {
            Ok(ch) => ch,
            Err(e) => return (self.send)(json!({ "id": id, "type": "error", "error": e.to_string() })),
        };
        let stdout = child.stdout.take();
        if !register(&self.running, &id, Handle::Process { pid: child.id(), _stdin: child.stdin.take() }) {
            return;
        }
        let (running, send) = (self.running.clone(), self.send.clone());
        let map = win_line(&cmd, WIN).map(|f| Box::new(f) as Box<dyn Fn(&str) -> String + Send + Sync>);
        let line_send = send.clone();
        let line_id = id.clone();
        let pumped = tokio::spawn(async move {
            let Some(mut out) = stdout else { return };
            let emit = |raw: &[u8]| {
                let mut l = String::from_utf8_lossy(raw).into_owned();
                if WIN && l.ends_with('\r') {
                    l.pop();
                }
                if !l.is_empty() {
                    let data = match &map {
                        Some(f) => f(&l),
                        None => l,
                    };
                    line_send(json!({ "id": line_id, "type": "line", "data": data }));
                }
            };
            let mut carry: Vec<u8> = Vec::new();
            let mut buf = vec![0u8; 16384];
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                carry.extend_from_slice(&buf[..n]);
                while let Some(at) = carry.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = carry.drain(..=at).collect();
                    emit(&line[..line.len() - 1]);
                }
            }
            if !carry.is_empty() {
                emit(&carry);
            }
        });
        tokio::spawn(async move {
            let code = child.wait().await.map(|s| exit_code(&s)).unwrap_or(1);
            jobs(&running).map.remove(&key(&id));
            // the hub stops listening at exit, so the last lines go first
            let _ = tokio::time::timeout(OUTPUT_GRACE, pumped).await;
            send(json!({ "id": id, "type": "exit", "code": code }));
        });
    }

    fn pty(&self, id: Value, m: &Value) {
        let size = |k: &str, d: u16| m.get(k).and_then(Value::as_u64).map(|v| v as u16).unwrap_or(d);
        let shell = shell_for(&text(m, "cmd"));
        let pair = match portable_pty::native_pty_system().openpty(portable_pty::PtySize { rows: size("rows", 24), cols: size("cols", 80), pixel_width: 0, pixel_height: 0 }) {
            Ok(p) => p,
            Err(e) => return (self.send)(json!({ "id": id, "type": "error", "error": e.to_string() })),
        };
        let mut cmd = portable_pty::CommandBuilder::new(&shell[0]);
        cmd.args(&shell[1..]);
        cmd.env_clear();
        for (k, v) in child_env() {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => return (self.send)(json!({ "id": id, "type": "error", "error": e.to_string() })),
        };
        drop(pair.slave);
        let (reader, writer) = match (pair.master.try_clone_reader(), terminal_input(&*pair.master)) {
            (Ok(r), Ok(w)) => (r, w),
            _ => return (self.send)(json!({ "id": id, "type": "error", "error": "couldn't open the terminal" })),
        };
        let (input, keys) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut writer = writer;
            // ends when the terminal is gone and its sender with it, or the terminal closed
            for data in keys {
                if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
        });
        let pty = Arc::new(Pty { pid: child.process_id(), input, master: Mutex::new(pair.master), killer: Mutex::new(child.clone_killer()) });
        if !register(&self.running, &id, Handle::Pty(pty)) {
            return;
        }
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let send = self.send.clone();
        let data_id = id.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = vec![0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => send(json!({ "id": data_id, "type": "data", "data": B64.encode(&buf[..n]) })),
                }
            }
            let _ = done_tx.send(());
        });
        let (running, send) = (self.running.clone(), self.send.clone());
        std::thread::spawn(move || {
            let mut child = child;
            let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(1);
            // what the terminal printed last goes before the exit
            let _ = done_rx.recv_timeout(Duration::from_millis(300));
            jobs(&running).map.remove(&key(&id));
            send(json!({ "id": id, "type": "exit", "code": code }));
        });
    }

    /// The connection closed: everything it started ends.
    pub fn close(&self) {
        let all: Vec<Handle> = {
            let mut j = jobs(&self.running);
            j.closed = true;
            j.map.drain().map(|(_, h)| h).collect()
        };
        for h in &all {
            h.kill();
        }
    }
}

impl Drop for Ops {
    fn drop(&mut self) {
        self.close();
    }
}

/// JavaScript's idea of true.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

async fn exec(running: Running, send: Sender, id: Value, m: Value) {
    if jobs(&running).closed {
        return;
    }
    let cmd = text(&m, "cmd");
    let mut env = child_env();
    let mut snapshot = None;
    // the hub's process listing: answer it from Windows' own process table
    if WIN && Regex::new(r"\bps -e -o ").unwrap().is_match(&cmd) {
        let file = std::env::temp_dir().join(format!("inyon-ps-{}-{}.txt", std::process::id(), id.as_str().map(String::from).unwrap_or_else(|| id.to_string())));
        let _ = std::fs::write(&file, win_procs().await);
        env.push(("INYON_PS".into(), file.clone().into_os_string()));
        snapshot = Some(file);
    }
    let input = m.get("input").filter(|v| truthy(v)).and_then(Value::as_str).map(|s| B64.decode(s.trim()).unwrap_or_default());
    let shell = shell_for(&cmd);
    let mut c = tokio::process::Command::new(&shell[0]);
    c.args(&shell[1..]).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).env_clear().envs(env);
    hide_window(&mut c);
    if !WIN {
        own_session(&mut c);
    }
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            if let Some(f) = &snapshot {
                let _ = std::fs::remove_file(f);
            }
            return send(json!({ "id": id, "type": "error", "error": e.to_string() }));
        }
    };
    if !register(&running, &id, Handle::Process { pid: child.id(), _stdin: None }) {
        if let Some(f) = &snapshot {
            let _ = std::fs::remove_file(f);
        }
        return;
    }
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(&data).await;
        });
    }
    let collect = |s: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let done = tokio::spawn(async move {
            let Some(mut s) = s else { return };
            let mut buf = vec![0u8; 16384];
            while let Ok(n) = s.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                g.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&buf[..n]);
            }
        });
        (got, done)
    };
    let (out, out_done) = collect(child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>));
    let (err, err_done) = collect(child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>));
    let code = child.wait().await.map(|s| exit_code(&s)).unwrap_or(1);
    // a timed-out or killed command answers even if something it started still holds the output
    let _ = tokio::time::timeout(OUTPUT_GRACE, async {
        let _ = out_done.await;
        let _ = err_done.await;
    })
    .await;
    jobs(&running).map.remove(&key(&id));
    if let Some(f) = &snapshot {
        let _ = std::fs::remove_file(f);
    }
    let text = |b: &Arc<Mutex<Vec<u8>>>| lf(String::from_utf8_lossy(&b.lock().unwrap_or_else(|e| e.into_inner())).into_owned());
    send(json!({ "id": id, "type": "exit", "code": code, "stdout": text(&out), "stderr": text(&err) }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_windows_live_output_from_a_hub_attach_gets_its_session_in_front_of_the_pane() {
        // what the hub's tmux/live.ts runs (attach), and the plain form from before it
        let attach = r#"exec tmux $([ "$(command -v tmux)" = tmux ] || echo -u) -C attach-session -t '$3' -f read-only,ignore-size"#;
        let plain = "tmux -C attach-session -t $3";
        for cmd in [attach, plain] {
            let map = win_line(cmd, true).expect("a mapping");
            assert_eq!(map("%output %1 aGVsbG8="), "%output %3.1 aGVsbG8=");
            assert_eq!(map("%extended-output %12 0 : x"), "%extended-output %3.12 0 : x");
            assert_eq!(map("%window-add @1"), "%window-add @1");
        }
        assert!(win_line("tmux list-panes -a", true).is_none());
        assert!(win_line(attach, false).is_none());
    }

    #[cfg(unix)]
    /// A sleep only these tests start, so they can tell whether it is still running.
    fn mark(n: u32) -> String {
        format!("{}.{}{n}", 4000 + crate::now_ms() % 999, std::process::id())
    }

    #[cfg(unix)]
    fn left(mark: &str) -> Vec<i32> {
        let out = std::process::Command::new("ps").args(["-eo", "pid=,args="]).output().unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let (pid, args) = l.trim().split_once(char::is_whitespace)?;
                (args.trim() == format!("sleep {mark}")).then(|| pid.parse().ok())?
            })
            .collect()
    }

    #[cfg(unix)]
    struct Frames(Arc<Mutex<Vec<Value>>>);

    #[cfg(unix)]
    impl Frames {
        async fn next(&self, id: &str, kind: &str) -> Value {
            for _ in 0..100 {
                if let Some(m) = self.0.lock().unwrap().iter().find(|m| m["id"] == id && m["type"] == kind) {
                    return m.clone();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("no {kind} for {id}");
        }

        async fn next_num(&self, id: u64) -> Value {
            for _ in 0..100 {
                if let Some(m) = self.0.lock().unwrap().iter().find(|m| m["id"] == id && m["type"] == "exit") {
                    return m.clone();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("no exit for {id}");
        }
    }

    #[cfg(unix)]
    fn ops() -> (Ops, Frames) {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let f = frames.clone();
        (Ops::new(Arc::new(move |m| f.lock().unwrap().push(m))), Frames(frames))
    }

    #[cfg(unix)]
    async fn until(ok: impl Fn() -> bool) {
        for _ in 0..60 {
            if ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_answers_with_the_output_code_and_input() {
        let (o, f) = ops();
        o.message(json!({ "op": "exec", "id": "e", "cmd": "echo out; echo err >&2; exit 3" }));
        let m = f.next("e", "exit").await;
        assert_eq!((m["code"].clone(), m["stdout"].clone(), m["stderr"].clone()), (json!(3), json!("out\n"), json!("err\n")));
        o.message(json!({ "op": "exec", "id": 7, "cmd": "cat", "input": B64.encode("from the phone") }));
        assert_eq!(f.next_num(7).await["stdout"], json!("from the phone"));
        o.message(json!({ "op": "stream", "id": "s", "cmd": "printf 'a\\n\\nb'" }));
        f.next("s", "exit").await;
        let lines: Vec<Value> = f.0.lock().unwrap().iter().filter(|m| m["id"] == "s" && m["type"] == "line").map(|m| m["data"].clone()).collect();
        assert_eq!(lines, [json!("a"), json!("b")]);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn killing_a_command_stops_its_whole_pipeline_and_the_exec_still_answers() {
        let mark = mark(1);
        let (o, f) = ops();
        o.message(json!({ "op": "exec", "id": "a", "cmd": format!("sleep {mark} | cat") }));
        until(|| !left(&mark).is_empty()).await;
        assert!(!left(&mark).is_empty());
        let t = std::time::Instant::now();
        o.message(json!({ "op": "kill", "id": "a" }));
        let m = f.next("a", "exit").await;
        assert!(t.elapsed() < Duration::from_secs(3));
        assert_eq!(m["code"], json!(143));
        until(|| left(&mark).is_empty()).await;
        assert!(left(&mark).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_held_open_by_a_child_that_left_the_group_does_not_hang_the_exec() {
        if which("setsid").is_none() {
            return;
        }
        let mark = mark(2);
        let (o, f) = ops();
        let t = std::time::Instant::now();
        o.message(json!({ "op": "exec", "id": "b", "cmd": format!("setsid sleep {mark} & echo started") }));
        let m = f.next("b", "exit").await;
        assert_eq!(m["stdout"].as_str().unwrap().trim(), "started");
        assert!(t.elapsed() < Duration::from_secs(3));
        for pid in left(&mark) {
            // SAFETY: a sleep this test started
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_the_connection_ends_streams_and_everything_they_started() {
        let mark = mark(3);
        let (o, _f) = ops();
        o.message(json!({ "op": "stream", "id": "c", "cmd": format!("sleep {mark} | cat") }));
        until(|| !left(&mark).is_empty()).await;
        assert!(!left(&mark).is_empty());
        o.close();
        until(|| left(&mark).is_empty()).await;
        assert!(left(&mark).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_paste_into_a_terminal_that_is_not_reading_holds_nothing_up() {
        let (o, f) = ops();
        let o = Arc::new(o);
        // raw like a full-screen agent: the terminal keeps every key, so once its buffer is full a write waits for a read that never comes
        o.message(json!({ "op": "pty", "id": "p", "cmd": "stty raw -echo; sleep 30" }));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let big = "x".repeat(4 << 20);
        let (done, finished) = std::sync::mpsc::channel();
        let o2 = o.clone();
        std::thread::spawn(move || {
            o2.message(json!({ "op": "write", "id": "p", "data": big }));
            o2.message(json!({ "op": "kill", "id": "p" }));
            let _ = done.send(());
        });
        assert!(finished.recv_timeout(Duration::from_secs(5)).is_ok(), "the paste held up the kill");
        f.next("p", "exit").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_arrives_as_the_connection_closes_is_ended_too() {
        let mark = mark(5);
        let (o, _f) = ops();
        o.message(json!({ "op": "exec", "id": "late", "cmd": format!("sleep {mark}") }));
        // closed before the command had its turn to start (one thread: it waits for this test to yield)
        o.close();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let stray = left(&mark);
        for pid in &stray {
            // SAFETY: a sleep this test started
            unsafe { libc::kill(*pid, libc::SIGKILL) };
        }
        assert!(stray.is_empty(), "the command kept running after its connection closed");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_terminal_runs_with_its_size_and_takes_keys() {
        let (o, f) = ops();
        o.message(json!({ "op": "pty", "id": "p", "cmd": "stty size; read line; echo got $line", "rows": 30, "cols": 100 }));
        tokio::time::sleep(Duration::from_millis(300)).await;
        o.message(json!({ "op": "write", "id": "p", "data": "hello\r" }));
        let m = f.next("p", "exit").await;
        assert_eq!(m["code"], json!(0));
        let out: Vec<u8> = f.0.lock().unwrap().iter().filter(|m| m["id"] == "p" && m["type"] == "data").flat_map(|m| B64.decode(m["data"].as_str().unwrap()).unwrap()).collect();
        let out = String::from_utf8_lossy(&out);
        assert!(out.contains("30 100"), "{out}");
        assert!(out.contains("got hello"), "{out}");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_a_terminal_types_nothing_into_it() {
        // a program that outlives the kill for a moment, like a tmux client on its way out: what
        // reaches it then reaches the pane it shows, and a newline there is Enter
        let dir = std::env::temp_dir().join(format!("inyon-pty-close-{}-{}", std::process::id(), crate::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let (got, pid) = (dir.join("got"), dir.join("pid"));
        let (o, f) = ops();
        o.message(json!({ "op": "pty", "id": "p", "cmd": format!("trap '' TERM HUP; stty raw -echo; echo $$ > '{}'; exec cat > '{}'", pid.display(), got.display()) }));
        let started = std::time::Instant::now();
        while !std::fs::read_to_string(&pid).is_ok_and(|s| s.ends_with('\n')) {
            assert!(started.elapsed() < Duration::from_secs(5), "the terminal never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        o.message(json!({ "op": "kill", "id": "p" }));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let typed = std::fs::read(&got).unwrap_or_default();
        let cat: i32 = std::fs::read_to_string(&pid).unwrap().trim().parse().unwrap();
        // SAFETY: the cat this test started, which ignores the polite signals
        unsafe { libc::kill(cat, libc::SIGKILL) };
        let _ = f.next("p", "exit").await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(typed.is_empty(), "closing the terminal typed {typed:?}");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_variable_that_is_not_utf8_reaches_the_command_unchanged() {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: a name no other test reads, set before anything starts that reads the environment
        unsafe { std::env::set_var("INYON_TEST_LATIN1", OsStr::from_bytes(b"caf\xe9")) };
        let (ops, frames) = ops();
        ops.message(json!({ "op": "exec", "id": "x", "cmd": "printf %s \"$INYON_TEST_LATIN1\" | od -An -tx1" }));
        let exit = frames.next("x", "exit").await;
        assert_eq!(exit["code"], json!(0));
        assert_eq!(exit["stdout"].as_str().unwrap().split_whitespace().collect::<Vec<_>>(), ["63", "61", "66", "e9"]);
    }

    // win-prelude.test.ts: the prelude under a plain sh, with a stand-in for psmux's tmux.exe
    // that logs its argv and answers like psmux would
    #[cfg(unix)]
    struct Psmux(std::path::PathBuf);

    #[cfg(unix)]
    impl Psmux {
        fn new() -> Psmux {
            use std::os::unix::fs::PermissionsExt;
            let bin = std::env::temp_dir().join(format!("psmux-{}-{}", std::process::id(), mark_n()));
            std::fs::create_dir_all(&bin).unwrap();
            let tmux = bin.join("tmux");
            std::fs::write(
                &tmux,
                format!(
                    "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> \"{argv}\"; done; echo >> \"{argv}\"\ncase \"$1\" in\n  list-panes) echo '$14 $14:%1 $14:@1 x'; echo '$16 $16:%1 $16:@1 y'; exit 3 ;;\n  list-sessions) printf '$14\\n$16\\n' ;;\n  list-clients) [ \"$3\" = '$14' ] && echo '/dev/pts/2: a: pwsh [80x24] (utf8) (control mode) [activity=1s ago]' ;;\nesac\n",
                    argv = bin.join("argv").display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
            Psmux(bin)
        }

        /// stdout of `cmd` after the prelude, and the argv log
        fn run(&self, cmd: &str, env: &[(&str, &str)]) -> (String, String) {
            std::fs::write(self.0.join("argv"), "").unwrap();
            let o = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("{WIN_PRELUDE}\n{cmd}"))
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", self.0.display()))
                .envs(env.iter().copied())
                .output()
                .unwrap();
            (String::from_utf8_lossy(&o.stdout).into_owned(), std::fs::read_to_string(self.0.join("argv")).unwrap())
        }
    }

    #[cfg(unix)]
    impl Drop for Psmux {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn mark_n() -> u64 {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(unix)]
    #[test]
    fn psmux_targets_session_pane_and_window_ids_become_qualified_ids() {
        let (_, argv) = Psmux::new().run("tmux send-keys -t '%14.1' -l -- '%3.4 #{pane_id}' && tmux select-window -t @16.2 && tmux send-keys -t %14.1 Enter", &[]);
        assert_eq!(argv, "[send-keys][-t][$14:.%1][-l][--][%3.4 #{pane_id}]\n[select-window][-t][$16:@2]\n[send-keys][-t][$14:.%1][Enter]\n");
    }

    #[cfg(unix)]
    #[test]
    fn pane_and_window_ids_print_as_session_pane_and_the_exit_code_survives() {
        let (out, argv) = Psmux::new().run("tmux list-panes -a -F '#{session_id} #{pane_id} #{window_id} x'; echo \"rc=$?\"", &[]);
        assert_eq!(argv, "[list-panes][-a][-F][#{session_id} #{session_id}:#{pane_id} #{session_id}:#{window_id} x]\n");
        assert_eq!(out, "$14 %14.1 @14.1 x\n$16 %16.1 @16.1 y\nrc=3\n");
    }

    #[cfg(unix)]
    #[test]
    fn list_clients_is_answered_per_session_from_psmux_default_output() {
        assert_eq!(Psmux::new().run("tmux list-clients -F '#{session_id} #{client_control_mode}'", &[]).0, "$14 1\n");
        // the probe's longer format (with the last key and the pane) gets the same two fields
        assert_eq!(Psmux::new().run("tmux list-clients -F '#{session_id} #{client_control_mode} #{client_activity} #{pane_id}'", &[]).0, "$14 1\n");
    }

    #[cfg(unix)]
    #[test]
    fn ps_reads_the_snapshot_the_node_wrote_pgrep_finds_nothing() {
        let p = Psmux::new();
        let snap = p.0.join("ps.txt");
        std::fs::write(&snap, "4 0 100 claude claude --resume x\n").unwrap();
        let (out, _) = p.run("ps -e -o pid=; pgrep -f claude || echo none", &[("INYON_PS", &snap.to_string_lossy())]);
        assert_eq!(out, "4 0 100 claude claude --resume x\nnone\n");
    }

    #[test]
    fn commands_get_the_programs_own_address_under_both_names() {
        let env = child_env();
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_string_lossy().into_owned());
        assert!(get("INYON_EXE").is_some_and(|v| !v.is_empty()));
        assert_eq!(get("INYON_EXE"), get("TETHER_NODE_EXE"));
        assert_eq!(get("INYON_SCRIPT").as_deref(), Some(""));
    }
}
