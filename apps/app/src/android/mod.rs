//! The Android side (from 1.0), over JNI with the Kotlin in apps/app/android. The Application
//! starts the phone core as the process starts (`Native.startCore`), so it runs whether or not a
//! screen is open, and a notification button can reach it with no window at all. The screens find
//! the core through crate::phone. Rust calls back into Kotlin's `Native` object for SSH, the
//! keep-alive service and notifications; those classes are shared with the Capacitor app
//! (apps/mobile/android/app/src/shared).

use std::ffi::{c_char, c_int, CString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use jni::objects::{GlobalRef, JClass, JString, JValue};
use jni::sys::{jboolean, jlong, JNI_FALSE, JNI_TRUE};
use jni::{JNIEnv, JavaVM};

use crate::notify::{Notice, Shade};
use crate::ssh_bridge::{self, BridgeSsh};

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// A line in logcat, under the tag `Inyon` (the CI's smoke test reads them).
pub fn log(error: bool, line: &str) {
    let text = CString::new(line.replace('\0', " ")).unwrap_or_default();
    // 6 = ANDROID_LOG_ERROR, 4 = ANDROID_LOG_INFO
    unsafe { __android_log_write(if error { 6 } else { 4 }, c"Inyon".as_ptr(), text.as_ptr()) };
}

/// The phone's time zone, as minutes east of UTC.
pub fn tz_minutes() -> i32 {
    // SAFETY: localtime_r writes only into the tm it's given
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return 0;
        }
        (tm.tm_gmtoff / 60) as i32
    }
}

static VM: OnceLock<JavaVM> = OnceLock::new();
/// Kotlin's `Native` class, held so any thread can call it (a thread Rust started can't find app
/// classes by name)
static NATIVE: OnceLock<GlobalRef> = OnceLock::new();

/// Calls a static method of `Native`, from any thread; None (and a line in logcat) if it failed.
fn call<R>(what: &str, f: impl FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<R>) -> Option<R> {
    let (vm, class) = (VM.get()?, NATIVE.get()?);
    let mut env = vm.attach_current_thread_permanently().ok()?;
    // a frame of its own: a thread that stays attached would otherwise keep every local reference
    let got = env.with_local_frame(16, |env| f(env, class.as_obj().into()));
    match got {
        Ok(r) => Some(r),
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log(true, &format!("{what} failed: {e}"));
            None
        }
    }
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

/// Hands an SSH call to Kotlin's SshMethods; its answer comes back through `Native.sshReply`.
fn ssh_call(id: u64, method: &'static str, args: &str) -> bool {
    call("ssh", |env, class| {
        let (m, a) = (env.new_string(method)?, env.new_string(args)?);
        env.call_static_method(class, "ssh", "(JLjava/lang/String;Ljava/lang/String;)V", &[JValue::Long(id as i64), JValue::Object(&m), JValue::Object(&a)])?;
        Ok(())
    })
    .is_some()
}

/// The upgrade file the Capacitor app sealed (MigrationExport.read), opened; None when there's none.
fn read_migration() -> Option<String> {
    call("readMigration", |env, class| {
        let r = env.call_static_method(class, "readMigration", "()Ljava/lang/String;", &[])?.l()?;
        Ok(if r.is_null() { None } else { Some(jstr(env, &JString::from(r))) })
    })
    .flatten()
}

/// Deletes the upgrade file and its key, once its contents are in this app's stores.
fn discard_migration() {
    call("discardMigration", |env, class| env.call_static_method(class, "discardMigration", "()V", &[]).map(drop));
}

/// The notification shade, through Kotlin's Notifications (the same as the Capacitor app posts).
struct AndroidShade;

impl Shade for AndroidShade {
    fn post(&self, n: &Notice) -> bool {
        let json = serde_json::to_string(n).unwrap_or_default();
        call("postNotification", |env, class| {
            let j = env.new_string(&json)?;
            env.call_static_method(class, "postNotification", "(Ljava/lang/String;)Z", &[JValue::Object(&j)])?.z()
        })
        .unwrap_or(false)
    }

    fn clear(&self, tag: &str) {
        call("clearNotification", |env, class| {
            let t = env.new_string(tag)?;
            env.call_static_method(class, "clearNotification", "(Ljava/lang/String;)V", &[JValue::Object(&t)])?;
            Ok(())
        });
    }
}

// ---------- staying connected in the background ----------

/// The setting the web app keeps under the same name: on unless turned off.
const KEEP_ALIVE: &str = "inyon.keepAlive";
const NOTICE_TITLE: &str = "Inyon is connected";
const NOTICE_TEXT: &str = "Keeping your sessions live so alerts arrive right away";

fn keep_alive_wanted() -> bool {
    crate::platform::load(KEEP_ALIVE).as_deref() != Some("0")
}

