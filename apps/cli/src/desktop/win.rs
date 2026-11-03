//! The window on Windows: a plain Win32 window that holds two WebView2 views. The first shows the
//! app's own pages (served from the binary at http://inyon.localhost); the second, laid over a
//! placeholder the pages leave for it, shows a hub's web app or a page from Pages. The window
//! lives on a thread of its own for as long as the app runs: closing it hides it, the tray icon
//! shows it again.
//!
//! The pages call the app through `window.ipc.postMessage({id, cmd, args})`. Calls that touch the
//! window (the view, menus, the folder picker, the clipboard) run here; the rest go to
//! [`super::Desk::handle`] on a thread each, and the answer comes back as
//! `window.__inyonReply(id, ok, value)`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, CreateSolidBrush, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, IsIconic, IsWindowVisible, LoadCursorW,
    PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, TrackPopupMenu, TranslateMessage, HMENU, IDC_ARROW, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MINMAXINFO, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, TPM_RETURNCMD, TPM_RIGHTBUTTON, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_CLOSE,
    WM_DPICHANGED, WM_GETMINMAXINFO, WM_NULL, WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use wry::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use wry::http::{header::CONTENT_TYPE, Request, Response};
use wry::{NewWindowResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use super::{own_page, trace, APP_ORIGIN, CLASS};

const SHOW: u32 = WM_APP + 10;
const HIDE: u32 = WM_APP + 11;
const TOGGLE: u32 = WM_APP + 12;
/// calls from the pages are waiting in INBOX
const IPC: u32 = WM_APP + 13;
/// answers are waiting in OUTBOX
const SCRIPT: u32 = WM_APP + 14;

/// the window, once its thread made it
static WINDOW: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// the app's hidden window, and the message it gets when this window closes
static HOST: AtomicUsize = AtomicUsize::new(0);
static HOST_CLOSED: AtomicU32 = AtomicU32::new(0);
/// when the window lost the focus (ms), 0 while it has it
static LEFT_FRONT: AtomicU64 = AtomicU64::new(1);
static PENDING: Mutex<Pending> = Mutex::new(Pending { show: false, page: None, pair: false });
static INBOX: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static OUTBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What the next SHOW does: open on a page, start a pairing code.
struct Pending {
    show: bool,
    page: Option<String>,
    pair: bool,
}

struct Ui {
    shell: Option<WebView>,
    view: Option<WebView>,
    /// the address the view was asked to show last (it may have moved on since)
    view_url: String,
    view_making: bool,
    dark: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// Runs `f` on the window's state; None when it isn't there or is in use further up the stack
/// (WebView2 runs the message loop while a view is made).
fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut u| u.as_mut().map(f)))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Parent(HWND);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let h = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(h))) })
    }
}

// ---------- what the app calls, from any thread ----------

/// Tells the window where to report that it closed: the app's hidden window and a message.
pub fn set_host(hwnd: usize, closed: u32) {
    HOST.store(hwnd, Ordering::SeqCst);
    HOST_CLOSED.store(closed, Ordering::SeqCst);
}

/// Whether the window is on screen.
pub fn visible() -> bool {
    VISIBLE.load(Ordering::SeqCst)
}

/// Whether the window's thread is still making it: WebView2's first start can take many seconds.
/// A window that couldn't be made clears STARTED, so this ends either way.
pub fn opening() -> bool {
    STARTED.load(Ordering::SeqCst) && WINDOW.load(Ordering::SeqCst) == 0
}

/// Shows the window (made the first time), on `page` when given, with a pairing code when `pair`.
pub fn show(page: Option<&str>, pair: bool) {
    {
        let mut p = lock(&PENDING);
        p.show = true;
        if page.is_some() {
            p.page = page.map(String::from);
        }
        p.pair |= pair;
    }
    if !STARTED.swap(true, Ordering::SeqCst) {
        std::thread::spawn(run);
        return;
    }
    post(SHOW);
}

pub fn hide() {
    post(HIDE);
}

