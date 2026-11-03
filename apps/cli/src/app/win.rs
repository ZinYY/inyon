//! The app's Win32 side: one process per signed-in user (the mutex Local\InyonApp), a hidden
//! window that receives the tray icon's clicks and the wake-ups from [`super::ask`], the job that
//! holds every process it starts, and the Inyon window ([`crate::desktop::win`]) on a thread of
//! its own, made once and shown or hidden from then on.

use std::collections::BTreeMap;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, SetTimer, TrackPopupMenu, TranslateMessage, MSG, SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use super::{labels, on_close, should_end, take_requests, trace, tray_line, AppState, Close, Do, Request, Supervisor, ADOPT_GRACE_MS};
use crate::paths;
use crate::service::{self, Role};
use crate::sys::{Proc, RealProc, NO_WINDOW};

const HOST: &str = "InyonAppHost";
/// the tray icon was clicked (lparam: the mouse message)
const TRAY: u32 = WM_APP + 1;
/// requests are waiting in the folder
const WAKE: u32 = WM_APP + 2;
/// the Inyon window closed
const WINDOW_CLOSED: u32 = WM_APP + 3;
const TICK: usize = 1;
// the tray menu's items
const OPEN: usize = 1;
const PAIR: usize = 2;
const QUIT: usize = 3;
const PHONES: usize = 4;
const HUB: usize = 5;
const TEMP: usize = 6;
const SETTINGS: usize = 7;
const LAN: usize = 8;
const VPN: usize = 9;
const TUNNEL: usize = 10;
const SIGN_IN: usize = 11;
const UPDATE: usize = 12;
const LOGS: usize = 13;
const START_LINK: usize = 14;
const STOP_LINK: usize = 15;
/// what the app writes to app.json at the least this often, so `inyon status` knows it's alive
const WRITE_MS: f64 = 5_000.0;
/// how long the app waits for what it was asked to start before an empty app counts as done
const SETTLE_MS: f64 = 10_000.0;

/// The app's job: everything it starts ends with it. Quit drops it.
static JOB: Mutex<Option<inyon_engine::winjob::Job>> = Mutex::new(None);

/// How the app was started.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Start {
    /// the jobs to run
    pub roles: Vec<Role>,
    /// started at sign-in: no window
    pub background: bool,
    /// open the window with a pairing code
    pub pair: bool,
    /// a job an older version started outside the app, which the app takes over
    pub adopt: Option<(Role, u32)>,
}

struct Kid {
    child: std::process::Child,
    started: f64,
}

struct Adopted {
    pid: u32,
    since: f64,
}

struct App {
    hwnd: HWND,
    mutex: HANDLE,
    sup: Supervisor,
    kids: BTreeMap<Role, Kid>,
    adopted: BTreeMap<Role, Adopted>,
    icon: NOTIFYICONDATAW,
    icon_shown: bool,
    taskbar_created: u32,
    started: f64,
    written: f64,
    settle_until: f64,
    last: Option<AppState>,
}

thread_local! {
    static APP: std::cell::RefCell<Option<App>> = const { std::cell::RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.borrow_mut().as_mut().map(f))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// inyon.exe next to this program: what the jobs' engine is copied from and what the app's
/// windowless steps run.
fn inyon_exe() -> PathBuf {
    std::env::current_exe().map(|e| super::command_beside(&e)).ok().filter(|p| p.is_file()).unwrap_or_else(paths::installed_binary)
}

/// Wakes the running app, so it reads a request now instead of within half a second.
pub fn wake() {
    unsafe {
        let host = FindWindowW(wide(HOST).as_ptr(), null());
        if !host.is_null() {
            PostMessageW(host, WAKE, 0, 0);
        }
    }
}

/// Puts `child` in the app's job, when this process is the app.
pub fn hold(child: &std::process::Child) {
    if let Some(job) = JOB.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        job.add(child.as_raw_handle());
    }
}

