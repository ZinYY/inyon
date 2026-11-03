//! Keeps inyon running without a terminal window: a user service that starts at login and comes
//! back if it stops (systemd on Linux, launchd on macOS, a logon task on Windows). No admin
//! rights, nothing outside your home folder. The program copies itself into ~/.inyon/bin first,
//! so the download can be deleted.
//!
//! One program does three jobs, each its own service:
//! - link: the phone link (`inyon setup` or `inyon pair`)
//! - node: an outbound connection to an Inyon hub (the hub's "Add a machine" command)
//! - hub: the hub itself (`get.sh --hub`, Linux and macOS)
//!
//! The units run `~/.inyon/bin/inyon link|connect|hub`: this program, which runs the link and the
//! hub connection and the hub itself. Units written by earlier versions
//! name the same file, so they keep working. On Windows the logon tasks open the Inyon app
//! (`inyon-app.exe app --background link`), which runs `inyon-engine.exe`, a windowless copy of
//! this program (see engine::make_copy), as its child.

use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::{basename, bin_dir, home, installed_binary, node_dir, ENGINE, NAME};
use crate::sys::{self, run, run_with_stdin, which, Platform, Proc, Signal};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Link,
    Node,
    Hub,
}

pub const ROLES: [Role; 3] = [Role::Link, Role::Node, Role::Hub];

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Link => "link",
            Role::Node => "node",
            Role::Hub => "hub",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "link" => Some(Role::Link),
            "node" => Some(Role::Node),
            "hub" => Some(Role::Hub),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The Android application id, which launchd labels are named after.
pub const APP_ID: &str = "tech.abhashchakraborty.inyon";

struct Spec {
    unit: &'static str,
    label: &'static str,
    task: Option<&'static str>,
    args: &'static [&'static str],
    what: &'static str,
}

const fn spec(role: Role) -> Spec {
    match role {
        Role::Link => Spec { unit: "inyon-link", label: "tech.abhashchakraborty.inyon.link", task: Some("Inyon link"), args: &["link"], what: "Inyon phone link" },
        Role::Node => Spec {
            unit: "inyon-node",
            label: "tech.abhashchakraborty.inyon.node",
            task: Some("Inyon node"),
            args: &["connect"],
            what: "Inyon node (outbound connection to your Inyon hub)",
        },
        Role::Hub => Spec { unit: "inyon-hub", label: "tech.abhashchakraborty.inyon.hub", task: None, args: &["hub"], what: "Inyon hub" },
    }
}

/// The one-off task an update runs in on Windows (see `detach`).
pub const UPDATE_TASK: &str = "Inyon update";
/// The one the tray icon's move to the built-in engine runs in: a name of its own, so it never
/// replaces an update's task while that update runs, nor an update removes it.
pub const HAND_OVER_TASK: &str = "Inyon engine hand-over";

/// How each job was set up when this program was tether-node, before the rename to Inyon. Both
/// of its installers named their systemd unit tether-node.service; what the unit runs says which job.
struct Old {
    unit: Option<&'static str>,
    label: Option<&'static str>,
    task: Option<&'static str>,
    shortcut: Option<&'static str>,
    cron: Option<&'static str>,
}

const fn old(role: Role) -> Old {
    match role {
        Role::Link => Old { unit: Some("tether-node"), label: Some("dev.tether.node-link"), task: Some("Tether link"), shortcut: None, cron: Some("# tether-node link") },
        Role::Node => Old { unit: Some("tether-node"), label: Some("dev.tether.node"), task: Some("Tether node"), shortcut: Some("Tether node.lnk"), cron: None },
        Role::Hub => Old { unit: Some("tether-hub"), label: None, task: None, shortcut: None, cron: None },
    }
}

/// What the service code needs to know about the machine it runs on. Tests pass a fake process
/// table and whichever platform they're checking.
pub struct Sys<'a> {
    pub platform: Platform,
    pub proc: &'a dyn Proc,
    /// this process, which is never stopped
    pub pid: u32,
    /// the command that runs this program
    pub program: Vec<String>,
}

impl<'a> Sys<'a> {
    pub fn new(proc: &'a dyn Proc) -> Self {
        Sys { platform: Platform::current(), proc, pid: std::process::id(), program: self_command() }
    }
}

/// The command that runs this program: the binary itself (inyon.exe for the Inyon app, which is
/// never what a service or an update runs).
pub fn self_command() -> Vec<String> {
    let exe = std::env::current_exe()
        .map(|p| if crate::app::started_as_app(&p.to_string_lossy()) { crate::app::command_beside(&p) } else { p })
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| NAME.to_string());
    vec![exe]
}

pub fn unit_file(name: &str) -> PathBuf {
    home().join(".config").join("systemd").join("user").join(format!("{name}.service"))
}

pub fn plist_file(label: &str) -> PathBuf {
    home().join("Library").join("LaunchAgents").join(format!("{label}.plist"))
}

/// The name of `role`'s logon task on Windows, if it has one.
pub fn task_name(role: Role) -> Option<&'static str> {
    spec(role).task
}

/// Where a job writes its log when nothing else keeps it (launchd, nohup).
pub fn log_file(role: Role) -> PathBuf {
    node_dir().join(format!("{role}.log"))
}

/// `log_file(role)`, made ready for launchd or nohup: the node's folder closed to other users of
/// this machine (0700) and the log readable only by you (0600). A log names hubs and sessions, and
/// an older hub wrote its setup code there.
fn private_log(role: Role) -> PathBuf {
    let file = log_file(role);
    make_private(&node_dir(), &file);
    file
}

fn make_private(dir: &Path, file: &Path) {
    let _ = fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        let _ = fs::OpenOptions::new().create(true).append(true).mode(0o600).open(file);
        // one made before keeps its mode when opened
        let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = file;
}

/// The PID of a job started in the background where there are no user services.
fn pid_file(role: Role) -> PathBuf {
    node_dir().join(format!("{role}.pid"))
}