/// What a click on the tray icon does: shows the window, or hides it when it's in front.
pub fn toggle() {
    if !STARTED.load(Ordering::SeqCst) {
        show(None, false);
    } else {
        post(TOGGLE);
    }
}

fn post(msg: u32) {
    let w = WINDOW.load(Ordering::SeqCst);
    if w != 0 {
        unsafe { PostMessageW(w as HWND, msg, 0, 0) };
    }
}

/// Queues `window.__inyonEvent(name, value)` for the pages.
pub fn event(name: &str, value: Value) {
    script(format!("window.__inyonEvent && window.__inyonEvent({}, {})", json!(name), value));
}

fn reply(id: &Value, r: Result<Value, String>) {
    let (ok, v) = match r {
        Ok(v) => (true, v),
        Err(e) => (false, json!(e)),
    };
    script(format!("window.__inyonReply({id}, {ok}, {v})"));
}

fn script(js: String) {
    lock(&OUTBOX).push(js);
    post(SCRIPT);
}

// ---------- the window's thread ----------

fn run() {
    let desk = super::desk();
    let class = wide(CLASS);
    let dark = super::looks_dark(&super::lock(&desk.settings).theme);
    unsafe {
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: instance,
            hIcon: crate::tray::win::icon_sized(32, 32),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: CreateSolidBrush(colorref(ground(dark))),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let (x, y, w, h) = placement();
        let hwnd = CreateWindowExW(0, class.as_ptr(), wide("Inyon").as_ptr(), WS_OVERLAPPEDWINDOW, x, y, w, h, null_mut(), null_mut(), instance, null());
        if hwnd.is_null() {
            trace(&format!("! no window: error {}", GetLastError()));
            STARTED.store(false, Ordering::SeqCst);
            return;
        }
        title_bar(hwnd, dark);
        menu_theme(dark);
        let shell = match make_shell(hwnd, dark) {
            Ok(s) => s,
            Err(e) => {
                trace(&format!("! no WebView2: {e}"));
                no_webview(&e.to_string());
                STARTED.store(false, Ordering::SeqCst);
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
                return;
            }
        };
        UI.with(|u| *u.borrow_mut() = Some(Ui { shell: Some(shell), view: None, view_url: String::new(), view_making: false, dark }));
        WINDOW.store(hwnd as usize, Ordering::SeqCst);
        trace(&format!("window made ({w}x{h})"));
        on_show(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 1120x760 at 96 dpi, centred on the monitor under the mouse and never past its work area.
fn placement() -> (i32, i32, i32, i32) {
    unsafe {
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        let monitor = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work = if GetMonitorInfoW(monitor, &mut mi) != 0 { mi.rcWork } else { RECT { left: 0, top: 0, right: 1366, bottom: 728 } };
        let mut dpi = 96u32;
        let mut dy = 96u32;
        let _ = windows_sys::Win32::UI::HiDpi::GetDpiForMonitor(monitor, windows_sys::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        let (ww, wh) = (work.right - work.left, work.bottom - work.top);
        let w = (1120 * dpi as i32 / 96).min(ww - 32);
        let h = (760 * dpi as i32 / 96).min(wh - 32);
        (work.left + (ww - w) / 2, work.top + (wh - h) / 2, w, h)
    }
}

/// The browser's switches; both views share one browser, so they must match.
fn browser_args() -> String {
    let mut a = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    // a port to inspect the views from another computer's DevTools while testing
    if let Some(port) = std::env::var("INYON_WEBVIEW_DEBUG_PORT").ok().filter(|p| p.parse::<u16>().is_ok()) {
        a.push_str(&format!(" --remote-debugging-port={port}"));
    }
    a
}

fn devtools() -> bool {
    std::env::var_os("INYON_DEVTOOLS").is_some_and(|v| !v.is_empty())
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut r) };
    ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
}

fn whole(hwnd: HWND) -> Rect {
    let (w, h) = client_size(hwnd);
    Rect { position: PhysicalPosition::new(0, 0).into(), size: PhysicalSize::new(w, h).into() }
}

fn web_context() -> WebContext {
    WebContext::new(Some(crate::paths::node_dir().join("webview")))
}

fn make_shell(hwnd: HWND, dark: bool) -> wry::Result<WebView> {
    let w = hwnd as usize;
    let mut context = web_context();
    WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("inyon".into(), |_, req| app_file(&req))
        .with_ipc_handler(move |req: Request<String>| {
            // only the app's own pages talk to the app
            if !own_page(&req.uri().to_string()) {
                return;
            }
            lock(&INBOX).push_back(req.into_body());
            unsafe { PostMessageW(w as HWND, IPC, 0, 0) };
        })
        .with_url(format!("{APP_ORIGIN}/index.html"))
        .with_background_color(rgba(ground(dark)))
        .with_bounds(whole(hwnd))
        .with_devtools(devtools())
        .with_navigation_handler(|url| {
            if own_page(&url) || url.starts_with("about:") {
                return true;
            }
            open_external(&url);
            false
        })
        .with_new_window_req_handler(|url, _| {
            open_external(&url);
            NewWindowResponse::Deny
        })
        .with_https_scheme(false)
        .with_browser_accelerator_keys(false)
        .with_additional_browser_args(browser_args())
        .with_theme(if dark { wry::Theme::Dark } else { wry::Theme::Light })
        .build_as_child(&Parent(hwnd))
}

/// The view, made the first time something is shown in it.
fn make_view(hwnd: HWND, env: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment>, url: &str, bounds: Rect, dark: bool) -> wry::Result<WebView> {
    // the shell's browser, or one on the same data folder
    let mut context = web_context();
    let mut b = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("inyonpage".into(), |_, req| page_file(&req))
        .with_url(url)
        .with_background_color(rgba(ground(dark)))
        .with_bounds(bounds)
        .with_devtools(devtools())
        .with_new_window_req_handler(|url, _| {
            open_external(&url);
            NewWindowResponse::Deny
        })
        .with_document_title_changed_handler(|t| event("viewTitle", json!(t)))
        .with_on_page_load_handler(|e, url| {
            if matches!(e, wry::PageLoadEvent::Finished) {
                event("viewUrl", json!(url));
            }
        })
        .with_https_scheme(false)
        .with_additional_browser_args(browser_args())
        .with_theme(if dark { wry::Theme::Dark } else { wry::Theme::Light });
    if let Some(env) = env {
        b = b.with_environment(env);
    }
    let view = b.build_as_child(&Parent(hwnd))?;
    pin_hub_certificate(&view);
    Ok(view)
}

/// A hub joined by its certificate's fingerprint (a hub on the LAN with a certificate of its own)
/// opens in the view without a warning when it shows that same certificate, and only then.
fn pin_hub_certificate(view: &WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_14, COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW};
    use webview2_com::ServerCertificateErrorDetectedEventHandler;
    use windows_core::Interface;
    let Ok(wv) = view.webview().cast::<ICoreWebView2_14>() else {
        trace("this WebView2 can't check a hub's own certificate");
        return;
    };
    let handler = ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let cert = args.ServerCertificate()?;
            let mut pem = windows_core::PWSTR::null();
            cert.ToPemEncoding(&mut pem)?;
            let pem = webview2_com::take_pwstr(pem);
            let pinned = crate::hub_cert::fingerprint(&inyon_engine::http::hub_cert_file());
            let shown = crate::hub_cert::fingerprint_pem(&pem);
            if pinned.is_some() && pinned == shown {
                args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)?;
            } else {
                trace(&format!("! the view was shown a certificate that isn't the joined hub's ({})", shown.unwrap_or_default()));
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    if let Err(e) = unsafe { wv.add_ServerCertificateErrorDetected(&handler, &mut token) } {
        trace(&format!("! no certificate check in the view: {e}"));
    }
}

fn respond(status: u16, mime: &str, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, mime)
        .header("Cache-Control", "no-store")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

/// The app's pages, and the temp files they show (only files inside a temp folder).
fn app_file(req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = req.uri().path();
    if path == "/temp-file" {
        let want = req.uri().query().unwrap_or_default().split('&').find_map(|kv| kv.strip_prefix("path=")).map(super::percent_decode).unwrap_or_default();
        // only a plain file inside a temp folder: never a link out of one
        let file = crate::temp::locate(&crate::temp::roots(), &want).ok().map(|(p, _)| p).filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()));
        return match file {
            Some(file) => match std::fs::read(&file) {
                Ok(b) => respond(200, super::mime_for(&file), b),
                Err(_) => respond(404, "text/plain", b"gone".to_vec()),
            },
            None => respond(403, "text/plain", b"not a temp file".to_vec()),
        };
    }
    let name = path.trim_start_matches('/');
    match super::asset(if name.is_empty() { "index.html" } else { name }) {
        Some((body, mime)) => Response::builder().header(CONTENT_TYPE, mime).body(Cow::Borrowed(body)).unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..]))),
        None => respond(404, "text/plain", b"not found".to_vec()),
    }
}