/// Starts the keep-alive service if the person wants it and there's a machine to stay connected
/// to. Only from the foreground: Android refuses a foreground service started from the background.
fn keep_alive_on_screen() {
    let Some(phone) = crate::phone::handle() else { return };
    let ask = phone.with_phone(|_| async { !inyon_core::hosts::registry::registry().configs().is_empty() });
    phone.spawn(move || async move {
        if !ask.await.unwrap_or(false) || !keep_alive_wanted() {
            return;
        }
        // alerts need notifications; without them Android still runs the service but hides it
        call("askNotifications", |env, class| env.call_static_method(class, "askNotifications", "()V", &[]).map(drop));
        let refused = call("startKeepAlive", |env, class| {
            let (t, x) = (env.new_string(NOTICE_TITLE)?, env.new_string(NOTICE_TEXT)?);
            let r = env.call_static_method(class, "startKeepAlive", "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&t), JValue::Object(&x)])?.l()?;
            Ok(if r.is_null() { None } else { Some(jstr(env, &JString::from(r))) })
        })
        .flatten();
        if let Some(why) = refused {
            log(false, &format!("keep-alive not started: {why}"));
        }
    });
}

/// Starts the keep-alive service when a machine is paired while the app is on screen, as it starts
/// when the app comes on screen with machines already paired.
fn keep_alive_when_paired(phone: &inyon_phone::native::PhoneHandle) {
    phone.spawn(|| async {
        use inyon_core::hosts::registry::{registry, RegistryEvent};
        let first = crate::phone::FirstMachine::default();
        let _ = registry().on(move |e| {
            if matches!(e, RegistryEvent::Overview) && crate::phone::is_visible() && first.seen(registry().configs().len(), true) {
                keep_alive_on_screen();
            }
        });
    });
}

// ---------- from Kotlin ----------

/// `Native.startCore(dir, maker, model)`, from the Application as the process starts: the phone
/// core, its SSH over Kotlin and its notifications.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_startCore(mut env: JNIEnv, class: JClass, dir: JString, maker: JString, model: JString) {
    if crate::phone::handle().is_some() {
        return;
    }
    if let Ok(vm) = env.get_java_vm() {
        let _ = VM.set(vm);
    }
    if let Ok(g) = env.new_global_ref(&class) {
        let _ = NATIVE.set(g);
    }
    let dir = PathBuf::from(jstr(&mut env, &dir));
    let name = crate::phone::readable_name(&jstr(&mut env, &maker), &jstr(&mut env, &model));
    // the Capacitor app's records and settings, on the first start after installing over it
    let _ = std::fs::create_dir_all(&dir);
    match crate::upgrade::first_start(&dir, read_migration, discard_migration, &mut |k, v| crate::platform::save_in(&dir, k, v)) {
        Some(Ok(got)) => {
            log(false, &format!("brought over from the previous app: {} records, {} settings, kept {:?}, skipped {:?}", got.records, got.settings, got.kept, got.skipped))
        }
        Some(Err(why)) => log(true, &format!("the previous app's file wasn't imported: {why:?}")),
        None => {}
    }
    let phone = inyon_phone::native::start(inyon_phone::native::Options {
        dir: dir.join("core"),
        name,
        ssh: Box::new(|| Rc::new(BridgeSsh(ssh_call))),
        log: Some(Arc::new(log)),
        offline: false,
    });
    crate::notify::start(&phone, || Rc::new(AndroidShade));
    keep_alive_when_paired(&phone);
    crate::phone::install(phone, Path::new(&dir));
    // the release smoke test looks for this line
    log(false, "core ready");
}

/// `Native.visible(on)`: the app's window came on screen or left it.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_visible(_env: JNIEnv, _class: JClass, on: jboolean) {
    let on = on == JNI_TRUE;
    crate::phone::set_visible(on);
    if on {
        // Android may have stopped it (a daily limit, memory); it can only start again from here
        keep_alive_on_screen();
    }
}

/// `Native.sshReply(id, ok, json)`: an SSH call's answer.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_sshReply(mut env: JNIEnv, _class: JClass, id: jlong, ok: jboolean, json: JString) {
    let json = jstr(&mut env, &json);
    ssh_bridge::reply(id as u64, ok == JNI_TRUE, &json);
}

/// `Native.sshEvent(name, json)`: a stream's lines, a terminal's output, a server gone.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_sshEvent(mut env: JNIEnv, _class: JClass, name: JString, json: JString) {
    let (name, json) = (jstr(&mut env, &name), jstr(&mut env, &json));
    ssh_bridge::event(&name, &json);
}

/// `Native.keepAliveEnded(reason)`: Stay connected stopped. Its Stop button turns it off for good.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_keepAliveEnded(mut env: JNIEnv, _class: JClass, reason: JString) {
    let reason = jstr(&mut env, &reason);
    if reason == "stopped" {
        crate::platform::save(KEEP_ALIVE, "0");
    }
    log(false, &format!("keep-alive ended: {reason}"));
}

/// `Native.openHref(href)`: a tapped notification's path (already checked by HrefGate).
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_openHref(mut env: JNIEnv, _class: JClass, href: JString) {
    let href = jstr(&mut env, &href);
    crate::phone::open(&href);
}

/// `Native.darkTheme(night)`: whether the page is dark, from the theme in Settings and whether the
/// system is in dark mode, so the system bars' icons match it.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_darkTheme(_env: JNIEnv, _class: JClass, night: jboolean) -> jboolean {
    crate::platform::dark(crate::platform::theme_setting(), night == JNI_TRUE) as jboolean
}

/// Whether the system is in dark mode now (for the theme "system").
pub fn night() -> bool {
    call("night", |env, class| env.call_static_method(class, "night", "()Z", &[])?.z()).unwrap_or(false)
}

/// `Native.answer(json)`: an answer tapped on a question's notification, as notify::Tap; false
/// when the core can't send it.
#[no_mangle]
pub extern "system" fn Java_tech_abhashchakraborty_inyon_app_Native_answer(mut env: JNIEnv, _class: JClass, json: JString) -> jboolean {
    let json = jstr(&mut env, &json);
    match serde_json::from_str::<crate::notify::Tap>(&json) {
        Ok(tap) => crate::notify::answer(tap).into(),
        Err(e) => {
            log(true, &format!("a tapped answer didn't read: {e}"));
            JNI_FALSE
        }
    }
}
