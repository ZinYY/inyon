//! The engine: what runs the link, the hub connection and the hub, and writes pairings and keys.
//! All of it is built into this program (the inyon-engine and inyon-hub crates): `link`,
//! `connect`, `hub` and the old `https://…` form run here, and `pair`, `forget` and `push` run
//! this program as `inyon engine … --json` and draw its answer. The Bun engine (`inyon-engine`
//! from a release) ran the hub until 0.22 and everything with INYON_LEGACY_ENGINE=1; both were
//! removed in 0.23, and releases since carry `inyon-engine-*` files only as copies of this program
//! for older inyon commands.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use serde_json::Value;

use crate::paths::{env_var, installed_engine, ENGINE};
use crate::sys::quiet;
use crate::update;

/// The notes older versions left when a machine's services were set up on the Bun engine or the
/// Bun hub (INYON_LEGACY_ENGINE=1, INYON_HUB_IMPL=bun).
fn retired_notes() -> [PathBuf; 2] {
    [crate::paths::node_dir().join("legacy-engine"), crate::paths::node_dir().join("hub-impl")]
}

/// What still asks for the Bun engine or hub, which 0.23 removed: the variables when they ask for
/// it (here or, for INYON_HUB_IMPL, in hub.env) and the notes older versions left. One line
/// saying they're no longer read, or None. The notes are removed as it's said.
pub fn retired() -> Option<String> {
    let hub_env = crate::paths::read_env_file(&crate::paths::data_dir().join("hub.env")).remove("INYON_HUB_IMPL");
    let mut names = vec![];
    if env_var("INYON_LEGACY_ENGINE").is_some_and(|v| v != "0" && !v.eq_ignore_ascii_case("false")) {
        names.push("INYON_LEGACY_ENGINE");
    }
    if env_var("INYON_HUB_IMPL").or(hub_env).is_some_and(|v| !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("rust")) {
        names.push("INYON_HUB_IMPL=bun");
    }
    if env_var("INYON_ENGINE").is_some() {
        names.push("INYON_ENGINE");
    }
    let mut noted = false;
    for note in retired_notes() {
        noted |= std::fs::remove_file(note).is_ok();
    }
    if names.is_empty() {
        return noted.then(|| "This machine was set up on the Bun engine or hub, which were removed in 0.23: it runs on the ones built into inyon now.".to_string());
    }
    Some(format!("{} {} no longer read: the Bun engine and hub were removed in 0.23, and inyon runs its own.", names.join(" and "), if names.len() == 1 { "is" } else { "are" }))
}

/// Answers an engine command with the engine built into this program, as `inyon-engine` would.
pub fn run_builtin(args: &[String]) -> i32 {
    // a link or hub connection the Inyon app started: a new version of the app replaces an old one
    // an older updater left running (see app::relaunch_if_older)
    if cfg!(windows) && crate::paths::env_var("INYON_APP").is_some() && args.first().is_some_and(|a| a == "link" || a == "connect") {
        std::thread::spawn(|| {
            // not while the update that started this engine is still restarting jobs: its next
            // restart would find the app gone. Then once, 10 seconds after. A "running" status an
            // updater left behind stops counting after 30 minutes, so this waits a little longer
            for _ in 0..31 * 6 {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if !crate::update::under_way() {
                    crate::app::relaunch_if_older(&crate::sys::RealProc);
                    return;
                }
            }
        });
    }
    inyon_engine::cli::run(args, |rest| {
        #[cfg(unix)]
        return inyon_hub::run(rest);
        #[cfg(not(unix))]
        {
            let _ = rest;
            eprintln!("The Inyon hub runs on Linux and macOS. On Windows, install it inside WSL.");
            2
        }
    })
}

/// What only the built-in engine's windowless copy holds (see `make_copy`), so it's never taken
/// for the Bun engine.
pub const COPY_MARK: &str = "inyon: the built-in engine, windowless copy";

/// The PE header's subsystem field of a Windows program: 3 is a console program, 2 one without.
fn subsystem_at(exe: &[u8]) -> Option<usize> {
    let pe = u32::from_le_bytes(exe.get(0x3c..0x40)?.try_into().ok()?) as usize;
    (exe.get(pe..pe + 4)? == b"PE\0\0").then_some(pe + 4 + 20 + 68).filter(|at| *at + 2 <= exe.len())
}