/// A page from Pages: http://inyonpage.localhost/<slug>/<file>.
fn page_file(req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    match super::page_file(req.uri().path(), &super::local_pages()) {
        Ok(file) => match std::fs::read(&file) {
            Ok(b) => respond(200, super::mime_for(&file), b),
            Err(_) => respond(404, "text/html; charset=utf-8", b"<p>That file isn't there any more.</p>".to_vec()),
        },
        Err(why) => respond(404, "text/html; charset=utf-8", format!("<p>{why}</p>").into_bytes()),
    }
}

/// Opens web addresses in the browser; nothing else is handed to the shell.
fn open_external(url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:") {
        shell_open(url);
    }
}

fn shell_open(target: &str) {
    unsafe { ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(target).as_ptr(), null(), null(), SW_SHOW) };
}

fn no_webview(e: &str) {
    let text = format!(
        "The Inyon window needs Microsoft Edge WebView2, which this computer doesn't have or couldn't start ({e}).\n\nInstall it from Microsoft (the page opens now), then open Inyon again. The phone link and the hub connection keep running."
    );
    crate::app::win::tell("Inyon", &text, true);
    shell_open("https://developer.microsoft.com/microsoft-edge/webview2/");
}

// ---------- colours and the theme ----------

fn ground(dark: bool) -> u32 {
    if dark {
        0x1b1a18
    } else {
        0xe9e8e4
    }
}