pub fn run(start: Start) -> i32 {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(null(), 0, wide("Local\\InyonApp").as_ptr());
        if mutex.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            if !mutex.is_null() {
                CloseHandle(mutex);
            }
            return hand_over(&start);
        }
        super::IN_APP.store(true, Ordering::Relaxed);
        trace(&format!("start {} (roles {:?}, background {}, adopt {:?})", crate::ui::VERSION, start.roles, start.background, start.adopt));
        *JOB.lock().unwrap_or_else(|e| e.into_inner()) = inyon_engine::winjob::Job::owner();
        let instance = GetModuleHandleW(null());
        let class = wide(HOST);
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // a window that's never shown: it gets the icon's clicks and Explorer's TaskbarCreated
        let hwnd = CreateWindowExW(0, class.as_ptr(), wide("Inyon").as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), instance, null());
        if hwnd.is_null() {
            trace(&format!("no host window: error {}", GetLastError()));
            return 1;
        }
        let mut icon: NOTIFYICONDATAW = std::mem::zeroed();
        icon.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        icon.hWnd = hwnd;
        icon.uID = 1;
        icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        icon.uCallbackMessage = TRAY;
        icon.hIcon = crate::tray::win::icon_sized(GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        let now = crate::state::now_ms();
        let mut sup = Supervisor::default();
        for r in &start.roles {
            sup.want(*r);
        }
        let mut adopted = BTreeMap::new();
        if let Some((role, pid)) = start.adopt {
            sup.want(role);
            adopted.insert(role, Adopted { pid, since: now });
        }
        let app = App {
            hwnd,
            mutex,
            sup,
            kids: BTreeMap::new(),
            adopted,
            icon,
            icon_shown: false,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            started: now,
            written: 0.0,
            settle_until: now + SETTLE_MS,
            last: None,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        // the engine is a windowless copy of inyon.exe: made again when inyon.exe changed
        if let Err(e) = crate::engine::refresh_copy(&inyon_exe()) {
            trace(&format!("! no engine copy: {e}"));
        }
        // an updater older than 0.20.4 writes its own version into Settings → Apps when it's
        // done, after this app has started, and restarting two jobs can take it past a minute:
        // put this version's entry back after 1, 3 and 10 minutes
        std::thread::spawn(|| {
            for wait in [60, 120, 420] {
                std::thread::sleep(std::time::Duration::from_secs(wait));
                let proc = RealProc;
                if crate::service::repair_app_entry(&crate::service::Sys::new(&proc)) {
                    trace("Settings → Apps named an older version: listed this one again");
                }
            }
        });
        crate::desktop::win::set_host(hwnd as usize, WINDOW_CLOSED);
        tick();
        SetTimer(hwnd, TICK, 500, None);
        // at sign-in the window opens only when Settings asks for it
        let at_sign_in = start.background && crate::desktop::load_settings().open_at_sign_in;
        if !start.background || start.pair || at_sign_in {
            open_window(start.pair);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        end();
        trace("ended");
        0
    }
}

/// Another start while the app runs: what it was started for goes to the running app, and its
/// window comes to the front.
fn hand_over(start: &Start) -> i32 {
    let proc = RealProc;
    // the app that holds the mutex may have only just started
    let mut state = None;
    for _ in 0..50 {
        state = super::running(&proc);
        if state.is_some() {
            break;
        }
        proc.sleep_ms(100);
    }
    let Some(state) = state else {
        trace("another app holds the mutex but doesn't answer");
        return 1;
    };
    // the app's window may come to the front: this start is what the user clicked
    unsafe { AllowSetForegroundWindow(state.pid) };
    for r in &start.roles {
        let _ = super::ask(&proc, Do::Start, Some(*r), None, 3000);
    }
    if let Some((role, pid)) = start.adopt {
        let _ = super::ask(&proc, Do::Adopt, Some(role), Some(pid), 3000);
    }
    if start.pair {
        let _ = super::ask(&proc, Do::Pair, None, None, 3000);
    } else if !start.background {
        let _ = super::ask(&proc, Do::Show, None, None, 3000);
    }
    0
}

fn open_window(pair: bool) {
    crate::desktop::win::show(None, pair);
}

fn open_page(page: &str) {
    crate::desktop::win::show(Some(page), false);
}

/// Starts `role`'s engine in the app's job.
fn spawn(role: Role) -> Result<Kid, String> {
    let inyon = inyon_exe();
    let engine = service::engine_program(&inyon)?;
    let mut c = Command::new(&engine);
    c.args(service::engine_args(role))
        .env("INYON_APP", std::process::id().to_string())
        .env("INYON_CLI", &inyon)
        .current_dir(paths::home())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(NO_WINDOW);
    let child = c.spawn().map_err(|e| format!("couldn't start {} ({e})", engine.display()))?;
    if !JOB.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|j| j.add(child.as_raw_handle())) {
        trace(&format!("! the {role} isn't in the app's job"));
    }
    Ok(Kid { child, started: crate::state::now_ms() })
}

