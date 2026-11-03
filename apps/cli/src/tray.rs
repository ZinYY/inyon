//! The Inyon icon in the Windows notification area, so you can see that inyon runs in the
//! background and reach it without a terminal. The engine starts it (`inyon tray link --pid N`)
//! when it runs with no console, from the sign-in task, and it goes away when that engine
//! stops. Its menu opens the status screen, pairs a phone, shows the log, restarts the job or
//! quits it until the next sign-in.

use crate::service::Role;

#[cfg(not(windows))]
pub fn run(_role: Role, _pid: Option<u32>) -> i32 {
    eprintln!("The tray icon is only on Windows.");
    2
}

#[cfg(windows)]
pub fn run(role: Role, pid: Option<u32>) -> i32 {
    win::run(role, pid)
}

/// The tooltip: what runs, in a few words.
pub fn tip(role: Role) -> String {
    match role {
        Role::Link => "Inyon is running: your phone can reach this computer".to_string(),
        Role::Node => "Inyon is running: connected to your hub".to_string(),
        Role::Hub => "Inyon hub is running".to_string(),
    }
}

#[cfg(windows)]
pub(crate) mod win {
    use std::cell::RefCell;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::{CreateMutexW, OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
        KillTimer, LoadIconW, LookupIconIdFromDirectoryEx, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem,
        SetTimer, TrackPopupMenu, TranslateMessage, HICON, IDI_APPLICATION, LR_DEFAULTCOLOR, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON, SW_SHOWNORMAL,
        TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
    };

    use crate::paths;
    use crate::service::{self, Role, Sys};
    use crate::sys::RealProc;

    const ICON: &[u8] = include_bytes!("../assets/inyon.ico");
    const CALLBACK: u32 = WM_APP + 1;
    const WATCH: usize = 1;
    const OPEN: usize = 1;
    const PAIR: usize = 2;
    const LOG: usize = 3;
    const RESTART: usize = 4;
    const QUIT: usize = 5;
    /// CREATE_NO_WINDOW
    const NO_WINDOW: u32 = 0x0800_0000;

    struct State {
        role: Role,
        engine: Option<HANDLE>,
        icon: NOTIFYICONDATAW,
        taskbar_created: u32,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn me() -> PathBuf {
        std::env::current_exe().unwrap_or_else(|_| paths::installed_binary())
    }

    /// The icon at the size the notification area draws, from the .ico built into the program.
    fn load_icon() -> HICON {
        unsafe { icon_sized(GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) }
    }

    /// The program's icon at `w` by `h` pixels (the window's title bar and the taskbar want
    /// other sizes than the notification area).
    pub(crate) fn icon_sized(w: i32, h: i32) -> HICON {
        unsafe {
            let id = LookupIconIdFromDirectoryEx(ICON.as_ptr(), 1, w, h, LR_DEFAULTCOLOR);
            // the directory entry for `id` says where that image's bytes are
            let entries = u16::from_le_bytes([ICON[4], ICON[5]]) as usize;
            for i in 0..entries {
                let e = 6 + 16 * i;
                let at = u32::from_le_bytes([ICON[e + 12], ICON[e + 13], ICON[e + 14], ICON[e + 15]]) as usize;
                let len = u32::from_le_bytes([ICON[e + 8], ICON[e + 9], ICON[e + 10], ICON[e + 11]]) as usize;
                if at as i32 == id && at + len <= ICON.len() {
                    let icon = CreateIconFromResourceEx(ICON[at..].as_ptr(), len as u32, 1, 0x0003_0000, w, h, LR_DEFAULTCOLOR);
                    if !icon.is_null() {
                        return icon;
                    }
                }
            }
            LoadIconW(null_mut(), IDI_APPLICATION)
        }
    }