fn rgba(rgb: u32) -> (u8, u8, u8, u8) {
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
}

fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}

/// The title bar follows the theme. Windows 10 before 20H1 knows the attribute as 19.
fn title_bar(hwnd: HWND, dark: bool) {
    let on: i32 = dark as i32;
    let set = |attr: u32| unsafe { DwmSetWindowAttribute(hwnd, attr, &on as *const i32 as *const _, 4) };
    if set(DWMWA_USE_IMMERSIVE_DARK_MODE as u32) != 0 {
        set(19);
    }
}

/// Menus the app draws itself (the tray's, the one over a hub) in the window's theme, through
/// uxtheme's unnamed SetPreferredAppMode (135) and FlushMenuThemes (136), as Explorer does.
pub fn menu_theme(dark: bool) {
    unsafe {
        let lib = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if lib.is_null() {
            return;
        }
        if let Some(set) = GetProcAddress(lib, 135 as *const u8) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set);
            // 2: dark, 3: light
            set(if dark { 2 } else { 3 });
        }
        if let Some(flush) = GetProcAddress(lib, 136 as *const u8) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

// ---------- showing and hiding ----------

fn on_show(hwnd: HWND) {
    let (page, pair) = {
        let mut p = lock(&PENDING);
        p.show = false;
        (p.page.take(), std::mem::take(&mut p.pair))
    };
    let desk = super::desk();
    if pair {
        desk.pair();
    }
    let page = page.or_else(|| pair.then(|| "phones".to_string()));
    if let Some(page) = &page {
        // read by the pages when they load, or sent now when they're there
        *super::lock(&desk.start_page) = Some(page.clone());
        event("go", json!({ "page": page }));
    }
    unsafe {
        ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(hwnd);
    }
    with(|u| {
        if let Some(s) = &u.shell {
            let _ = s.set_visible(true);
            let _ = s.focus();
        }
        if let Some(v) = &u.view {
            if !u.view_url.is_empty() {
                let _ = v.set_visible(true);
            }
        }
    });
    VISIBLE.store(true, Ordering::SeqCst);
    // the pages poll while they're seen; tell them now
    event("shown", Value::Null);
}