/// This program as a Windows program without a console, the bytes of `inyon-engine.exe`: logon
/// tasks start it with no window, and named inyon-engine it answers the engine's commands. Tasks,
/// the firewall rule and older inyon commands keep the path they know.
/// Only inyon 0.19 or later can be the copy: an older one doesn't answer to the engine's name and
/// would start itself again and again.
pub fn make_copy(program: &[u8]) -> Result<Vec<u8>, String> {
    if !marked(program) {
        return Err("That inyon is from before 0.19 and has no engine built in, so it can't stand in for the engine.".into());
    }
    let mut out = program.to_vec();
    let at = subsystem_at(&out).ok_or("inyon isn't a Windows program, so there's no copy to make.")?;
    out[at..at + 2].copy_from_slice(&2u16.to_le_bytes());
    Ok(out)
}

/// Whether a program's bytes carry COPY_MARK: inyon 0.19 or later, or its windowless copy.
fn marked(program: &[u8]) -> bool {
    program.windows(COPY_MARK.len()).any(|w| w == COPY_MARK.as_bytes())
}

/// Whether `file` has the engine built in: inyon 0.19 or later, or its windowless copy (both carry
/// COPY_MARK), rather than the Bun engine or an inyon from before 0.19, which still runs it.
pub fn has_engine_built_in(file: &Path) -> bool {
    // the Bun engine is upwards of 40 MB; this program is a fraction of that
    let small = std::fs::metadata(file).is_ok_and(|m| m.len() < 40 << 20);
    small && std::fs::read(file).is_ok_and(|d| marked(&d))
}

/// Whether `file`, where the engine goes, is a copy `make_copy` made (of any version) rather than
/// the Bun engine.
pub fn is_builtin_copy(file: &Path) -> bool {
    has_engine_built_in(file)
}

/// Writes the windowless copy of `program` next to it (Windows), unless one of this exact
/// program is there already. A running copy is renamed aside first, as an update does.
pub fn refresh_copy(program: &Path) -> Result<PathBuf, String> {
    let to = update::engine_beside(program);
    let fresh = make_copy(&std::fs::read(program).map_err(|e| e.to_string())?)?;
    if std::fs::read(&to).is_ok_and(|d| d == fresh) {
        return Ok(to);
    }
    let tmp = PathBuf::from(format!("{}.download", to.to_string_lossy()));
    std::fs::write(&tmp, &fresh).map_err(|e| e.to_string())?;
    update::swap_in(&update::RealFiles, &to, &tmp, crate::state::now_ms() as u64).map_err(|e| e.to_string())?;
    Ok(to)
}

/// What's left of the engine file on Linux and macOS, which nothing runs: the Bun engine an update
/// to 0.19 left, or a copy of inyon a hub on the Bun hub fetched. On Windows the file under that
/// name is the windowless copy the logon tasks start.
pub fn stale_engine() -> Option<PathBuf> {
    let file = installed_engine();
    (!cfg!(windows) && file.is_file()).then_some(file)
}

/// Whether this machine runs a hub, or did: set up as a service, started by hand, or configured.
pub fn hub_here() -> bool {
    crate::paths::data_dir().join("hub.env").exists() || crate::paths::node_dir().join("hub.json").exists() || crate::service::hub_set_up()
}

