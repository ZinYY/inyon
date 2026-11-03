//! The Android side, over JNI: logcat, and starting the Kotlin keep-alive service.

use jni::objects::{JObject, JValue};
use jni::JavaVM;
use std::ffi::{c_char, c_int, CString};

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// Stdout goes nowhere on Android, so the CI job reads these lines from logcat (tag InyonSpike).
pub fn log(msg: &str) {
    let text = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    // 4 = ANDROID_LOG_INFO
    unsafe { __android_log_write(4, c"InyonSpike".as_ptr(), text.as_ptr()) };
}

/// Process.getStartUptimeMillis() against SystemClock.uptimeMillis(): reading /proc/uptime is
/// denied to apps.
pub fn ms_since_process_start() -> Option<i64> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let start = env.call_static_method("android/os/Process", "getStartUptimeMillis", "()J", &[]).ok()?.j().ok()?;
    let now = env.call_static_method("android/os/SystemClock", "uptimeMillis", "()J", &[]).ok()?.j().ok()?;
    Some(now - start)
}

/// Starts KeepAliveService (android/MainActivity.kt) as a foreground service: the same call
/// InyonDevicePlugin makes today, built here from Rust.
pub fn start_keep_alive(text: &str) -> Result<(), String> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // a global ref owned by the activity glue: borrowed, never deleted here
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let res = (|| -> jni::errors::Result<()> {
        // FindClass on a thread Rust started only sees system classes; the app's own come
        // through its class loader
        let loader = env.call_method(&activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string("dev.dioxus.main.KeepAliveService")?;
        let class = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?;
        let intent = env.new_object("android/content/Intent", "(Landroid/content/Context;Ljava/lang/Class;)V", &[JValue::Object(&activity), JValue::Object(&class)])?;
        let key = env.new_string("text")?;
        let value = env.new_string(text)?;
        env.call_method(&intent, "putExtra", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&key), JValue::Object(&value)])?;
        env.call_method(&activity, "startForegroundService", "(Landroid/content/Intent;)Landroid/content/ComponentName;", &[JValue::Object(&intent)])?;
        Ok(())
    })();
    if let Err(e) = res {
        let mut detail = e.to_string();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            detail.push_str(" (a Java exception, see logcat)");
        }
        return Err(detail);
    }
    Ok(())
}
