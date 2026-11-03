//! The system calls the service and update code makes: running programs, finding them on PATH,
//! and looking at or stopping processes. The process table is a trait so tests can fake it; the
//! programs are found on PATH so tests can put fakes there.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Darwin,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Platform::Linux
        } else if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Darwin => "darwin",
            Platform::Windows => "windows",
            Platform::Other => "this system",
        }
    }
}

/// The C library a Linux system's programs are built against. The engine comes in a glibc and a
/// musl build (Alpine); the command is static and the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Libc {
    Glibc,
    Musl,
}

impl Libc {
    /// Alpine says so in /etc/alpine-release. Elsewhere glibc's own loader decides: a glibc system
    /// with the musl package installed has musl's loader too, and must still get the glibc engine.
    /// Without either, ldd names its C library. Anything else, and any other OS, counts as glibc.
    pub fn current() -> Libc {
        if cfg!(target_os = "linux") && is_musl(Path::new("/")) {
            Libc::Musl
        } else {
            Libc::Glibc
        }
    }
}

/// The glibc dynamic loaders, by the paths distributions put them at.
const GLIBC_LOADERS: &[&str] = &[
    "lib64/ld-linux-x86-64.so.2",
    "lib/ld-linux-aarch64.so.1",
    "lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
    "lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
    "usr/lib64/ld-linux-x86-64.so.2",
    "usr/lib/ld-linux-aarch64.so.1",
];

/// `root` is "/" except in tests.
fn is_musl(root: &Path) -> bool {
    if root.join("etc/alpine-release").exists() {
        return true;
    }
    if GLIBC_LOADERS.iter().any(|l| root.join(l).exists()) {
        return false;
    }
    // musl's ldd says so on stderr and exits 1; run keeps both
    root == Path::new("/") && run(&["ldd", "--version"]).out.contains("musl")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Signal::Term => "SIGTERM",
            Signal::Kill => "SIGKILL",
        }
    }
}

/// How a process is looked at and stopped (tests swap this out).
pub trait Proc {
    /// The arguments `pid` runs with, or None when there's no such process.
    fn cmdline(&self, pid: u32) -> Option<Vec<String>>;
    fn signal(&self, pid: u32, sig: Signal);
    /// Whether `pid` still runs (not just an exited process nobody collected).
    fn alive(&self, pid: u32) -> bool;
    fn sleep_ms(&self, ms: u64);
}

pub struct RealProc;

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getuid() -> u32;
}

impl Proc for RealProc {
    fn cmdline(&self, pid: u32) -> Option<Vec<String>> {
        let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
    }

    #[cfg(unix)]
    fn signal(&self, pid: u32, sig: Signal) {
        let Some(pid) = kill_target(pid) else {
            return;
        };
        let n = match sig {
            Signal::Term => 15,
            Signal::Kill => 9,
        };
        // SAFETY: kill takes two integers and touches no memory of ours
        unsafe {
            kill(pid, n);
        }
    }

    #[cfg(not(unix))]
    fn signal(&self, pid: u32, sig: Signal) {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string()]);
        if sig == Signal::Kill {
            c.arg("/F");
        }
        let _ = quiet(&mut c).output();
    }

    #[cfg(unix)]
    fn alive(&self, pid: u32) -> bool {
        let Some(target) = kill_target(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists
        let exists = unsafe { kill(target, 0) } == 0;
        exists && !exited(pid)
    }

    #[cfg(not(unix))]
    fn alive(&self, pid: u32) -> bool {
        if pid <= 1 {
            return false;
        }
        let mut c = Command::new("tasklist");
        c.args(["/FI", &format!("PID eq {pid}"), "/NH"]);
        quiet(&mut c).output().map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string())).unwrap_or(false)
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// A pid from state.json as kill() takes it. Nothing for 0, 1 or anything past i32, since
/// kill(-1, …) would signal every process of this user and kill(0, …) the whole group.
#[cfg(unix)]
fn kill_target(pid: u32) -> Option<i32> {
    if pid <= 1 {
        return None;
    }
    i32::try_from(pid).ok()
}

/// A process that has exited but wasn't collected yet (in a container whose first process
/// doesn't collect them, say) still answers kill(pid, 0). Linux shows it as Z.
pub fn exited(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // "123 (name with spaces) Z ..."
    stat.rsplit_once(')').map(|(_, rest)| rest.trim_start().starts_with('Z')).unwrap_or(false)
}

/// The user id, for launchd's gui/<uid> domain.
#[cfg(unix)]
pub fn uid() -> u32 {
    // SAFETY: getuid takes nothing and cannot fail
    unsafe { getuid() }
}

#[cfg(not(unix))]
pub fn uid() -> u32 {
    0
}

pub fn username() -> String {
    for k in ["USER", "LOGNAME", "USERNAME"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    run(&["id", "-un"]).out
}

pub struct Output {
    pub ok: bool,
    /// stdout and stderr, trimmed
    pub out: String,
}

/// CREATE_NO_WINDOW: the child gets a console of its own that is never shown, and whatever it
/// starts shares that hidden console instead of opening one.
pub const NO_WINDOW: u32 = 0x0800_0000;

/// Keeps a child from opening a console window on Windows.
pub fn quiet(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(NO_WINDOW);
    }
    c
}