/// The engine at `engine`. A hub joined by its certificate's sha256 is pinned by the engine that
/// connects (crates/inyon-engine/src/hub_tls.rs), never made an authority here.
fn command(engine: &Path) -> Command {
    // hidden by the caller: quiet for what's read back, console_child for a terminal
    let mut c = Command::new(engine);
    c.env("INYON_CLI", std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    c
}

/// The engine commands that may run the link: `link`, and `pair`, which starts the link itself
/// when none is running and stays on as it.
fn may_run_link(first: &str) -> bool {
    matches!(first, "link" | "pair")
}

/// On Windows the link runs from the windowless copy, `inyon-engine.exe`, even when started from a
/// terminal: the firewall rule "Inyon link" lets that program in, and only that one.
fn windows_copy(first: &str) -> Option<PathBuf> {
    if !cfg!(windows) || !may_run_link(first) {
        return None;
    }
    let me = std::env::current_exe().ok()?;
    [update::engine_beside(&me), installed_engine()].into_iter().find(|p| is_builtin_copy(p))
}

/// The program that answers engine command `first`: this one as `inyon engine`, or on Windows
/// the windowless copy for the link.
fn engine_command(first: &str) -> Result<(Command, PathBuf), String> {
    if let Some(copy) = windows_copy(first) {
        return Ok((command(&copy), copy));
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    // hidden by the caller: quiet for what's read back, console_child for a terminal
    let mut c = Command::new(&me);
    c.arg("engine");
    Ok((c, me))
}

/// Runs engine command `args` in this process, or for the link on Windows in the windowless copy.
pub fn forward(args: &[String]) -> ! {
    let first = args.first().map(String::as_str).unwrap_or("");
    if let Some(copy) = windows_copy(first) {
        let mut c = command(&copy);
        c.args(args);
        match relayed(c) {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(err) => eprintln!("Couldn't start {} ({err}); running the link here.", copy.display()),
        }
    }
    std::process::exit(run_builtin(args))
}

/// Runs the engine with `args`, its output going to this terminal, and returns how it ended.
pub fn run_through(args: &[String]) -> Result<ExitStatus, String> {
    let (mut c, engine) = engine_command(args.first().map(String::as_str).unwrap_or(""))?;
    c.args(args);
    relayed(c).map_err(|e| format!("Couldn't start {} ({e}).", engine.display()))
}

/// Runs the engine with this terminal. On Windows the engine has no console of its own (so the
/// sign-in task starts it with no window), so what it prints comes through pipes and is passed
/// on here, INYON_CONSOLE tells it to print rather than log, and it runs in a job that ends it
/// with this program (Ctrl-C, a closed window).
fn relayed(mut c: Command) -> std::io::Result<ExitStatus> {
    if !cfg!(windows) {
        return c.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).status();
    }
    c.env("INYON_CONSOLE", "1").stdin(Stdio::inherit()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::sys::console_child(&mut c);
    let mut child = c.spawn()?;
    #[cfg(windows)]
    let _job = inyon_engine::winjob::ends_with_me(&child);
    let out = child.stdout.take().map(|mut o| std::thread::spawn(move || std::io::copy(&mut o, &mut std::io::stdout())));
    let err = child.stderr.take().map(|mut e| std::thread::spawn(move || std::io::copy(&mut e, &mut std::io::stderr())));
    let status = child.wait();
    for t in [out, err].into_iter().flatten() {
        let _ = t.join();
    }
    status
}

/// Runs the engine and collects what it printed.
pub fn capture(args: &[&str]) -> Result<(ExitStatus, String), String> {
    let (mut c, engine) = engine_command(args.first().copied().unwrap_or(""))?;
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    quiet(&mut c);
    let out = c.output().map_err(|e| format!("Couldn't start {} ({e}).", engine.display()))?;
    Ok((out.status, format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
}

/// The engine's version: this program's own, as it's built in.
pub fn version() -> String {
    inyon_engine::VERSION.to_string()
}

/// The last line of JSON in what a command printed (`{"v":1,…}`).
fn last_json(text: &str) -> Option<Value> {
    text.lines().rev().find_map(|l| {
        let l = l.trim();
        l.starts_with('{').then(|| serde_json::from_str::<Value>(l).ok()).flatten()
    })
}

/// Runs an engine command with `--json` (and `envs` in its environment) and returns its answer;
/// an answer with `ok: false`, or none at all, is the error it gave.
pub fn ask(args: &[&str], envs: &[(&str, &str)]) -> Result<Value, String> {
    let (mut c, engine) = engine_command(args.first().copied().unwrap_or(""))?;
    c.args(args).arg("--json").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in envs {
        c.env(k, v);
    }
    quiet(&mut c);
    let out = c.output().map_err(|e| format!("Couldn't start {} ({e}).", engine.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    answer(last_json(&stdout), out.status, &stderr, args.first().copied().unwrap_or(""))
}

fn answer(v: Option<Value>, status: ExitStatus, stderr: &str, what: &str) -> Result<Value, String> {
    match v {
        Some(v) if v.get("ok").and_then(Value::as_bool) == Some(false) => Err(v.get("error").and_then(Value::as_str).unwrap_or("the engine said no").to_string()),
        Some(v) => Ok(v),
        None => {
            let said = stderr.trim();
            Err(if said.is_empty() { format!("{ENGINE} {what} ended with {} and said nothing.", status.code().unwrap_or(1)) } else { said.to_string() })
        }
    }
}

/// The engine's answer that the code expired before the tunnel came up, shown as it comes.
fn expired(v: &Value) -> bool {
    v.get("expired").and_then(Value::as_bool) == Some(true)
}

/// Runs `pair` (with `args`) and calls `show` with each of the engine's answers as soon as it
/// comes: one that says the tunnel is starting (`"waiting": "tunnel"`), then the code, and the code
/// again once the tunnel's address arrives. The engine keeps running when it had to start the
/// link itself, so this waits for it to end afterwards; Ctrl-C reaches both. Nothing shown means
/// the engine failed, and this says why.
pub fn pair(args: &[String], show: &mut dyn FnMut(&Value)) -> Result<(), String> {
    let (mut c, engine) = engine_command("pair")?;
    c.arg("pair").args(args).arg("--json").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // asked from the Inyon window, which has no console, the engine mustn't open one of its own
    quiet(&mut c);
    let mut child = c.spawn().map_err(|e| format!("Couldn't start {} ({e}).", engine.display()))?;
    // asked from the Inyon app's window: it ends with the app, like everything the app runs
    crate::app::hold(&child);
    let stderr = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = e.read_to_string(&mut text);
            text
        })
    });
    let mut answered = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(v) = last_json(&line) {
                // a failure is the engine's last word and is said below, except a code that ran
                // out while the tunnel started: the engine may keep running the link after it
                if v.get("ok").and_then(Value::as_bool) != Some(false) || expired(&v) {
                    show(&v);
                }
                answered = Some(v);
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if answered.as_ref().is_some_and(expired) {
        return Ok(());
    }
    let said = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    answer(answered, status, &said, "pair").map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Windows program's first bytes: the DOS header pointing at a PE header, the subsystem in it.
    fn fake_exe(subsystem: u16) -> Vec<u8> {
        let mut exe = vec![0u8; 512];
        exe[..2].copy_from_slice(b"MZ");
        exe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        exe[0x80..0x84].copy_from_slice(b"PE\0\0");
        let at = 0x80 + 4 + 20 + 68;
        exe[at..at + 2].copy_from_slice(&subsystem.to_le_bytes());
        exe
    }

    #[test]
    fn the_windowless_copy_differs_only_in_its_subsystem() {
        let console = fake_inyon(3);
        let copy = make_copy(&console).unwrap();
        assert_eq!(copy, fake_inyon(2));
        assert_eq!(console.iter().zip(&copy).filter(|(a, b)| a != b).count(), 1);
        assert!(make_copy(format!("#!/bin/sh\n{COPY_MARK}").as_bytes()).is_err());
    }

    // the hub, and so its scripts, are built into inyon on Linux and macOS only
    #[cfg(unix)]
    #[test]
    fn the_hubs_windows_installer_looks_for_this_mark() {
        // install.ps1 makes inyon-engine.exe only from an inyon.exe that carries it
        assert!(inyon_hub::node_dist::INSTALL_PS1.contains(&format!("'{COPY_MARK}'")));
    }

    #[test]
    fn an_inyon_from_before_0_19_is_never_made_the_copy() {
        // it doesn't know the engine's name, so as the engine it would start itself again and again
        let err = make_copy(&fake_exe(3)).unwrap_err();
        assert!(err.contains("before 0.19"), "{err}");
        let dir = std::env::temp_dir().join(format!("inyon-old-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("inyon.exe");
        std::fs::write(&old, fake_exe(3)).unwrap();
        assert!(refresh_copy(&old).is_err());
        assert!(!update::engine_beside(&old).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// inyon 0.19 or later: a Windows program carrying the mark.
    fn fake_inyon(subsystem: u16) -> Vec<u8> {
        let mut exe = fake_exe(subsystem);
        exe.extend_from_slice(COPY_MARK.as_bytes());
        exe
    }

    #[test]
    fn the_copy_is_told_from_the_bun_engine_by_its_mark() {
        let dir = std::env::temp_dir().join(format!("inyon-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (copy, bun) = (dir.join("copy.exe"), dir.join("bun.exe"));
        let mut bytes = fake_exe(2);
        bytes.extend_from_slice(COPY_MARK.as_bytes());
        std::fs::write(&copy, &bytes).unwrap();
        std::fs::write(&bun, fake_exe(2)).unwrap();
        assert!(is_builtin_copy(&copy));
        assert!(!is_builtin_copy(&bun));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairing_runs_where_the_link_does_as_it_may_start_the_link() {
        // on Windows that's the windowless copy, the one program the firewall rule lets in
        assert!(may_run_link("link"));
        assert!(may_run_link("pair"));
        assert!(!may_run_link("forget"));
        assert!(!may_run_link("push"));
    }

    #[test]
    fn the_answer_is_the_last_json_line() {
        assert_eq!(last_json("noise\n{\"v\":1,\"ok\":true,\"link\":\"x\"}\n").unwrap()["link"], "x");
        assert!(last_json("no json here").is_none());
    }
}