fn cron_mark(role: Role) -> String {
    format!("# inyon {role}")
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// systemd splits ExecStart on spaces; quote each argument
fn sdq(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn psq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn ps(script: &str) -> sys::Output {
    run(&["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script])
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Copies this program into ~/.inyon/bin when it runs from somewhere else (a download, a
/// build), so the service points at a file that stays.
pub fn settle(cmd: &[String]) -> Vec<String> {
    let target = installed_binary();
    let Some(first) = cmd.first() else {
        return cmd.to_vec();
    };
    let same = fs::canonicalize(first).ok() == fs::canonicalize(&target).ok() && target.exists();
    if same || Path::new(first) == target {
        return cmd.to_vec();
    }
    let _ = fs::create_dir_all(bin_dir());
    // a running copy can't be overwritten on Windows, but it can be renamed out of the way
    if target.exists() && fs::remove_file(&target).is_err() {
        let _ = fs::rename(&target, format!("{}.old-{}", path_str(&target), crate::state::now_ms() as u64));
    }
    let download = PathBuf::from(format!("{}.download", path_str(&target)));
    if fs::copy(first, &download).is_err() {
        return cmd.to_vec();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&download, fs::Permissions::from_mode(0o755));
    }
    if fs::rename(&download, &target).is_err() {
        return cmd.to_vec();
    }
    vec![path_str(&target)]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub ok: bool,
    /// how it was set up, or why it couldn't be
    pub how: String,
}

fn ok(how: impl Into<String>) -> Installed {
    Installed { ok: true, how: how.into() }
}

fn failed(how: impl Into<String>) -> Installed {
    Installed { ok: false, how: how.into() }
}

fn startup_shortcut(name: &str) -> String {
    format!("(Join-Path ([Environment]::GetFolderPath('Startup')) {})", psq(name))
}

/// Whether `role` is set up to run in the background here, under its own name or the old one.
pub fn has_service(sys: &Sys, role: Role) -> bool {
    let s = spec(role);
    let o = old(role);
    match sys.platform {
        Platform::Linux => unit_file(s.unit).exists() || old_unit_for(role).is_some() || pid_file(role).exists(),
        Platform::Darwin => plist_file(s.label).exists() || o.label.is_some_and(|l| plist_file(l).exists()),
        Platform::Windows => {
            let Some(task) = s.task else { return false };
            let names = [Some(task), o.task].into_iter().flatten().map(psq).collect::<Vec<_>>().join(", ");
            let startup = format!("(Test-Path {})", startup_shortcut(&format!("{task}.lnk")));
            ps(&format!("if (({names} | ForEach-Object {{ Get-ScheduledTask -TaskName $_ -ErrorAction SilentlyContinue }}) -or {startup}) {{ exit 0 }} else {{ exit 1 }}")).ok
        }
        Platform::Other => false,
    }
}

/// Whether `restart_service` has something to restart for `role`: a service, or a copy started in
/// the background with its pid recorded.
pub fn can_restart(sys: &Sys, role: Role) -> bool {
    has_service(sys, role) || pid_file(role).exists()
}

/// Installs and starts `role` as a user service; `restart` replaces a copy that's already running.
pub fn install_service(sys: &Sys, role: Role, restart: bool) -> Installed {
    let s = spec(role);
    let cmd = settle(&sys.program);
    let args: Vec<String> = cmd.iter().cloned().chain(s.args.iter().map(|a| a.to_string())).collect();
    match sys.platform {
        Platform::Linux => {
            if !run(&["systemctl", "--user", "show-environment"]).ok {
                return background(sys, role, &args);
            }
            let file = unit_file(s.unit);
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let exec = args.iter().map(|a| sdq(a)).collect::<Vec<_>>().join(" ");
            let text = format!(
                "[Unit]\nDescription={}\nAfter=network-online.target\n\n[Service]\nExecStart={exec}\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
                s.what
            );
            if let Err(err) = fs::write(&file, text) {
                return failed(format!("couldn't write {} ({err})", path_str(&file)));
            }
            run(&["systemctl", "--user", "daemon-reload"]);
            let unit = format!("{}.service", s.unit);
            let r = run(&["systemctl", "--user", "enable", "--now", &unit]);
            if !r.ok {
                return failed(if r.out.is_empty() { "systemctl refused".to_string() } else { r.out });
            }
            // enable --now leaves a running unit alone
            if restart {
                run(&["systemctl", "--user", "restart", &unit]);
            }
            // keeps it running while you're logged out (allowed for your own user on most systems)
            run(&["loginctl", "enable-linger", &sys::username()]);
            ok(format!("a user service that starts again by itself after a restart (systemctl --user status {})", s.unit))
        }
        Platform::Darwin => {
            let file = plist_file(s.label);
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let log = xml(&path_str(&private_log(role)));
            let program = args.iter().map(|a| format!("<string>{}</string>", xml(a))).collect::<String>();
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>{}</string>\n  <key>ProgramArguments</key><array>{program}</array>\n  <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\n  <key>StandardOutPath</key><string>{log}</string>\n  <key>StandardErrorPath</key><string>{log}</string>\n</dict></plist>\n",
                s.label
            );
            if let Err(err) = fs::write(&file, text) {
                return failed(format!("couldn't write {} ({err})", path_str(&file)));
            }
            let f = path_str(&file);
            run(&["launchctl", "unload", &f]);
            let r = run(&["launchctl", "load", "-w", &f]);
            if !r.ok {
                return failed(if r.out.is_empty() { "launchctl refused".to_string() } else { r.out });
            }
            ok("a login item that starts again by itself when you sign in")
        }
        Platform::Windows => {
            let Some(task) = s.task else {
                return failed("the hub runs on Linux and macOS (on Windows, use WSL)");
            };
            // the task opens the Inyon app (inyon-app.exe, no console), which runs the job, shows
            // the Inyon icon while it does, and ends it on Quit
            let app = match ensure_app(Path::new(&cmd[0])) {
                Ok(a) => a,
                Err(e) => return failed(e),
            };
            let r = ps(&task_script(task, &path_str(&app), role, s.what));
            if !r.ok {
                return failed(r.out.lines().next().filter(|l| !l.is_empty()).unwrap_or("Task Scheduler refused").to_string());
            }
            let how = if r.out.trim().ends_with("startup") {
                "the Inyon app, which runs it from when you sign in (Task Scheduler is blocked here, so from the Startup folder), with the Inyon icon in the notification area"
                    .to_string()
            } else {
                format!("the Inyon app, which runs it from when you sign in (\"{task}\" in Task Scheduler), with the Inyon icon in the notification area")
            };
            match start_in_app(sys, role, restart, &app) {
                Ok(()) => ok(how),
                Err(e) => failed(e),
            }
        }
        Platform::Other => failed(format!("{} isn't supported for a background service", sys.platform.name())),
    }
}

/// The engine next to the installed command, which the logon task runs: a windowless copy of the
/// installed command itself, which answers the engine's commands under that name.
fn engine_for(cmd: &[String]) -> Result<PathBuf, String> {
    let first = cmd.first().map(PathBuf::from).unwrap_or_else(installed_binary);
    crate::engine::refresh_copy(&first)
}

/// The engine program for the Inyon app's jobs: the windowless copy of `inyon`.
pub fn engine_program(inyon: &Path) -> Result<PathBuf, String> {
    engine_for(&[path_str(inyon)])
}

/// What the engine runs for `role`: `link`, `connect` or `hub`.
pub fn engine_args(role: Role) -> &'static [&'static str] {
    spec(role).args
}

/// PowerShell that registers `role`'s logon task to open the Inyon app for it, or puts a shortcut
/// in the Startup folder when Task Scheduler is blocked. It starts nothing: the app does that.
/// Prints 'task' or 'startup'.
pub fn task_script(task: &str, app: &str, role: Role, what: &str) -> String {
    let arg = format!("app --background {}", role.name());
    let shortcut = startup_shortcut(&format!("{task}.lnk"));
    format!(
        "$ErrorActionPreference='Stop'\ntry {{\n  $a = New-ScheduledTaskAction -Execute {exe} -Argument {arg} -WorkingDirectory $env:USERPROFILE\n  $t = New-ScheduledTaskTrigger -AtLogOn -User \"$env:USERDOMAIN\\$env:USERNAME\"\n  $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew\n  $p = New-ScheduledTaskPrincipal -UserId \"$env:USERDOMAIN\\$env:USERNAME\" -LogonType Interactive -RunLevel Limited\n  Register-ScheduledTask -TaskName {name} -Action $a -Trigger $t -Settings $s -Principal $p -Description {what} -Force | Out-Null\n  Remove-Item -Force -ErrorAction SilentlyContinue {shortcut}\n  'task'\n}} catch {{\n  # tasks blocked by policy: a shortcut in the Startup folder\n  $l = (New-Object -ComObject WScript.Shell).CreateShortcut({shortcut})\n  $l.TargetPath = {exe}\n  $l.Arguments = {arg}\n  $l.WorkingDirectory = $env:USERPROFILE\n  $l.Save()\n  'startup'\n}}",
        exe = psq(app),
        arg = psq(&arg),
        name = psq(task),
        what = psq(what),
    )
}

/// Has the Inyon app run `role` now: the running app is asked; otherwise copies an older version
/// started are stopped and the app is opened for it, in the signed-in user's session (also from
/// SSH), through the task when there is one. Waits until the app runs it.
fn start_in_app(sys: &Sys, role: Role, restart: bool, app: &Path) -> Result<(), String> {
    if crate::app::current(sys.proc).is_some() {
        let what = if restart { crate::app::Do::Restart } else { crate::app::Do::Start };
        return crate::app::ask(sys.proc, what, Some(role), None, 10_000).and_then(|r| if r.ok { Ok(()) } else { Err(r.message) });
    }
    let task = spec(role).task.unwrap_or_default();
    let r = ps(&format!(
        "{stop}\nif (Get-ScheduledTask -TaskName {t} -ErrorAction SilentlyContinue) {{ Stop-ScheduledTask -TaskName {t} -ErrorAction SilentlyContinue; Start-ScheduledTask -TaskName {t} }} else {{ Start-Process -FilePath {exe} -ArgumentList 'app', '--background', {role} -WorkingDirectory $env:USERPROFILE }}",
        stop = stop_copies(sys, Some(role)),
        t = psq(task),
        exe = psq(&path_str(app)),
        role = psq(role.name()),
    ));
    if !r.ok {
        return Err(r.out.lines().next().filter(|l| !l.is_empty()).unwrap_or("Task Scheduler refused").to_string());
    }
    for _ in 0..40 {
        if crate::app::running(sys.proc).is_some_and(|a| a.runs(role)) {
            return Ok(());
        }
        sys.proc.sleep_ms(250);
    }
    Err("the Inyon app didn't start (~/.inyon/node/app.log says why)".into())
}

/// ~/.inyon/bin/inyon-app.exe for `program` (inyon.exe): a copy of `program` marked to run without
/// a console, made now unless the one there is already this version's.
pub fn ensure_app(program: &Path) -> Result<PathBuf, String> {
    let app = crate::app::app_beside(program);
    let _ = crate::app::build_mark();
    let mine = crate::app::build_of(program);
    if app.is_file() && mine.is_some() && crate::app::build_of(&app) == mine {
        return Ok(app);
    }
    let fresh = crate::engine::make_copy(&fs::read(program).map_err(|e| e.to_string())?)?;
    let tmp = PathBuf::from(format!("{}.download", path_str(&app)));
    fs::write(&tmp, &fresh).map_err(|e| e.to_string())?;
    // a running app can't be overwritten, but it can be renamed out of the way
    crate::update::swap_in(&crate::update::RealFiles, &app, &tmp, crate::state::now_ms() as u64).map_err(|e| e.to_string())?;
    Ok(app)
}

/// Whether `role` is set to run at sign-in on Windows: its task, or its Startup shortcut.
pub fn task_exists(sys: &Sys, role: Role) -> bool {
    let Some(task) = spec(role).task.filter(|_| sys.platform == Platform::Windows) else { return false };
    let shortcut = std::env::var_os("APPDATA").map(PathBuf::from).map(|d| d.join(r"Microsoft\Windows\Start Menu\Programs\Startup").join(format!("{task}.lnk")));
    run(&["schtasks.exe", "/Query", "/TN", task]).ok || shortcut.is_some_and(|s| s.exists())
}

/// The jobs the Inyon app runs when it's opened with none named: those set to run at sign-in.
pub fn app_roles(sys: &Sys) -> Vec<Role> {
    [Role::Link, Role::Node].into_iter().filter(|r| task_exists(sys, *r)).collect()
}

/// Moves a machine to the Inyon app (from 0.20.3): the app's file, its Start menu shortcut and
/// Settings → Apps entry, and each logon task opening the app instead of the engine. Nothing is
/// started or stopped. Returns the app.
pub fn adopt_app(sys: &Sys) -> Result<PathBuf, String> {
    let program = installed_binary();
    let app = ensure_app(&program)?;
    register_app(sys);
    for role in [Role::Link, Role::Node] {
        if task_exists(sys, role) {
            let s = spec(role);
            let r = ps(&task_script(s.task.unwrap_or_default(), &path_str(&app), role, s.what));
            if !r.ok {
                crate::app::trace(&format!("! the {role}'s task still starts the engine: {}", r.out.lines().next().unwrap_or("")));
            }
        }
    }
    Ok(app)
}

/// Turns `role`'s start at sign-in on or off on Windows (its logon task, or its Startup shortcut
/// where Task Scheduler is blocked). Nothing is started or stopped.
pub fn set_sign_in(sys: &Sys, role: Role, on: bool) -> Result<(), String> {
    if sys.platform != Platform::Windows {
        return Err("Starting at sign-in is set with `inyon setup` on this system.".into());
    }
    let s = spec(role);
    let task = s.task.ok_or_else(|| format!("The {role} has no sign-in task."))?;
    let r = if on {
        let app = ensure_app(&installed_binary())?;
        ps(&task_script(task, &path_str(&app), role, s.what))
    } else {
        ps(&sign_in_off_script(task))
    };
    if !r.ok {
        return Err(r.out.lines().find(|l| !l.trim().is_empty()).unwrap_or("Task Scheduler refused the change.").trim().to_string());
    }
    if task_exists(sys, role) != on {
        return Err(if on { format!("The {role} still doesn't start at sign-in.") } else { format!("The {role} still starts at sign-in.") });
    }
    Ok(())
}

/// PowerShell that removes `task` and its Startup shortcut.
pub fn sign_in_off_script(task: &str) -> String {
    format!(
        "Unregister-ScheduledTask -TaskName {t} -Confirm:$false -ErrorAction SilentlyContinue\nRemove-Item -Force -ErrorAction SilentlyContinue {shortcut}",
        t = psq(task),
        shortcut = startup_shortcut(&format!("{task}.lnk")),
    )
}

/// Whether the hub is set up as a service here (Linux and macOS), from its files alone.
pub fn hub_set_up() -> bool {
    let s = spec(Role::Hub);
    unit_file(s.unit).exists() || plist_file(s.label).exists() || old_unit_for(Role::Hub).is_some()
}

/// PowerShell that stops the other copies of this program doing `role`'s job (every job when
/// none is given), under either program name and renamed aside by an update or not. Never this
/// process. tether-node's installers started the hub connection with no arguments.
fn stop_copies(sys: &Sys, role: Option<Role>) -> String {
    let line = "(Get-CimInstance Win32_Process -Filter \"ProcessId=$($_.Id)\").CommandLine";
    let job = match role {
        Some(Role::Node) => format!(" -and {line} -notmatch '\\s\"?(link|hub|migrate|update|tray|hidden)\"?(\\s|$)'"),
        Some(r) => format!(" -and {line} -match '\\s\"?{}\"?\\s*$'", spec(r).args[0]),
        None => String::new(),
    };
    // the link's tunnel (cloudflared from ~/.inyon/bin), in case the link's own job didn't take it along
    let tunnel = if matches!(role, None | Some(Role::Link)) { format!("\n{}", stop_tunnel_script()) } else { String::new() };
    format!(
        "Get-Process -Name '{NAME}', '{ENGINE}', 'tether-node' -ErrorAction SilentlyContinue | Where-Object {{ $_.Id -ne {}{job} }} | Stop-Process -Force -ErrorAction SilentlyContinue{tunnel}",
        sys.pid
    )
}

/// PowerShell that stops the link's tunnel: cloudflared running from ~/.inyon/bin.
fn stop_tunnel_script() -> String {
    format!(
        "Get-CimInstance Win32_Process -Filter \"Name='cloudflared.exe'\" | Where-Object {{ $_.ExecutablePath -like {} }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
        psq(&format!("{}\\*", path_str(&bin_dir())))
    )
}

/// Ends `role`'s logon task (Windows), so Task Scheduler doesn't count the stop as a failure to
/// start it again from.
pub fn end_task(sys: &Sys, role: Role) {
    if sys.platform != Platform::Windows {
        return;
    }
    if let Some(task) = task_name(role) {
        run(&["schtasks.exe", "/End", "/TN", task]);
    }
}

/// Stops the link's tunnel once the link itself has stopped (Windows, where a child process
/// outlives its parent). The link's own job takes cloudflared with it from 0.20; this catches a
/// tunnel an older link left, and is what Restart does too.
pub fn stop_tunnel(sys: &Sys, role: Role) {
    if sys.platform == Platform::Windows && role == Role::Link {
        ps(&stop_tunnel_script());
    }
}

/// Quit, from the tray icon or the Inyon window (Windows): ends the task, the engine (`pid`) and
/// then its tunnel, so nothing keeps answering for a link that has stopped. When the Inyon app
/// runs, it stops the job itself and the task is left alone: one task's process is the app, and
/// ending it would end everything else the app runs too.
pub fn quit(sys: &Sys, role: Role, pid: Option<u32>) {
    if crate::app::running(sys.proc).is_some() {
        let _ = crate::app::ask(sys.proc, crate::app::Do::Stop, Some(role), None, 10_000);
    } else {
        end_task(sys, role);
    }
    if let Some(pid) = pid.filter(|p| *p != sys.pid && sys.proc.alive(*p)) {
        sys.proc.signal(pid, Signal::Kill);
    }
    stop_tunnel(sys, role);
}

/// Stops `role` now without changing whether it starts at sign-in: its user service (systemctl
/// stop, launchctl unload without -w), a copy started in the background, or on Windows its
/// task and the engine an older version runs outside the app (`pid`). True when it stopped one.
pub fn stop_now(sys: &Sys, role: Role, pid: Option<u32>) -> bool {
    let s = spec(role);
    let mut did = false;
    match sys.platform {
        Platform::Linux => {
            for unit in [Some(s.unit), old_unit_for(role)].into_iter().flatten() {
                if unit_file(unit).exists() && run(&["systemctl", "--user", "is-active", "--quiet", &format!("{unit}.service")]).ok {
                    did |= run(&["systemctl", "--user", "stop", &format!("{unit}.service")]).ok;
                }
            }
            if pid_file(role).exists() {
                did |= stop_background(sys, role).is_ok();
            }
        }
        Platform::Darwin => {
            let file = plist_file(s.label);
            if file.exists() {
                did |= run(&["launchctl", "unload", &path_str(&file)]).ok;
            }
        }
        Platform::Windows => {
            if pid.is_some() {
                end_task(sys, role);
            }
        }
        Platform::Other => {}
    }
    if let Some(pid) = pid.filter(|p| *p != sys.pid && sys.proc.alive(*p)) {
        sys.proc.signal(pid, Signal::Term);
        sys.proc.sleep_ms(500);
        if sys.proc.alive(pid) {
            sys.proc.signal(pid, Signal::Kill);
        }
        did = true;
    }
    stop_tunnel(sys, role);
    did
}

/// PowerShell that stops every program running from ~/.inyon/bin but this one (the engines, the
/// tray icons, the tunnel, the shell scripts' busybox), so uninstall can delete the folder.
fn stop_everything(sys: &Sys) -> String {
    format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.ProcessId -ne {} -and ($_.ExecutablePath -like {} -or $_.Name -in 'tether-node.exe') }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}\nStart-Sleep -Milliseconds 500",
        sys.pid,
        psq(&format!("{}\\*", path_str(&bin_dir())))
    )
}