/// Ends a process the app didn't start (an older version's engine), by its pid.
fn kill_pid(pid: u32) {
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, 0);
            WaitForSingleObject(h, 3000);
            CloseHandle(h);
        }
    }
}

fn stop_kid(mut k: Kid) {
    let _ = k.child.kill();
    let _ = k.child.wait();
}

/// What runs, in the tray's words.
fn running_labels(a: &App) -> Vec<String> {
    let link = a.sup.wants(Role::Link) || a.adopted.contains_key(&Role::Link);
    let tunnel = link && crate::state::link_config().tunnel.unwrap_or(true);
    let node = a.sup.wants(Role::Node) || a.adopted.contains_key(&Role::Node);
    labels(link, tunnel, node, super::pairing())
}

/// Every half second: requests, jobs that ended, jobs due to start, the icon and app.json.
fn tick() {
    let now = crate::state::now_ms();
    for r in take_requests(now) {
        handle(r);
    }
    let proc = RealProc;
    let update_running = crate::update::under_way();
    let Some(due) = with(|a| {
        let ended: Vec<Role> = a
            .kids
            .iter_mut()
            .filter_map(|(r, k)| k.child.try_wait().ok().flatten().map(|s| (*r, s)))
            .map(|(r, s)| {
                trace(&format!("the {r} ended ({s})"));
                r
            })
            .collect();
        for r in ended {
            if let Some(k) = a.kids.remove(&r) {
                let wait = a.sup.ended(r, now, now - k.started);
                if a.sup.wants(r) {
                    trace(&format!("starting the {r} again in {}s", (wait / 1000.0).round()));
                }
            }
        }
        // an older version's engine: gone by itself, or replaced once the update that started it is done
        let mut replace = Vec::new();
        a.adopted.retain(|r, ad| {
            if !proc.alive(ad.pid) {
                trace(&format!("the {r} an older version started (pid {}) has ended", ad.pid));
                return false;
            }
            if super::replace_adopted(ad.since, now, update_running) {
                replace.push((*r, ad.pid));
                return false;
            }
            true
        });
        for (r, pid) in replace {
            trace(&format!("taking the {r} over from the older version's engine (pid {pid})"));
            kill_pid(pid);
        }
        let kids: Vec<Role> = a.kids.keys().copied().collect();
        let adopted: Vec<Role> = a.adopted.keys().copied().collect();
        a.sup.due(now, &|r| kids.contains(&r) || adopted.contains(&r))
    }) else {
        return;
    };
    for role in due {
        match spawn(role) {
            Ok(k) => {
                trace(&format!("started the {role} (pid {})", k.child.id()));
                with(|a| a.kids.insert(role, k));
            }
            Err(e) => {
                trace(&format!("! {e}"));
                with(|a| a.sup.ended(role, now, 0.0));
            }
        }
    }
    refresh_icon();
    write(now, false);
    let end = with(|a| {
        let anything = !a.sup.is_empty() || !a.adopted.is_empty() || super::pairing();
        should_end(crate::desktop::win::visible(), crate::desktop::win::opening(), anything, now < a.settle_until)
    })
    .unwrap_or(false);
    if end {
        trace("nothing runs and no window is open: ending");
        unsafe { PostQuitMessage(0) };
    }
}

/// app.json: what runs and the processes doing it, written when it changes and every few seconds.
fn write(now: f64, force: bool) {
    with(|a| {
        let s = AppState {
            pid: std::process::id(),
            version: crate::ui::VERSION.to_string(),
            started: a.started,
            seen: now,
            running: running_labels(a),
            jobs: super::job_list(a.sup.wanted().into_iter().chain(a.adopted.keys().copied()), |r| {
                a.kids.get(&r).map(|k| k.child.id()).or_else(|| a.adopted.get(&r).map(|ad| ad.pid))
            }),
        };
        let changed = a.last.as_ref().is_none_or(|l| l.running != s.running || l.jobs != s.jobs);
        if force || changed || now - a.written >= WRITE_MS {
            super::write_state(&s);
            a.written = now;
            a.last = Some(s);
        }
    });
}