fn on_hide(hwnd: HWND) {
    unsafe { ShowWindow(hwnd, SW_HIDE) };
    // hidden views stop drawing and their timers slow down
    with(|u| {
        if let Some(s) = &u.shell {
            let _ = s.set_visible(false);
        }
        if let Some(v) = &u.view {
            let _ = v.set_visible(false);
        }
    });
    VISIBLE.store(false, Ordering::SeqCst);
    let host = HOST.load(Ordering::SeqCst);
    if host != 0 {
        unsafe { PostMessageW(host as HWND, HOST_CLOSED.load(Ordering::SeqCst), 0, 0) };
    }
}

// ---------- the pages' calls ----------

fn take_calls(hwnd: HWND) {
    loop {
        let Some(raw) = lock(&INBOX).pop_front() else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&raw) else { continue };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = msg.get("args").cloned().unwrap_or(Value::Null);
        match here(hwnd, &cmd, &args) {
            Some(r) => reply(&id, r),
            None => {
                std::thread::spawn(move || {
                    let r = super::desk().handle(&cmd, &args);
                    reply(&id, r);
                });
            }
        }
    }
}

/// The calls the window answers itself; None for the rest.
fn here(hwnd: HWND, cmd: &str, args: &Value) -> Option<Result<Value, String>> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    Some(match cmd {
        "view" => view(hwnd, args),
        "viewGo" => with(|u| match (&u.view, text("to").as_str()) {
            (Some(v), "back") => v.evaluate_script("history.back()").map_err(|e| e.to_string()),
            (Some(v), "reload") => v.reload().map_err(|e| e.to_string()),
            _ => Ok(()),
        })
        .unwrap_or(Ok(()))
        .map(|_| Value::Null),
        "menu" => Ok(popup(hwnd, args)),
        "theme" => {
            let dark = args.get("dark").and_then(Value::as_bool).unwrap_or(false);
            title_bar(hwnd, dark);
            menu_theme(dark);
            with(|u| {
                u.dark = dark;
                for v in [&u.shell, &u.view].into_iter().flatten() {
                    let _ = v.set_theme(if dark { wry::Theme::Dark } else { wry::Theme::Light });
                }
            });
            Ok(Value::Null)
        }
        "openUrl" => {
            let url = text("url");
            if url.starts_with("https://") || url.starts_with("http://") {
                shell_open(&url);
                Ok(Value::Null)
            } else {
                Err("That isn't a web address.".into())
            }
        }
        "openPath" => open_path(&text("path"), args.get("reveal").and_then(Value::as_bool).unwrap_or(false)),
        "pickFolder" => Ok(pick_folder(hwnd).map(Value::from).unwrap_or(Value::Null)),
        "copy" => copy(hwnd, &text("text")).map(|_| Value::Null),
        "hide" => {
            on_hide(hwnd);
            Ok(Value::Null)
        }
        _ => return None,
    })
}