/// Stops every program running from ~/.inyon/bin but this one (Windows only), so uninstall can
/// delete them: an open .exe can't be deleted there.
pub fn stop_programs(sys: &Sys) {
    if sys.platform == Platform::Windows {
        ps(&stop_everything(sys));
    }
}

/// Deletes `files` and then `dir` once this process has ended, from a hidden PowerShell that
/// waits for it (Windows only): a running .exe can't delete itself.
pub fn delete_after_exit(sys: &Sys, files: &[PathBuf], dir: &Path) {
    if sys.platform != Platform::Windows || files.is_empty() {
        return;
    }
    let list = files.iter().map(|f| psq(&path_str(f))).collect::<Vec<_>>().join(", ");
    let script = format!(
        "Wait-Process -Id {} -Timeout 3600 -ErrorAction SilentlyContinue\nRemove-Item -Force -ErrorAction SilentlyContinue -LiteralPath {list}\nif (-not (Get-ChildItem -Force -Recurse -File -LiteralPath {d} -ErrorAction SilentlyContinue)) {{ Remove-Item -Force -Recurse -ErrorAction SilentlyContinue -LiteralPath {d} }}",
        sys.pid,
        d = psq(&path_str(dir))
    );
    let mut c = std::process::Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sys::quiet(&mut c);
    let _ = c.spawn();
}