/// For a child whose output belongs in the terminal (a package manager's progress, the journal):
/// it shares this program's console when there is one to see. Started from the Inyon app or a
/// task, which have none, a console program would open a window of its own, so it gets a hidden
/// one instead (Windows; elsewhere nothing changes).
pub fn console_child(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    if !has_console() {
        quiet(c);
    }
    c
}

/// Whether this program has a console someone can see (a terminal, Windows Terminal or an SSH
/// session). The Inyon app, the engine's windowless copy and anything started hidden have none.
#[cfg(windows)]
pub fn has_console() -> bool {
    // SAFETY: takes nothing and only reads this process's console
    !unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null()
}

#[cfg(not(windows))]
pub fn has_console() -> bool {
    true
}

/// Runs a program and collects what it printed. A missing program (no systemctl in a
/// container, say) is a failure like any other, not a crash.
pub fn run(cmd: &[&str]) -> Output {
    run_with_stdin(cmd, None)
}

pub fn run_with_stdin(cmd: &[&str], stdin: Option<&str>) -> Output {
    let Some((prog, args)) = cmd.split_first() else {
        return Output { ok: false, out: String::new() };
    };
    let mut c = Command::new(prog);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    quiet(&mut c);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(err) => return Output { ok: false, out: format!("{prog} isn't available here ({err})") },
    };
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        let _ = pipe.write_all(text.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => Output { ok: o.status.success(), out: format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string() },
        Err(err) => Output { ok: false, out: err.to_string() },
    }
}

/// Where `name` is on PATH, if anywhere.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let mut e = vec![String::new()];
        e.extend(std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into()).split(';').map(|s| s.to_lowercase()));
        e
    } else {
        vec![String::new()]
    };
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    // a program started over SSH (or before a Path change) has an older PATH than a new terminal
    // would: the Path saved in the registry counts too
    dirs.extend(saved_path().into_iter().filter(|d| !dirs.contains(d)).collect::<Vec<_>>());
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The user's and the machine's Path as saved in the registry, what a new terminal starts with.
#[cfg(windows)]
fn saved_path() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    fn read(root: HKEY, key: &str) -> Option<String> {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (key, value) = (wide(key), wide("Path"));
        let mut size = 0u32;
        // REG_SZ takes REG_EXPAND_SZ too, with %USERPROFILE% and the like filled in (asking for
        // REG_EXPAND_SZ itself is refused unless it's left unexpanded)
        let flags = RRF_RT_REG_SZ;
        unsafe {
            if RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) != 0 {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            if RegGetValueW(root, key.as_ptr(), value.as_ptr(), flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) != 0 {
                return None;
            }
            let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }
    [read(HKEY_CURRENT_USER, "Environment"), read(HKEY_LOCAL_MACHINE, "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment")]
        .into_iter()
        .flatten()
        .flat_map(|p| p.split(';').filter(|d| !d.trim().is_empty()).map(|d| PathBuf::from(d.trim())).collect::<Vec<_>>())
        .collect()
}

#[cfg(not(windows))]
fn saved_path() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Starts `args` on its own, in a session of its own, with its output going to `log`.
pub fn spawn_detached(args: &[String], log: &Path) -> std::io::Result<()> {
    let (prog, rest) = args.split_first().ok_or_else(|| std::io::Error::other("nothing to run"))?;
    let out = fs::OpenOptions::new().create(true).append(true).open(log)?;
    let err = out.try_clone()?;
    let mut c = Command::new(prog);
    c.args(rest).stdin(Stdio::null()).stdout(out).stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // a hidden console of its own (it outlives this one) in a process group of its own. Not
        // DETACHED_PROCESS: with no console at all, every console program it started would open
        // a window
        c.creation_flags(NO_WINDOW | 0x0000_0200);
    }
    #[cfg(not(windows))]
    quiet(&mut c);
    c.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_is_a_failed_step_not_a_crash() {
        // a container or minimal server has no systemctl; setup must fall back instead of dying
        let r = run(&["inyon-no-such-program-here", "--user", "show-environment"]);
        assert!(!r.ok);
        assert!(r.out.contains("isn't available here"));
        assert!(run(&["true"]).ok || cfg!(windows));
    }

    #[test]
    fn alpine_is_musl_and_glibc_with_the_musl_package_is_still_glibc() {
        let root = std::env::temp_dir().join(format!("inyon-libc-{}", std::process::id()));
        let put = |rel: &str| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        };
        // Ubuntu with musl-tools: both loaders are there
        put("lib/ld-musl-x86_64.so.1");
        put("lib64/ld-linux-x86-64.so.2");
        assert!(!is_musl(&root));
        // Alpine says so whatever else is installed
        put("etc/alpine-release");
        assert!(is_musl(&root));
        fs::remove_dir_all(&root).unwrap();
        // nothing to go on, away from the real root: glibc
        put("etc/hostname");
        assert!(!is_musl(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exited_reads_the_state_letter() {
        assert!(!exited(u32::MAX));
    }

    #[cfg(unix)]
    #[test]
    fn only_a_real_pid_is_signalled() {
        // 4294967295 as i32 is -1, which kill() reads as "everything of mine"
        assert_eq!(kill_target(u32::MAX), None);
        assert_eq!(kill_target(i32::MAX as u32 + 1), None);
        assert_eq!(kill_target(0), None);
        assert_eq!(kill_target(1), None);
        assert_eq!(kill_target(4242), Some(4242));
        assert!(!RealProc.alive(u32::MAX));
        RealProc.signal(u32::MAX, Signal::Term);
    }
}