/// Shows, moves or hides the view: `{show, url, x, y, w, h}` in the pages' pixels.
fn view(hwnd: HWND, args: &Value) -> Result<Value, String> {
    let show = args.get("show").and_then(Value::as_bool).unwrap_or(false);
    let url = args.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
    let n = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
    let bounds = Rect { position: LogicalPosition::new(n("x"), n("y")).into(), size: LogicalSize::new(n("w").max(1.0), n("h").max(1.0)).into() };
    if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("The view shows web addresses only.".into());
    }
    let state = with(|u| (u.view.is_some(), u.view_making, u.dark)).ok_or("The window is busy; try again.")?;
    match state {
        (true, _, _) => {
            with(|u| {
                let v = u.view.as_ref().expect("checked");
                if show && !url.is_empty() && url != u.view_url {
                    u.view_url = url.clone();
                    let _ = v.load_url(&url);
                }
                let _ = v.set_bounds(bounds);
                let _ = v.set_visible(show && !u.view_url.is_empty());
            });
            Ok(Value::Null)
        }
        (false, true, _) => Ok(Value::Null),
        (false, false, _) if !show || url.is_empty() => Ok(Value::Null),
        (false, false, dark) => {
            // made outside the borrow: WebView2 runs the message loop while it makes a view
            let env = with(|u| {
                u.view_making = true;
                u.shell.as_ref().map(|s| s.environment())
            })
            .ok_or("The window is busy; try again.")?;
            let made = make_view(hwnd, env, &url, bounds, dark);
            with(|u| u.view_making = false);
            match made {
                Ok(v) => {
                    with(|u| {
                        u.view = Some(v);
                        u.view_url = url;
                    });
                    Ok(Value::Null)
                }
                Err(e) => {
                    trace(&format!("! no view: {e}"));
                    Err(format!("Couldn't open it here ({e})."))
                }
            }
        }
    }
}

/// The app's own menu at `{x, y}` (the pages' pixels) with `{items: [{id, label, disabled,
/// checked} | {sep}]}`: the id picked, or null.
fn popup(hwnd: HWND, args: &Value) -> Value {
    let items = args.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f64 / 96.0;
    let mut at = POINT { x: (args.get("x").and_then(Value::as_f64).unwrap_or(0.0) * scale) as i32, y: (args.get("y").and_then(Value::as_f64).unwrap_or(0.0) * scale) as i32 };
    unsafe {
        ClientToScreen(hwnd, &mut at);
        let m = CreatePopupMenu();
        let mut ids = Vec::new();
        for it in &items {
            if it.get("sep").and_then(Value::as_bool).unwrap_or(false) {
                AppendMenuW(m, MF_SEPARATOR, 0, null());
                continue;
            }
            let label = it.get("label").and_then(Value::as_str).unwrap_or_default();
            let mut flags = MF_STRING;
            if it.get("disabled").and_then(Value::as_bool).unwrap_or(false) {
                flags |= MF_GRAYED;
            }
            if it.get("checked").and_then(Value::as_bool).unwrap_or(false) {
                flags |= MF_CHECKED;
            }
            ids.push(it.get("id").cloned().unwrap_or(Value::Null));
            AppendMenuW(m, flags, ids.len(), wide(label).as_ptr());
        }
        let chosen = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, 0, hwnd, null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);
        if chosen > 0 {
            ids.get(chosen as usize - 1).cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        }
    }
}

/// A native popup menu from `(label, id, flags)` rows, with submenus, for the tray.
pub struct Menu(pub HMENU);

impl Menu {
    pub fn new() -> Menu {
        Menu(unsafe { CreatePopupMenu() })
    }
    pub fn item(&self, id: usize, label: &str, checked: bool, enabled: bool) {
        let mut f = MF_STRING;
        if checked {
            f |= MF_CHECKED;
        }
        if !enabled {
            f |= MF_GRAYED;
        }
        unsafe { AppendMenuW(self.0, f, id, wide(label).as_ptr()) };
    }
    pub fn sep(&self) {
        unsafe { AppendMenuW(self.0, MF_SEPARATOR, 0, null()) };
    }
    pub fn sub(&self, label: &str, sub: Menu) {
        unsafe { AppendMenuW(self.0, MF_STRING | MF_POPUP, sub.0 as usize, wide(label).as_ptr()) };
        // destroyed with its parent
        std::mem::forget(sub);
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe { DestroyMenu(self.0) };
    }
}

impl Default for Menu {
    fn default() -> Self {
        Menu::new()
    }
}