/// The Windows Firewall rule that lets phones on your network and VPN reach the link.
pub const FIREWALL_RULE: &str = "Inyon link";

/// PowerShell that adds the firewall rule for the link on `port`: inbound TCP to the engine only,
/// and only from the local network and the VPN ranges (Tailscale and NetBird use 100.64.0.0/10).
pub fn firewall_rule_script(port: u16) -> String {
    format!(
        "Remove-NetFirewallRule -DisplayName {n} -ErrorAction SilentlyContinue\nNew-NetFirewallRule -DisplayName {n} -Description 'Lets phones on your network and VPN reach Inyon' -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port} -Program {prog} -RemoteAddress 'LocalSubnet', '100.64.0.0/10', 'fd7a:115c:a1e0::/48' -Profile Any | Out-Null",
        n = psq(FIREWALL_RULE),
        prog = psq(&path_str(&crate::paths::installed_engine()))
    )
}

/// Whether the firewall rule is there for `port` (Windows).
pub fn firewall_allows(port: u16) -> bool {
    ps(&format!(
        "$r = Get-NetFirewallRule -DisplayName {} -ErrorAction SilentlyContinue | Select-Object -First 1\nif ($r -and $r.Enabled -eq 'True' -and (($r | Get-NetFirewallPortFilter).LocalPort -eq '{port}')) {{ exit 0 }} else {{ exit 1 }}",
        psq(FIREWALL_RULE)
    ))
    .ok
}

/// Runs `script` as administrator: Windows asks first (UAC), or runs it straight away for an
/// administrator's elevated session. False when it was refused or couldn't ask.
fn as_admin(script: &str) -> bool {
    ps(&as_admin_command(script)).ok
}

/// What `as_admin` runs. The script goes inside the elevated command itself (-EncodedCommand), not
/// in a file: any program running as you could rewrite a file between writing it and the yes on
/// the UAC prompt, and have its own script run as administrator.
fn as_admin_command(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!(
        "$ErrorActionPreference='Stop'\ntry {{ $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile -NonInteractive -EncodedCommand {encoded}'; exit $p.ExitCode }} catch {{ exit 1 }}"
    )
}

/// Windows Firewall lets nothing in to a program it hasn't been told about, and VPNs such as
/// NetBird count as public networks, so phones on the LAN or the VPN can't reach the link until
/// a rule allows it (the tunnel goes out, so it works either way). Adds the rule, which needs an
/// administrator's yes, and says how it went (Windows only).
pub fn allow_link_through_firewall(sys: &Sys, port: u16) -> Option<String> {
    if sys.platform != Platform::Windows {
        return None;
    }
    if firewall_allows(port) {
        return Some("✓ Windows Firewall lets phones on your network and VPN reach this computer".into());
    }
    println!("Windows asks for an administrator's permission to let phones on your network and VPN through the firewall…");
    if as_admin(&firewall_rule_script(port)) && firewall_allows(port) {
        return Some(format!("✓ Windows Firewall lets phones on your network and VPN reach this computer (rule \"{FIREWALL_RULE}\")"));
    }
    Some(format!(
        "! Windows Firewall blocks phones on your network and VPN, so they connect through the tunnel only. To let them in, run this in PowerShell as administrator:\n  {}",
        firewall_command(port)
    ))
}

/// The one line that adds the firewall rule, for a person to run in PowerShell as administrator.
pub fn firewall_command(port: u16) -> String {
    firewall_rule_script(port).lines().last().unwrap_or_default().to_string()
}

/// Takes the firewall rule away again (Windows), asking for an administrator's yes when it's there.
pub fn remove_firewall_rule(sys: &Sys) -> Option<String> {
    if sys.platform != Platform::Windows {
        return None;
    }
    let there = ps(&format!("if (Get-NetFirewallRule -DisplayName {} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}", psq(FIREWALL_RULE))).ok;
    if !there {
        return None;
    }
    let script = format!("Remove-NetFirewallRule -DisplayName {}", psq(FIREWALL_RULE));
    if as_admin(&script) {
        Some(format!("removed the firewall rule \"{FIREWALL_RULE}\""))
    } else {
        Some(format!("the firewall rule \"{FIREWALL_RULE}\" stays; remove it in PowerShell as administrator: {script}"))
    }
}

/// No user services (a container, a minimal server): start it detached now, and from cron at
/// boot when cron is there. Our crontab line is marked so it can be found and removed again.
/// Each start writes its PID to ~/.inyon/node/<role>.pid, so uninstall can stop that process and
/// no other.
fn background(sys: &Sys, role: Role, args: &[String]) -> Installed {
    let log = private_log(role);
    // installing again replaces the copy started last time instead of adding another
    if let Err(note) = stop_background(sys, role) {
        return failed(note);
    }
    let line =
        format!("nohup {} >> {} 2>&1 < /dev/null & echo $! > {}", args.iter().map(|a| shq(a)).collect::<Vec<_>>().join(" "), shq(&path_str(&log)), shq(&path_str(&pid_file(role))));
    if !run(&["sh", "-c", &line]).ok {
        return failed("this system has no user services and inyon couldn't start in the background");
    }
    let again = match role {
        Role::Hub => "`inyon hub start`",
        Role::Link => "`inyon pair`",
        Role::Node => "`inyon service install node`",
    };
    if which("crontab").is_none() {
        return ok(format!("a background process (no user services or cron here, so run {again} again after a restart)"));
    }
    let current = run(&["crontab", "-l"]).out;
    let mark = cron_mark(role);
    let mut kept: Vec<String> = current.lines().filter(|l| !l.is_empty() && !l.contains(&mark) && !l.to_lowercase().starts_with("no crontab")).map(String::from).collect();
    kept.push(format!("@reboot {line} {mark}"));
    kept.push(String::new());
    let r = run_with_stdin(&["crontab", "-"], Some(&kept.join("\n")));
    if r.ok {
        ok("a background process that cron starts again after a restart")
    } else {
        ok(format!("a background process (cron refused the restart entry, so run {again} again after a restart)"))
    }
}

/// True while `pid` runs this program as `role` (a PID can be reused once it has exited).
pub fn is_role_process(sys: &Sys, pid: u32, role: Role) -> bool {
    if pid <= 1 {
        return false;
    }
    let Some(args) = sys.proc.cmdline(pid) else {
        return false;
    };
    if !args.iter().any(|a| {
        let b = basename(a);
        b.starts_with("inyon") || b.starts_with("tether-node")
    }) {
        return false;
    }
    let last = args.last().map(String::as_str).unwrap_or("");
    // tether-node's installers started the hub connection with no arguments
    if role == Role::Node {
        return last == "connect" || !["link", "hub", "migrate", "update"].contains(&last);
    }
    last == spec(role).args[spec(role).args.len() - 1]
}

fn stop_pid(sys: &Sys, pid: u32, role: Role) -> Result<(), String> {
    let stop_with = |sig: Signal, ms: u64| {
        sys.proc.signal(pid, sig);
        let mut waited = 0;
        while waited < ms && is_role_process(sys, pid, role) {
            sys.proc.sleep_ms(100);
            waited += 100;
        }
    };
    if is_role_process(sys, pid, role) {
        stop_with(Signal::Term, 3000);
    }
    if is_role_process(sys, pid, role) {
        stop_with(Signal::Kill, 1000);
    }
    if is_role_process(sys, pid, role) {
        return Err(format!("the background {role} (PID {pid}) didn't stop"));
    }
    Ok(())
}

/// Stops the background process recorded for `role`, after checking it's still that job.
fn stop_background(sys: &Sys, role: Role) -> Result<(), String> {
    let file = pid_file(role);
    let Some(pid) = fs::read_to_string(&file).ok().and_then(|t| t.trim().parse::<u32>().ok()) else {
        return Ok(());
    };
    stop_pid(sys, pid, role)?;
    let _ = fs::remove_file(&file);
    Ok(())
}

/// Removes the crontab lines carrying `mark`; says why when crontab refuses.
fn drop_cron(mark: &str) -> Option<String> {
    which("crontab")?;
    let current = run(&["crontab", "-l"]).out;
    if !current.contains(mark) {
        return None;
    }
    let kept: Vec<&str> = current.lines().filter(|l| !l.is_empty() && !l.contains(mark) && !l.to_lowercase().starts_with("no crontab")).collect();
    let text = if kept.is_empty() { String::new() } else { format!("{}\n", kept.join("\n")) };
    if run_with_stdin(&["crontab", "-"], Some(&text)).ok {
        None
    } else {
        Some("its restart entry couldn't be removed from your crontab (crontab -e)".to_string())
    }
}