/// The icon shows while anything runs, with what runs as its tooltip.
fn refresh_icon() {
    with(|a| {
        let running = running_labels(a);
        let show = !running.is_empty() || !a.adopted.is_empty();
        let tip: Vec<u16> = format!("Inyon · {}", tray_line(&running)).encode_utf16().take(a.icon.szTip.len() - 1).collect();
        let same_tip = a.icon.szTip[..tip.len()] == tip[..] && a.icon.szTip[tip.len()] == 0;
        if !same_tip {
            a.icon.szTip = [0; 128];
            a.icon.szTip[..tip.len()].copy_from_slice(&tip);
        }
        unsafe {
            if show && !a.icon_shown {
                a.icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
                a.icon_shown = Shell_NotifyIconW(NIM_ADD, &a.icon) != 0;
            } else if !show && a.icon_shown {
                Shell_NotifyIconW(NIM_DELETE, &a.icon);
                a.icon_shown = false;
            } else if show && !same_tip {
                a.icon.uFlags = NIF_TIP;
                Shell_NotifyIconW(NIM_MODIFY, &a.icon);
            }
        }
    });
}

/// The one-time note when the window closes and the app stays in the notification area.
fn note_once() {
    let file = super::note_file();
    if file.exists() {
        return;
    }
    let _ = crate::paths::ensure_node_dir();
    let _ = std::fs::write(&file, "shown\n");
    with(|a| unsafe {
        let mut n = a.icon;
        n.uFlags = NIF_INFO;
        let title: Vec<u16> = "Inyon is still running".encode_utf16().collect();
        let text: Vec<u16> = "It keeps running here, in the notification area. To stop it, choose Quit in this icon's menu.".encode_utf16().take(n.szInfo.len() - 1).collect();
        n.szInfoTitle[..title.len()].copy_from_slice(&title);
        n.szInfo[..text.len()].copy_from_slice(&text);
        Shell_NotifyIconW(NIM_MODIFY, &n);
    });
}

fn handle(r: Request) {
    let now = crate::state::now_ms();
    let role = r.role();
    trace(&format!("asked: {:?}{}", r.what, role.map(|x| format!(" {x}")).unwrap_or_default()));
    let (ok, message) = match (r.what, role) {
        (Do::Start, Some(role)) => {
            let had = with(|a| {
                let had = a.sup.wants(role);
                a.sup.want(role);
                a.sup.now(role);
                a.settle_until = now + SETTLE_MS;
                had
            })
            .unwrap_or(false);
            (true, if had { format!("The Inyon app runs the {role} already.") } else { format!("The Inyon app runs the {role} from now on.") })
        }
        (Do::Stop, Some(role)) => {
            let (kid, adopted) = with(|a| {
                a.sup.unwant(role);
                (a.kids.remove(&role), a.adopted.remove(&role))
            })
            .unwrap_or((None, None));
            if let Some(k) = kid {
                stop_kid(k);
            }
            if let Some(ad) = adopted {
                kill_pid(ad.pid);
            }
            (true, format!("Stopped the {role}."))
        }
        (Do::Restart, Some(role)) => {
            let (kid, adopted) = with(|a| {
                a.sup.want(role);
                a.sup.now(role);
                a.settle_until = now + SETTLE_MS;
                (a.kids.remove(&role), a.adopted.remove(&role))
            })
            .unwrap_or((None, None));
            if let Some(k) = kid {
                stop_kid(k);
            }
            if let Some(ad) = adopted {
                kill_pid(ad.pid);
            }
            (true, format!("Restarting the {role}."))
        }
        (Do::Adopt, Some(role)) => match r.pid {
            Some(pid) if with(|a| a.kids.contains_key(&role)).unwrap_or(false) => {
                // the app runs its own already: the other copy would only fight it for the port
                kill_pid(pid);
                (true, format!("The Inyon app runs the {role} already."))
            }
            Some(pid) => {
                with(|a| {
                    a.sup.want(role);
                    a.adopted.insert(role, Adopted { pid, since: now });
                });
                (true, format!("The Inyon app looks after the {role} now; it takes it over within {}s.", (ADOPT_GRACE_MS / 1000.0) as u64))
            }
            None => (false, "No process to take over.".into()),
        },
        (Do::Show, _) => {
            open_window(false);
            (true, "Opened the Inyon window.".into())
        }
        (Do::Pair, _) => {
            with(|a| {
                a.sup.want(Role::Link);
                a.settle_until = now + SETTLE_MS;
            });
            open_window(true);
            (true, "Opened the Inyon window with a pairing code.".into())
        }
        (Do::Quit, _) => {
            super::reply(&r.id, true, "Quit Inyon: everything it ran has stopped.");
            quit();
            return;
        }
        (Do::Relaunch, _) => {
            super::reply(&r.id, true, "Starting the new version of the Inyon app.");
            relaunch();
            return;
        }
        (what, None) => (false, format!("{what:?} needs a job (link or node).")),
    };
    super::reply(&r.id, ok, &message);
    tick_soon();
    write(now, true);
}