/// Opens a folder or file in Explorer (`reveal` selects it there); `logs` and `inyon` name
/// Inyon's own folders.
fn open_path(path: &str, reveal: bool) -> Result<Value, String> {
    let target = match path {
        "logs" => crate::service::log_file(crate::service::Role::Link).parent().map(PathBuf::from).unwrap_or_else(crate::paths::node_dir),
        "inyon" => crate::paths::data_dir(),
        p => PathBuf::from(p),
    };
    if !target.exists() {
        return Err(format!("{} isn't there any more.", target.display()));
    }
    if reveal {
        let mut c = std::process::Command::new("explorer.exe");
        c.arg(format!("/select,{}", target.display()));
        // explorer.exe is a window, which is what this opens
        crate::sys::quiet(&mut c).spawn().map_err(|e| format!("Couldn't open Explorer ({e})."))?;
    } else {
        shell_open(&target.to_string_lossy());
    }
    Ok(Value::Null)
}

/// The folder picker; None when it was cancelled.
fn pick_folder(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = d.GetOptions().ok()?;
        d.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let title = wide("Pick a folder with an index.html");
        let _ = d.SetTitle(windows_core::PCWSTR(title.as_ptr()));
        d.Show(Some(windows::Win32::Foundation::HWND(hwnd))).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Puts `text` on the clipboard as Unicode text.
fn copy(hwnd: HWND, text: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let w = wide(text);
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return Err("Another app holds the clipboard; try again.".into());
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
        if mem.is_null() {
            CloseClipboard();
            return Err("Couldn't copy it.".into());
        }
        let p = GlobalLock(mem) as *mut u16;
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        GlobalUnlock(mem);
        if SetClipboardData(CF_UNICODETEXT, mem as _).is_null() {
            GlobalFree(mem);
            CloseClipboard();
            return Err("Couldn't copy it.".into());
        }
        CloseClipboard();
    }
    Ok(())
}

// ---------- the window procedure ----------

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        SHOW => {
            on_show(hwnd);
            0
        }
        HIDE => {
            on_hide(hwnd);
            0
        }
        TOGGLE => {
            // clicking the icon takes the focus from the window first: it was in front when it
            // had the focus until a moment ago
            let left = LEFT_FRONT.load(Ordering::SeqCst);
            let front = unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 } && (left == 0 || crate::state::now_ms() as u64 - left < 600);
            if front {
                on_hide(hwnd);
            } else {
                on_show(hwnd);
            }
            0
        }
        IPC => {
            take_calls(hwnd);
            0
        }
        SCRIPT => {
            let scripts = std::mem::take(&mut *lock(&OUTBOX));
            if with(|u| {
                if let Some(s) = &u.shell {
                    for js in &scripts {
                        let _ = s.evaluate_script(js);
                    }
                }
            })
            .is_none()
            {
                // busy further up: try again in a moment
                lock(&OUTBOX).splice(0..0, scripts);
                unsafe { PostMessageW(hwnd, SCRIPT, 0, 0) };
            }
            0
        }
        WM_ACTIVATE => {
            // 0: it has the focus now; else when it lost it
            LEFT_FRONT.store(if wparam & 0xffff == WA_INACTIVE as usize { crate::state::now_ms() as u64 } else { 0 }, Ordering::SeqCst);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SIZE => {
            let b = whole(hwnd);
            with(|u| {
                if let Some(s) = &u.shell {
                    let _ = s.set_bounds(b);
                }
            });
            0
        }
        WM_DPICHANGED => {
            let r = unsafe { &*(lparam as *const RECT) };
            unsafe { SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE) };
            0
        }
        WM_GETMINMAXINFO => {
            let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
            let mm = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            mm.ptMinTrackSize = POINT { x: 620 * scale / 96, y: 480 * scale / 96 };
            0
        }
        WM_CLOSE => {
            let to_tray = super::lock(&super::desk().settings).close_to_tray;
            if to_tray {
                on_hide(hwnd);
            } else {
                trace("closed with close-to-tray off: quitting");
                let _ = crate::app::send(&crate::app::Request::new(crate::app::Do::Quit, None, None));
                crate::app::win::wake();
                on_hide(hwnd);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