/// Stops and removes `role`'s service (pairings and keys stay; `inyon forget` unpairs phones).
pub fn uninstall_service(sys: &Sys, role: Role) -> Installed {
    let s = spec(role);
    match sys.platform {
        Platform::Linux => {
            let unit = format!("{}.service", s.unit);
            // stopped last: run from inside the service (a node the hub removed takes itself
            // down), the stop ends this process too, and everything else must be done by then
            run(&["systemctl", "--user", "disable", &unit]);
            let _ = fs::remove_file(unit_file(s.unit));
            run(&["systemctl", "--user", "daemon-reload"]);
            let mut problems = Vec::new();
            if let Some(note) = drop_cron(&cron_mark(role)) {
                problems.push(note);
            }
            if let Err(note) = stop_background(sys, role) {
                problems.push(note);
            }
            run(&["systemctl", "--user", "stop", &unit]);
            if !problems.is_empty() {
                return failed(problems.join("; "));
            }
            ok("the background service is removed")
        }
        Platform::Darwin => {
            let file = plist_file(s.label);
            run(&["launchctl", "unload", "-w", &path_str(&file)]);
            let _ = fs::remove_file(&file);
            ok("the login item is removed")
        }
        Platform::Windows => {
            let Some(task) = s.task else {
                return failed("nothing to remove");
            };
            // the app stops running it (and ends, if that was all it ran). Its task isn't stopped
            // then: that task's process may be the app, running the other job as well
            let app = crate::app::running(sys.proc).is_some();
            if app {
                let _ = crate::app::ask(sys.proc, crate::app::Do::Stop, Some(role), None, 10_000);
            }
            let end = if app { String::new() } else { format!("Stop-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue; ", psq(task)) };
            ps(&format!(
                // unregistered before and after the stop: run from the task itself, the stop may end this
                "Unregister-ScheduledTask -TaskName {t} -Confirm:$false -ErrorAction SilentlyContinue; {end}Unregister-ScheduledTask -TaskName {t} -Confirm:$false -ErrorAction SilentlyContinue\nRemove-Item -Force -ErrorAction SilentlyContinue {shortcut}\n{stop}",
                t = psq(task),
                shortcut = startup_shortcut(&format!("{task}.lnk")),
                stop = stop_copies(sys, Some(role))
            ));
            // said only once it's true: a task that stays would start the engine again at sign-in
            if ps(&format!("if (Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}", psq(task))).ok {
                return failed(format!("the task \"{task}\" couldn't be removed; delete it in Task Scheduler"));
            }
            ok("the logon task is removed")
        }
        Platform::Other => failed("nothing to remove"),
    }
}

/// Restarts `role`'s service so a new file runs. `cmd` is how the installed program runs. A job
/// still set up under its name from before the rename to Inyon is restarted under that name.
pub fn restart_service(sys: &Sys, role: Role, cmd: &[String]) -> Installed {
    let s = spec(role);
    let o = old(role);
    match sys.platform {
        Platform::Linux => {
            for unit in [Some(s.unit), old_unit_for(role)].into_iter().flatten() {
                if !unit_file(unit).exists() {
                    continue;
                }
                let r = run(&["systemctl", "--user", "restart", &format!("{unit}.service")]);
                return if r.ok { ok(format!("systemctl --user restart {unit}")) } else { failed(if r.out.is_empty() { "systemctl refused".to_string() } else { r.out }) };
            }
        }
        Platform::Darwin => {
            for label in [Some(s.label), o.label].into_iter().flatten() {
                let file = plist_file(label);
                if !file.exists() {
                    continue;
                }
                if run(&["launchctl", "kickstart", "-k", &format!("gui/{}/{label}", sys::uid())]).ok {
                    return ok(format!("launchctl kickstart {label}"));
                }
                let f = path_str(&file);
                run(&["launchctl", "unload", &f]);
                let r = run(&["launchctl", "load", "-w", &f]);
                return if r.ok { ok(format!("launchctl load {label}")) } else { failed(if r.out.is_empty() { "launchctl refused".to_string() } else { r.out }) };
            }
        }
        Platform::Windows => {
            if let Some(task) = s.task {
                // the Inyon app runs it: the app starts it again (a new version of the app itself first)
                if crate::app::running(sys.proc).is_some() {
                    return match crate::app::current_relaunched(sys.proc) {
                        // the new version of the app starts everything it ran again
                        Some((a, true)) if a.runs(role) => ok("the new Inyon app started it again"),
                        Some(_) => match crate::app::ask(sys.proc, crate::app::Do::Restart, Some(role), None, 10_000) {
                            Ok(r) if r.ok => ok("the Inyon app started it again"),
                            Ok(r) => failed(r.message),
                            Err(e) => failed(e),
                        },
                        None => failed("the Inyon app didn't come back after the update (~/.inyon/node/app.log says why)"),
                    };
                }
                // inyon's own task is registered again, so one set up by an earlier version
                // opens the Inyon app from now on (no console window at sign-in)
                let own = format!(
                    "if ((Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue) -or (Test-Path {})) {{ exit 0 }} else {{ exit 1 }}",
                    psq(task),
                    startup_shortcut(&format!("{task}.lnk"))
                );
                if ps(&own).ok {
                    let again = Sys { platform: sys.platform, proc: sys.proc, pid: sys.pid, program: cmd.to_vec() };
                    return install_service(&again, role, true);
                }
                // a running copy of the old program (renamed aside or not) is stopped, never this
                // process. Started without a task (install.ps1's Startup shortcut), it's started
                // the same way.
                let exe = cmd.first().map(String::as_str).unwrap_or("");
                let tasks = [Some(task), o.task].into_iter().flatten().map(psq).collect::<Vec<_>>().join(", ");
                let start = if role == Role::Node && basename(exe).starts_with("tether-node") {
                    "Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe) -WindowStyle Hidden".to_string()
                } else {
                    format!("Start-Process -FilePath $exe -ArgumentList {} -WindowStyle Hidden", s.args.iter().map(|a| psq(a)).collect::<Vec<_>>().join(", "))
                };
                let r = ps(&format!(
                    "$ErrorActionPreference='Stop'\n$exe = {}\n$t = {tasks} | ForEach-Object {{ Get-ScheduledTask -TaskName $_ -ErrorAction SilentlyContinue }} | Select-Object -First 1\nif ($t) {{ Stop-ScheduledTask -TaskName $t.TaskName -ErrorAction SilentlyContinue }}\n{}\nStart-Sleep -Milliseconds 500\nif ($t) {{ Start-ScheduledTask -TaskName $t.TaskName }} else {{ {start} }}",
                    psq(exe),
                    stop_copies(sys, Some(role))
                ));
                return if r.ok { ok("restarted its logon task") } else { failed(r.out.lines().next().filter(|l| !l.is_empty()).unwrap_or("Task Scheduler refused").to_string()) };
            }
        }
        Platform::Other => {}
    }
    // no service manager: a process started in the background (nohup), recorded by its pid
    if pid_file(role).exists() {
        let args: Vec<String> = cmd.iter().cloned().chain(s.args.iter().map(|a| a.to_string())).collect();
        return background(sys, role, &args);
    }
    if role == Role::Node {
        return restart_node_process(sys, cmd);
    }
    failed("no background service or process found")
}

/// install.sh's fallback before the rename: the node started with nohup, recorded in node.json.
fn restart_node_process(sys: &Sys, cmd: &[String]) -> Installed {
    let pid = fs::read_to_string(node_dir().join("node.json")).ok().and_then(|t| serde_json::from_str::<crate::state::Running>(&t).ok()).and_then(|r| r.pid);
    let Some(pid) = pid.filter(|p| is_role_process(sys, *p, Role::Node)) else {
        return failed("no background service or process found");
    };
    if let Err(note) = stop_pid(sys, pid, Role::Node) {
        return failed(note);
    }
    let args: Vec<String> = cmd.iter().cloned().chain(spec(Role::Node).args.iter().map(|a| a.to_string())).collect();
    background(sys, Role::Node, &args)
}

/// The systemd unit tether-node made for `role`, if this machine still has it. Both of its
/// installers named it tether-node.service; the phone link's runs `… link`.
fn old_unit_for(role: Role) -> Option<&'static str> {
    let unit = old(role).unit?;
    let text = fs::read_to_string(unit_file(unit)).ok()?;
    if role == Role::Hub {
        return Some(unit);
    }
    let exec = text.lines().find_map(|l| l.strip_prefix("ExecStart=")).map(str::trim).unwrap_or("");
    let last = exec.rsplit(char::is_whitespace).next().unwrap_or("").trim_matches('"');
    let runs_link = last == "link";
    if runs_link == (role == Role::Link) {
        Some(unit)
    } else {
        None
    }
}