    pub fn run(role: Role, pid: Option<u32>) -> i32 {
        unsafe {
            // one icon per job: a tray left from an engine that just restarted goes first
            let mutex = CreateMutexW(null(), 0, wide(&format!("Local\\InyonTray-{}", role.name())).as_ptr());
            if !mutex.is_null() && WaitForSingleObject(mutex, 5000) == WAIT_TIMEOUT {
                return 0;
            }
            let engine = pid.map(|p| OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, p)).filter(|h| !h.is_null());
            if pid.is_some() && engine.is_none() {
                return 0;
            }
            let instance = GetModuleHandleW(null());
            let class = wide("InyonTray");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(0, class.as_ptr(), wide("Inyon").as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), instance, null());
            if hwnd.is_null() {
                return 1;
            }
            let mut icon: NOTIFYICONDATAW = std::mem::zeroed();
            icon.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            icon.hWnd = hwnd;
            icon.uID = 1;
            icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            icon.uCallbackMessage = CALLBACK;
            icon.hIcon = load_icon();
            let tip: Vec<u16> = super::tip(role).encode_utf16().take(icon.szTip.len() - 1).collect();
            icon.szTip[..tip.len()].copy_from_slice(&tip);
            Shell_NotifyIconW(NIM_ADD, &icon);
            // Explorer sends this when it starts again (after a crash): the icon is added anew
            let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
            STATE.with(|s| *s.borrow_mut() = Some(State { role, engine, icon, taskbar_created }));
            if engine.is_some() {
                SetTimer(hwnd, WATCH, 2000, None);
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().take() {
                    Shell_NotifyIconW(NIM_DELETE, &st.icon);
                    if let Some(h) = st.engine {
                        CloseHandle(h);
                    }
                }
            });
            KillTimer(hwnd, WATCH);
            if !mutex.is_null() {
                CloseHandle(mutex);
            }
            0
        }
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER => {
                let gone = STATE.with(|s| s.borrow().as_ref().and_then(|st| st.engine).is_some_and(|h| unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0));
                if gone {
                    unsafe { PostQuitMessage(0) };
                }
                0
            }
            CALLBACK => {
                if matches!((lparam as u32) & 0xffff, WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU) {
                    menu(hwnd);
                }
                0
            }
            m if STATE.with(|s| s.borrow().as_ref().is_some_and(|st| st.taskbar_created == m)) => {
                STATE.with(|s| {
                    if let Some(st) = s.borrow().as_ref() {
                        unsafe { Shell_NotifyIconW(NIM_ADD, &st.icon) };
                    }
                });
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn menu(hwnd: HWND) {
        let Some(role) = STATE.with(|s| s.borrow().as_ref().map(|st| st.role)) else { return };
        unsafe {
            let m = CreatePopupMenu();
            let head = format!("Inyon {}", env!("CARGO_PKG_VERSION"));
            AppendMenuW(m, MF_STRING | MF_GRAYED, 0, wide(&head).as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(m, MF_STRING, OPEN, wide("Open Inyon").as_ptr());
            if role == Role::Link {
                AppendMenuW(m, MF_STRING, PAIR, wide("Pair a phone…").as_ptr());
            }
            AppendMenuW(m, MF_STRING, LOG, wide("Show the log").as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(m, MF_STRING, RESTART, wide("Restart").as_ptr());
            AppendMenuW(m, MF_STRING, QUIT, wide("Quit Inyon").as_ptr());
            SetMenuDefaultItem(m, OPEN as u32, 0);
            let mut at = POINT { x: 0, y: 0 };
            GetCursorPos(&mut at);
            // without this the menu stays open when you click somewhere else
            SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, 0, hwnd, null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(m);
            if chosen > 0 {
                act(chosen as usize);
            }
        }
    }

    /// Opens the Inyon window (`inyon window <role>`): a window of its own, not a terminal, so
    /// no console shows. `pair` has it show a pairing code straight away.
    fn open_window(role: Role, pair: bool) {
        let mut c = Command::new(me());
        c.args(["window", role.name()]);
        if pair {
            c.arg("--pair");
        }
        let _ = c.creation_flags(NO_WINDOW).spawn();
    }

    fn act(what: usize) {
        let Some((role, engine)) = STATE.with(|s| s.borrow().as_ref().map(|st| (st.role, st.engine))) else { return };
        let exe = me();
        match what {
            OPEN => open_window(role, false),
            PAIR => open_window(role, true),
            LOG => {
                let log = service::log_file(role);
                if let Some(dir) = log.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::OpenOptions::new().create(true).append(true).open(&log);
                unsafe {
                    ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(&log.to_string_lossy()).as_ptr(), null(), null(), SW_SHOWNORMAL);
                }
            }
            RESTART => {
                // a new engine starts its own icon; this one goes when the old engine has stopped
                std::thread::spawn(move || {
                    let proc = RealProc;
                    let sys = Sys::new(&proc);
                    service::restart_service(&sys, role, &[exe.to_string_lossy().into_owned()]);
                });
            }
            QUIT => {
                let proc = RealProc;
                let sys = Sys::new(&proc);
                service::end_task(&sys, role);
                if let Some(h) = engine {
                    unsafe {
                        if WaitForSingleObject(h, 1000) == WAIT_TIMEOUT {
                            TerminateProcess(h, 0);
                            WaitForSingleObject(h, 2000);
                        }
                    }
                }
                // and the tunnel, as Restart does, once the engine can't start another
                service::stop_tunnel(&sys, role);
                unsafe { PostQuitMessage(0) };
            }
            _ => {}
        }
    }
}