fn tick_soon() {
    with(|a| unsafe { PostMessageW(a.hwnd, WAKE, 0, 0) });
}

/// Stops everything the app runs: its jobs (with the job object), an older version's engine it
/// looks after, and the link's tunnel in case one outlived its link. Then the app ends.
fn quit() {
    trace("quit");
    stop_all();
    unsafe { PostQuitMessage(0) };
}

fn stop_all() {
    let (kids, adopted) = with(|a| {
        for r in a.sup.wanted() {
            a.sup.unwant(r);
        }
        (std::mem::take(&mut a.kids), std::mem::take(&mut a.adopted))
    })
    .unwrap_or_default();
    // closing the job ends what's in it, the pairings included
    drop(JOB.lock().unwrap_or_else(|e| e.into_inner()).take());
    for (_, k) in kids {
        stop_kid(k);
    }
    for (_, ad) in adopted {
        kill_pid(ad.pid);
    }
    let proc = RealProc;
    let sys = crate::service::Sys::new(&proc);
    service::stop_tunnel(&sys, Role::Link);
}

/// An update replaced the app's file: the new version takes over, with the same jobs and the
/// window open if it was.
fn relaunch() {
    let roles = with(|a| a.sup.wanted()).unwrap_or_default();
    let window = crate::desktop::win::visible();
    trace(&format!("relaunching for the new version with {roles:?}"));
    stop_all();
    // the new app takes the mutex and app.json
    with(|a| unsafe {
        if a.icon_shown {
            Shell_NotifyIconW(NIM_DELETE, &a.icon);
            a.icon_shown = false;
        }
        CloseHandle(a.mutex);
        a.mutex = null_mut();
    });
    super::clear_state(std::process::id());
    let mut c = Command::new(super::installed_app());
    c.arg("app");
    if !window {
        c.arg("--background");
    }
    c.args(roles.iter().map(|r| r.name()));
    c.current_dir(paths::home()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(NO_WINDOW | 0x0000_0200);
    // started outside this app's job (which is closed by now), so it outlives this process
    if let Err(e) = c.spawn() {
        trace(&format!("! the new version didn't start ({e})"));
    }
    std::process::exit(0);
}

fn end() {
    stop_all();
    with(|a| unsafe {
        if a.icon_shown {
            Shell_NotifyIconW(NIM_DELETE, &a.icon);
        }
        if !a.mutex.is_null() {
            CloseHandle(a.mutex);
        }
    });
    super::clear_state(std::process::id());
}

/// The icon's menu: what runs, the window's pages, how phones reach this computer, and the
/// settings worth a right-click.
fn menu(hwnd: HWND) {
    use crate::desktop::win::Menu;
    let Some((running, link)) = with(|a| (running_labels(a), a.sup.wants(Role::Link) || a.adopted.contains_key(&Role::Link))) else { return };
    let desk = crate::desktop::desk();
    let ((lan, vpn, tunnel), applying) = desk.reach_now();
    let joined = paths::saved_hub().is_some();
    let shown = crate::desktop::win::visible();
    crate::desktop::win::menu_theme(crate::desktop::looks_dark(&crate::desktop::lock(&desk.settings).theme));
    let m = Menu::new();
    m.item(0, &tray_line(&running), false, false);
    m.sep();
    m.item(OPEN, if shown { "Hide Inyon" } else { "Open Inyon" }, false, true);
    m.item(PAIR, "Pair a phone…", false, true);
    m.sep();
    let reach = Menu::new();
    reach.item(LAN, "LAN (this network)", lan, !applying);
    reach.item(VPN, "VPN (Tailscale, NetBird…)", vpn, !applying);
    reach.item(TUNNEL, "Tunnel (anywhere)", tunnel, !applying);
    m.sub(if applying { "Phones reach this computer (changing…)" } else { "Phones reach this computer" }, reach);
    if link {
        m.item(STOP_LINK, "Stop the phone link", false, true);
    } else {
        m.item(START_LINK, "Start the phone link", false, true);
    }
    m.sep();
    m.item(PHONES, "Phones", false, true);
    m.item(HUB, if joined { "Hub" } else { "Connect to a hub…" }, false, true);
    m.item(TEMP, "Temp files", false, true);
    m.item(SETTINGS, "Settings", false, true);
    m.sep();
    m.item(SIGN_IN, "Start at sign-in", desk.sign_in(), true);
    m.item(UPDATE, "Check for updates", false, true);
    m.item(LOGS, "Open the logs", false, true);
    m.sep();
    m.item(QUIT, "Quit Inyon", false, true);
    let chosen = unsafe {
        SetMenuDefaultItem(m.0, OPEN as u32, 0);
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // without this the menu stays open when you click somewhere else
        SetForegroundWindow(hwnd);
        let c = TrackPopupMenu(m.0, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, 0, hwnd, null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        c as usize
    };
    drop(m);
    // the slow ones run on a thread, so the icon keeps answering
    let later = |what: &'static str, f: Box<dyn FnOnce() -> Result<serde_json::Value, String> + Send>| {
        std::thread::spawn(move || {
            if let Err(e) = f() {
                trace(&format!("! {what}: {e}"));
                tell("Inyon", &e, true);
            }
        });
    };
    match chosen {
        OPEN if shown => crate::desktop::win::hide(),
        OPEN => open_window(false),
        PAIR => {
            with(|a| a.sup.want(Role::Link));
            open_window(true);
        }
        PHONES => open_page("phones"),
        HUB => open_page("hub"),
        TEMP => open_page("temp"),
        SETTINGS => open_page("settings"),
        LAN | VPN | TUNNEL => {
            let which = match chosen {
                LAN => "lan",
                VPN => "vpn",
                _ => "tunnel",
            };
            later("changing how phones reach this computer", Box::new(move || desk.flip_reach(which)));
        }
        START_LINK | STOP_LINK => {
            let start = chosen == START_LINK;
            with(|a| {
                if start {
                    a.sup.want(Role::Link);
                    a.sup.now(Role::Link);
                    a.settle_until = crate::state::now_ms() + SETTLE_MS;
                }
            });
            if !start {
                handle(Request::new(Do::Stop, Some(Role::Link), None));
            }
        }
        SIGN_IN => {
            let on = !desk.sign_in();
            later("start at sign-in", Box::new(move || desk.handle("autostart", &serde_json::json!({ "on": on }))));
        }
        UPDATE => {
            later("the update check", Box::new(move || desk.handle("update", &serde_json::json!({ "install": false }))));
            open_page("settings");
        }
        LOGS => {
            let dir = service::log_file(Role::Link).parent().map(PathBuf::from).unwrap_or_else(paths::node_dir);
            let _ = Command::new("explorer.exe").arg(dir).creation_flags(NO_WINDOW).spawn();
        }
        QUIT => quit(),
        _ => {}
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            tick();
            0
        }
        WAKE => {
            tick();
            0
        }
        TRAY => {
            match (lparam as u32) & 0xffff {
                WM_LBUTTONUP => crate::desktop::win::toggle(),
                WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                _ => {}
            }
            0
        }
        WINDOW_CLOSED => {
            let anything = with(|a| !a.sup.is_empty() || !a.adopted.is_empty() || super::pairing()).unwrap_or(false);
            match on_close(anything, super::note_file().exists()) {
                Close::Quit => {
                    trace("the window closed and nothing runs: ending");
                    unsafe { PostQuitMessage(0) };
                }
                Close::ToTray { note } => {
                    if note {
                        note_once();
                    }
                }
            }
            0
        }
        m if with(|a| a.taskbar_created == m).unwrap_or(false) => {
            // Explorer started again: the icon is added anew
            with(|a| {
                if a.icon_shown {
                    a.icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
                    unsafe { Shell_NotifyIconW(NIM_ADD, &a.icon) };
                }
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Asks `text` in a message box with OK and Cancel; true for OK.
pub fn confirm(title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDOK, MB_ICONQUESTION, MB_OKCANCEL, MB_SETFOREGROUND};
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(title).as_ptr(), MB_OKCANCEL | MB_ICONQUESTION | MB_SETFOREGROUND) == IDOK }
}

/// Says `text` in a message box.
pub fn tell(title: &str, text: &str, error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND};
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(title).as_ptr(), MB_OK | icon | MB_SETFOREGROUND) };
}