/// The jobs still set up under their names from before the rename to Inyon. A hub installed with
/// git clone keeps its unit (its own update restarts either name), so it isn't listed.
pub fn old_services(sys: &Sys) -> Vec<Role> {
    let mut found = Vec::new();
    for role in [Role::Link, Role::Node] {
        let o = old(role);
        let there = match sys.platform {
            Platform::Linux => old_unit_for(role).is_some() || o.cron.is_some_and(|c| which("crontab").is_some() && run(&["crontab", "-l"]).out.contains(c)),
            Platform::Darwin => o.label.is_some_and(|l| plist_file(l).exists()),
            Platform::Windows => match o.task {
                Some(task) => {
                    let startup = o.shortcut.map(|s| format!("(Test-Path {})", startup_shortcut(s))).unwrap_or_else(|| "$false".to_string());
                    ps(&format!("if ((Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue) -or {startup}) {{ exit 0 }} else {{ exit 1 }}", psq(task))).ok
                }
                None => false,
            },
            Platform::Other => false,
        };
        if there {
            found.push(role);
        }
    }
    found
}

/// Moves `role` from the service tether-node set up to inyon's own. The old one is stopped
/// first, so two copies never run at once, then the new one is set up and started, and only
/// then is the old one removed. If the new one can't be set up, the old one is started again.
/// Run from a process of its own (see detach): stopping the old service ends everything it started.
pub fn move_old_service(sys: &Sys, role: Role) -> Installed {
    let o = old(role);
    match sys.platform {
        Platform::Linux => {
            let unit = old_unit_for(role);
            if let Some(u) = unit {
                run(&["systemctl", "--user", "stop", &format!("{u}.service")]);
            } else {
                // the nohup fallback: the old copy recorded its PID in link.pid or node.json
                stop_old_background(sys, role);
            }
            let r = install_service(sys, role, true);
            if !r.ok {
                if let Some(u) = unit {
                    run(&["systemctl", "--user", "start", &format!("{u}.service")]);
                }
                return r;
            }
            if let Some(u) = unit {
                run(&["systemctl", "--user", "disable", &format!("{u}.service")]);
                let _ = fs::remove_file(unit_file(u));
                run(&["systemctl", "--user", "daemon-reload"]);
            }
            if let Some(c) = o.cron {
                drop_cron(c);
            }
            r
        }
        Platform::Darwin => {
            let Some(label) = o.label else {
                return failed("nothing to move");
            };
            let file = path_str(&plist_file(label));
            run(&["launchctl", "unload", "-w", &file]);
            let r = install_service(sys, role, true);
            if !r.ok {
                run(&["launchctl", "load", "-w", &file]);
                return r;
            }
            let _ = fs::remove_file(&file);
            r
        }
        Platform::Windows => {
            let Some(task) = o.task else {
                return failed("nothing to move");
            };
            ps(&format!("Stop-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue\n{}", psq(task), stop_copies(sys, Some(role))));
            let r = install_service(sys, role, true);
            if !r.ok {
                ps(&format!("Start-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue", psq(task)));
                return r;
            }
            let remove = o.shortcut.map(|s| format!("Remove-Item -Force -ErrorAction SilentlyContinue {}", startup_shortcut(s))).unwrap_or_default();
            ps(&format!("Unregister-ScheduledTask -TaskName {} -Confirm:$false -ErrorAction SilentlyContinue\n{remove}", psq(task)));
            r
        }
        Platform::Other => failed("nothing to move"),
    }
}

/// The old copy started with nohup where there were no user services.
fn stop_old_background(sys: &Sys, role: Role) {
    let pid = match role {
        Role::Link => fs::read_to_string(node_dir().join("link.pid")).ok().and_then(|t| t.trim().parse::<u32>().ok()),
        _ => fs::read_to_string(node_dir().join("node.json")).ok().and_then(|t| serde_json::from_str::<crate::state::Running>(&t).ok()).and_then(|r| r.pid),
    };
    if let Some(pid) = pid.filter(|p| *p != sys.pid) {
        let _ = stop_pid(sys, pid, role);
    }
}

/// What the phone needs on this machine besides the link: tmux (psmux on Windows). The message
/// says how to install it, when it's missing.
/// How tmux (psmux on Windows) is installed and removed here, with the package manager's name
/// for the question setup asks.
pub struct MuxPackage {
    pub manager: &'static str,
    pub install: &'static [&'static str],
    pub remove: &'static [&'static str],
    /// the commands start with sudo, so they need a terminal for the password
    pub sudo: bool,
}

/// The package manager that can install tmux (psmux on Windows) on this system, if one is here.
pub fn mux_package(platform: Platform) -> Option<MuxPackage> {
    let pkg = |manager, install, remove, sudo| Some(MuxPackage { manager, install, remove, sudo });
    match platform {
        Platform::Windows => {
            if which("winget").is_some() {
                pkg(
                    "winget",
                    &["winget", "install", "--id", "marlocarlo.psmux", "--exact", "--accept-source-agreements", "--accept-package-agreements"],
                    &["winget", "uninstall", "--id", "marlocarlo.psmux", "--exact"],
                    false,
                )
            } else if which("cargo").is_some() {
                pkg("cargo", &["cargo", "install", "psmux"], &["cargo", "uninstall", "psmux"], false)
            } else {
                None
            }
        }
        Platform::Darwin => {
            if which("brew").is_some() || Path::new("/opt/homebrew/bin/brew").exists() {
                pkg("Homebrew", &["brew", "install", "tmux"], &["brew", "uninstall", "tmux"], false)
            } else {
                None
            }
        }
        Platform::Linux => {
            let sudo = sys::uid() != 0;
            if which("apt-get").is_some() {
                pkg("apt", &["sudo", "apt-get", "install", "-y", "tmux"], &["sudo", "apt-get", "remove", "-y", "tmux"], sudo)
            } else if which("dnf").is_some() {
                pkg("dnf", &["sudo", "dnf", "install", "-y", "tmux"], &["sudo", "dnf", "remove", "-y", "tmux"], sudo)
            } else if which("pacman").is_some() {
                pkg("pacman", &["sudo", "pacman", "-S", "--noconfirm", "tmux"], &["sudo", "pacman", "-R", "--noconfirm", "tmux"], sudo)
            } else if which("zypper").is_some() {
                pkg("zypper", &["sudo", "zypper", "install", "-y", "tmux"], &["sudo", "zypper", "remove", "-y", "tmux"], sudo)
            } else if which("apk").is_some() {
                pkg("apk", &["sudo", "apk", "add", "tmux"], &["sudo", "apk", "del", "tmux"], sudo)
            } else {
                None
            }
        }
        Platform::Other => None,
    }
}

/// Runs a package manager in the user's terminal, so its progress and any sudo prompt show.
fn run_package_command(cmd: &[&str], sudo: bool) -> Result<(), String> {
    let mut cmd = cmd;
    // root doesn't go through sudo
    if !sudo && cmd.first() == Some(&"sudo") {
        cmd = &cmd[1..];
    }
    let (prog, args) = cmd.split_first().ok_or("nothing to run")?;
    // in a terminal its progress shows there; from the Inyon window (no console) no window opens for it
    let mut c = std::process::Command::new(prog);
    c.args(args);
    sys::console_child(&mut c);
    let status = c.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("exit code {}", status.code().unwrap_or(1)))
    }
}

fn mux_name(platform: Platform) -> &'static str {
    if platform == Platform::Windows {
        "psmux (tmux for Windows)"
    } else {
        "tmux"
    }
}

/// What setup installed on the user's behalf (installed.json), so uninstall can offer to take
/// it away again and leaves alone what was there before.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstalledByInyon {
    /// the package manager tmux or psmux came from
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<String>,
}

pub fn installed_by_inyon() -> InstalledByInyon {
    fs::read_to_string(node_dir().join("installed.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Installs tmux (psmux on Windows) when it's missing: asked first when `ask`, and only with a
/// terminal when the install needs sudo. The line for setup's summary.
pub fn install_mux(sys: &Sys, ask: bool, interactive: bool) -> String {
    let name = mux_name(sys.platform);
    let Some(missing) = missing_mux(sys.platform) else {
        return format!("✓ {name} is installed");
    };
    let Some(pkg) = mux_package(sys.platform) else {
        return format!("! {missing}");
    };
    if pkg.sudo && !interactive {
        return format!("! {missing}");
    }
    if ask && !crate::ui::ask_yes_no(&format!("{name} isn't installed. Install it now with {}?", pkg.manager), true) {
        return format!("! {missing}");
    }
    println!("Installing {name} with {}…", pkg.manager);
    match run_package_command(pkg.install, pkg.sudo) {
        Ok(()) => {
            let mut rec = installed_by_inyon();
            rec.mux = Some(pkg.manager.to_string());
            let _ = crate::state::write_json("installed.json", &rec);
            let found = missing_mux(sys.platform).is_none();
            format!("✓ Installed {name} with {}{}", pkg.manager, if found { "" } else { " (new terminals find it)" })
        }
        Err(e) => format!("! {name} couldn't be installed with {} ({e}). Install it with: {}", pkg.manager, pkg.install.join(" ")),
    }
}

/// With `inyon uninstall`: tmux or psmux that setup installed goes too when the user says so.
/// Nothing is asked without a terminal, and then it stays. None when setup didn't install it.
pub fn remove_mux(sys: &Sys, ask: bool) -> Option<String> {
    let manager = installed_by_inyon().mux?;
    let name = mux_name(sys.platform);
    let pkg = mux_package(sys.platform).filter(|p| p.manager == manager);
    let Some(pkg) = pkg else {
        return Some(format!("{name} stays (setup installed it with {manager})"));
    };
    if !ask {
        return Some(format!("{name} stays (setup installed it with {manager}; remove it with: {})", pkg.remove.join(" ")));
    }
    if !crate::ui::ask_yes_no(&format!("Setup installed {name} with {manager}. Remove it too?"), false) {
        return Some(format!("{name} stays"));
    }
    match run_package_command(pkg.remove, pkg.sudo) {
        Ok(()) => Some(format!("removed {name}")),
        Err(e) => Some(format!("{name} couldn't be removed with {manager} ({e}); try: {}", pkg.remove.join(" "))),
    }
}

/// Whether a link answers on `port` here, the way the engine's listener does (it says "inyon" to
/// a plain request). The status screen counts a link by this as well as by its recorded pid, so
/// a stale state.json never shows a running link as stopped.
pub fn link_answers(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(700)));
    if s.write_all(b"GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = [0u8; 1024];
    let mut text = String::new();
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        text.push_str(&String::from_utf8_lossy(&buf[..n]));
        if text.contains("\r\n\r\n") && text.len() > 512 {
            break;
        }
    }
    text.starts_with("HTTP/1.") && text.split("\r\n\r\n").nth(1).is_some_and(|body| body.trim() == "inyon")
}

/// The registry key that lists Inyon under Settings → Apps on Windows.
pub const APP_KEY: &str = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inyon";

/// The Start menu shortcut that opens the Inyon app.
pub fn start_menu_shortcut() -> String {
    "(Join-Path ([Environment]::GetFolderPath('Programs')) 'Inyon.lnk')".to_string()
}

/// The kite, next to the programs: the Start menu shortcut and Settings → Apps show it.
pub fn icon_file() -> PathBuf {
    bin_dir().join("inyon.ico")
}

/// PowerShell that writes the Settings → Apps entry (name, version, icon, where it is, how big,
/// and what Uninstall runs: the app, so no console opens) and the Start menu shortcut "Inyon".
/// `app` is the installed inyon-app.exe.
pub fn app_entry_script(app: &str, icon: &str, version: &str, size_kb: u64) -> String {
    let dir = Path::new(app).parent().map(path_str).unwrap_or_default();
    format!(
        "$k = {k}\nNew-Item -Path $k -Force | Out-Null\n$p = [ordered]@{{ DisplayName = 'Inyon'; DisplayVersion = {v}; Publisher = 'Abhash Chakraborty'; DisplayIcon = {icon}; InstallLocation = {dir}; UninstallString = {u}; QuietUninstallString = {q}; URLInfoAbout = 'https://inyon.tech'; HelpLink = 'https://inyon.tech/docs/troubleshooting/'; InstallDate = (Get-Date -Format yyyyMMdd) }}\nforeach ($n in $p.Keys) {{ Set-ItemProperty -Path $k -Name $n -Value $p[$n] }}\nforeach ($n in 'NoModify', 'NoRepair') {{ Set-ItemProperty -Path $k -Name $n -Value 1 -Type DWord }}\nSet-ItemProperty -Path $k -Name EstimatedSize -Value {size} -Type DWord\n$l = (New-Object -ComObject WScript.Shell).CreateShortcut({lnk})\n$l.TargetPath = {exe}\n$l.WorkingDirectory = $env:USERPROFILE\n$l.IconLocation = {icon}\n$l.Description = 'Pair your phone and see what Inyon runs'\n$l.Save()",
        k = psq(APP_KEY),
        v = psq(version),
        exe = psq(app),
        icon = psq(icon),
        dir = psq(&dir),
        u = psq(&uninstall_command(app)),
        q = psq(&format!("\"{app}\" uninstall --purge --yes")),
        size = size_kb,
        lnk = start_menu_shortcut(),
    )
}

/// What Uninstall in Settings → Apps runs: the app, so no console opens.
fn uninstall_command(app: &str) -> String {
    format!("\"{app}\" uninstall --purge")
}

/// PowerShell that exits 0 when Settings → Apps lists `version` with `app` as its Uninstall, as
/// `app_entry_script` writes it.
pub fn app_entry_current_script(app: &str, version: &str) -> String {
    format!(
        "$e = Get-ItemProperty -Path {} -ErrorAction SilentlyContinue\nif ($e -and $e.DisplayVersion -eq {} -and $e.UninstallString -eq {}) {{ exit 0 }} else {{ exit 1 }}",
        psq(APP_KEY),
        psq(version),
        psq(&uninstall_command(app))
    )
}

/// Lists Inyon again when Settings → Apps names another version than the installed one, or
/// Uninstall doesn't open the app (Windows): what an updater older than 0.20.4 leaves, since it
/// writes its own version once it's done. True when it listed it again.
pub fn repair_app_entry(sys: &Sys) -> bool {
    if sys.platform != Platform::Windows || !installed_binary().is_file() {
        return false;
    }
    let version = crate::app::build_of(&installed_binary()).unwrap_or_else(|| crate::ui::VERSION.to_string());
    let app = crate::app::app_beside(&installed_binary());
    if ps(&app_entry_current_script(&path_str(&app), &version)).ok {
        return false;
    }
    register_app(sys).is_some_and(|l| l.starts_with('✓'))
}

/// Lists Inyon under Settings → Apps and in the Start menu (Windows), once inyon.exe is in place.
/// Done again after an update, for the version. The line for setup's summary, or None when
/// there's nothing to do.
pub fn register_app(sys: &Sys) -> Option<String> {
    if sys.platform != Platform::Windows {
        return None;
    }
    let exe = installed_binary();
    if !exe.is_file() {
        return None;
    }
    let app = match ensure_app(&exe) {
        Ok(a) => a,
        Err(e) => return Some(format!("! Couldn't put the Inyon app in place ({e})")),
    };
    let icon = icon_file();
    let _ = fs::write(&icon, crate::app::ICON);
    let bytes: u64 = fs::read_dir(bin_dir()).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum();
    // the installed program's version: after an update, this program is the one it replaced
    let version = crate::app::build_of(&exe).unwrap_or_else(|| crate::ui::VERSION.to_string());
    if ps(&app_entry_script(&path_str(&app), &path_str(&icon), &version, bytes / 1024)).ok {
        Some("✓ In the Start menu and Settings → Apps as Inyon".into())
    } else {
        Some("! Couldn't list Inyon in the Start menu and Settings → Apps (uninstall with `inyon uninstall`)".into())
    }
}

/// Takes the Settings → Apps entry and the Start menu shortcut away (Windows).
pub fn unregister_app(sys: &Sys) {
    if sys.platform == Platform::Windows {
        ps(&format!(
            "Remove-Item -Path {} -Recurse -Force -ErrorAction SilentlyContinue\nRemove-Item -Force -ErrorAction SilentlyContinue {}",
            psq(APP_KEY),
            start_menu_shortcut()
        ));
    }
}

pub fn missing_mux(platform: Platform) -> Option<String> {
    if platform == Platform::Windows {
        if which("psmux").is_some() || which("tmux").is_some() {
            return None;
        }
        return Some("psmux (tmux for Windows) is missing. Install it with: winget install psmux".to_string());
    }
    if which("tmux").is_some() || ["/opt/homebrew/bin/tmux", "/usr/local/bin/tmux", "/usr/bin/tmux"].iter().any(|p| Path::new(p).exists()) {
        return None;
    }
    if platform == Platform::Darwin {
        return Some(if which("brew").is_some() || Path::new("/opt/homebrew/bin/brew").exists() {
            "tmux is missing. Install it with: brew install tmux".to_string()
        } else {
            "tmux is missing. Install Homebrew from https://brew.sh, then run: brew install tmux".to_string()
        });
    }
    let pm = ["apt-get", "dnf", "pacman", "zypper", "apk"].into_iter().find(|p| which(p).is_some());
    let cmd = match pm {
        Some("apt-get") => Some("sudo apt-get install -y tmux"),
        Some("dnf") => Some("sudo dnf install -y tmux"),
        Some("pacman") => Some("sudo pacman -S tmux"),
        Some("zypper") => Some("sudo zypper install -y tmux"),
        Some("apk") => Some("sudo apk add tmux"),
        _ => None,
    };
    Some(match cmd {
        Some(c) => format!("tmux is missing. Install it with: {c}"),
        None => "tmux is missing. Install it with your package manager.".to_string(),
    })
}

/// Runs `args` (this program and a command) on its own: restarting a service ends everything the
/// service started, the command that asked included. On Linux it becomes a transient systemd
/// unit, on Windows the one-off task `task`, elsewhere a process in a session of its own.
pub fn detach(sys: &Sys, args: &[String], task: &str) -> Installed {
    if sys.platform == Platform::Linux && which("systemd-run").is_some() && run(&["systemctl", "--user", "show-environment"]).ok {
        let mut cmd: Vec<String> = vec!["systemd-run".into(), "--user".into(), "--collect".into(), "--quiet".into()];
        cmd.extend(detached_env());
        cmd.extend(args.iter().cloned());
        let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
        if run(&refs).ok {
            return ok("a transient systemd unit");
        }
    }
    if sys.platform == Platform::Windows {
        let (exe, rest) = args.split_first().expect("a program to run");
        // started through the engine, which has no console, so no window opens for it
        let engine = crate::update::engine_beside(Path::new(exe));
        let (exe, quoted) = if engine.is_file() {
            (
                path_str(&engine),
                std::iter::once("hidden").chain(std::iter::once(exe.as_str())).chain(rest.iter().map(String::as_str)).map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" "),
            )
        } else {
            (exe.clone(), rest.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" "))
        };
        let exe = &exe;
        let r = ps(&format!(
            "$ErrorActionPreference='Stop'\n$a = New-ScheduledTaskAction -Execute {} -Argument {}\n$p = New-ScheduledTaskPrincipal -UserId \"$env:USERDOMAIN\\$env:USERNAME\" -LogonType Interactive -RunLevel Limited\n$s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 30)\nRegister-ScheduledTask -TaskName {task} -Action $a -Principal $p -Settings $s -Force | Out-Null\nStart-ScheduledTask -TaskName {task}",
            psq(exe),
            psq(&quoted),
            task = psq(task)
        ));
        if r.ok {
            return ok("a one-off task");
        }
    }
    let log = node_dir().join("update.log");
    make_private(&node_dir(), &log);
    match sys::spawn_detached(args, &log) {
        Ok(()) => ok("a background process"),
        Err(err) => failed(err.to_string()),
    }
}

/// What a command started by systemd-run takes along, since it gets the service manager's
/// environment instead of this one: the mirror and the hub's folder (each under its INYON_ name,
/// also when it was set under its TETHER_ one).
fn detached_env() -> Vec<String> {
    ["INYON_DOWNLOAD", "INYON_DATA_DIR"].iter().filter_map(|k| crate::paths::env_var(k).map(|val| format!("--setenv={k}={val}"))).collect()
}

/// `inyon update`, started on its own (the app asks for it from inside the service it restarts).
pub fn detach_update(sys: &Sys, cmd: &[String]) -> Installed {
    let mut args = cmd.to_vec();
    args.push("update".into());
    detach(sys, &args, UPDATE_TASK)
}

/// A one-off task removes itself when it's done (Windows only).
pub fn forget_task(sys: &Sys, task: &str) {
    if sys.platform != Platform::Windows {
        return;
    }
    ps(&format!("Unregister-ScheduledTask -TaskName {} -Confirm:$false -ErrorAction SilentlyContinue", psq(task)));
}

/// Which jobs run in the background here: set up as a service, or started and still running.
pub fn active_roles(sys: &Sys) -> Vec<Role> {
    ROLES.iter().copied().filter(|r| has_service(sys, *r) || crate::update::is_running(sys.proc, *r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_log_for_launchd_or_nohup_is_yours_alone_in_a_folder_only_you_can_open() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("inyon-private-log-{}", std::process::id())).join("node");
        let _ = fs::remove_dir_all(dir.parent().unwrap());
        let log = dir.join("hub.log");
        make_private(&dir, &log);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&log)), (0o700, 0o600));
        // one an older version left open to everyone is closed too, and keeps what it holds
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&log, "kept\n").unwrap();
        fs::set_permissions(&log, fs::Permissions::from_mode(0o644)).unwrap();
        make_private(&dir, &log);
        assert_eq!((mode(&dir), mode(&log)), (0o700, 0o600));
        assert_eq!(fs::read_to_string(&log).unwrap(), "kept\n");
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn the_script_run_as_administrator_goes_in_the_command_not_a_file_anyone_could_rewrite() {
        use base64::Engine;
        let script = firewall_rule_script(8765);
        let cmd = as_admin_command(&script);
        assert!(cmd.contains("-Verb RunAs") && cmd.contains("-EncodedCommand "), "{cmd}");
        assert!(!cmd.contains("-File ") && !cmd.contains("admin.ps1"), "{cmd}");
        let b64 = cmd.split("-EncodedCommand ").nth(1).unwrap().split('\'').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let utf16: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&utf16).unwrap(), script);
        // base64 only, so nothing in it can end the quoted argument
        assert!(b64.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)));
    }

    #[test]
    fn the_settings_apps_entry_and_the_start_menu_open_the_app_with_no_console() {
        let s = app_entry_script(r"C:\Users\a\.inyon\bin\inyon-app.exe", r"C:\Users\a\.inyon\bin\inyon.ico", "0.20.3", 90000);
        assert!(s.contains("DisplayName = 'Inyon'"));
        assert!(s.contains("DisplayVersion = '0.20.3'"));
        assert!(s.contains("Publisher = 'Abhash Chakraborty'"));
        assert!(s.contains(r"DisplayIcon = 'C:\Users\a\.inyon\bin\inyon.ico'"));
        // Uninstall runs the app, which asks in a message box: no console window, no --pause
        assert!(s.contains(r#"UninstallString = '"C:\Users\a\.inyon\bin\inyon-app.exe" uninstall --purge'"#));
        assert!(s.contains(r#"QuietUninstallString = '"C:\Users\a\.inyon\bin\inyon-app.exe" uninstall --purge --yes'"#));
        assert!(!s.contains("--pause") && !s.contains(r"\inyon.exe"), "{s}");
        assert!(s.contains("EstimatedSize -Value 90000"));
        assert!(s.contains("GetFolderPath('Programs')) 'Inyon.lnk'"));
        assert!(s.contains(r"$l.TargetPath = 'C:\Users\a\.inyon\bin\inyon-app.exe'"));
        assert!(s.contains(r"$l.IconLocation = 'C:\Users\a\.inyon\bin\inyon.ico'"));
        if cfg!(windows) {
            assert!(s.contains(r"InstallLocation = 'C:\Users\a\.inyon\bin'"));
        }
    }

    #[test]
    fn sign_in_off_removes_the_task_and_the_shortcut() {
        let s = sign_in_off_script("Inyon link");
        assert!(s.contains("Unregister-ScheduledTask -TaskName 'Inyon link' -Confirm:$false"));
        assert!(s.contains("'Inyon link.lnk'"));
        assert!(s.contains("GetFolderPath('Startup')"));
    }

    #[test]
    fn sign_in_is_windows_only() {
        let proc = crate::sys::RealProc;
        let mut sys = Sys::new(&proc);
        sys.platform = Platform::Linux;
        assert!(set_sign_in(&sys, Role::Link, true).unwrap_err().contains("inyon setup"));
    }

    #[test]
    fn the_logon_task_opens_the_app_in_the_background_and_starts_nothing_itself() {
        let s = task_script("Inyon link", r"C:\Users\a\.inyon\bin\inyon-app.exe", Role::Link, "Inyon phone link");
        assert!(s.contains(r"New-ScheduledTaskAction -Execute 'C:\Users\a\.inyon\bin\inyon-app.exe' -Argument 'app --background link'"), "{s}");
        assert!(s.contains("-LogonType Interactive -RunLevel Limited"));
        assert!(s.contains("-RestartCount 3"));
        // the fallback shortcut opens the app too
        assert!(s.contains(r"$l.TargetPath = 'C:\Users\a\.inyon\bin\inyon-app.exe'") && s.contains("$l.Arguments = 'app --background link'"));
        assert!(!s.contains("Start-ScheduledTask") && !s.contains("Start-Process"), "the app starts the job, not the script: {s}");
        assert!(!s.contains("inyon-engine"), "{s}");
        let n = task_script("Inyon node", r"C:\a\inyon-app.exe", Role::Node, "x");
        assert!(n.contains("-Argument 'app --background node'"));
    }

    #[test]
    fn a_link_is_known_by_what_its_port_answers() {
        use std::io::{Read, Write};
        let serve = |body: &'static str| {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            std::thread::spawn(move || {
                if let Ok((mut s, _)) = l.accept() {
                    let mut buf = [0u8; 256];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
                }
            });
            port
        };
        assert!(link_answers(serve("inyon\n")));
        assert!(!link_answers(serve("<html>something else</html>")));
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(!link_answers(closed));
    }

    #[test]
    fn root_skips_sudo_in_a_package_command() {
        // a command that isn't there fails cleanly either way
        assert!(run_package_command(&["sudo", "no-such-package-manager-here", "install"], false).is_err());
        assert!(run_package_command(&[], false).is_err());
    }
}
