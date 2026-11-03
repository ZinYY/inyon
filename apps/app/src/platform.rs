//! The few things the app asks of where it runs: the clock, saved settings, the theme, and where
//! its requests go. The browser build uses the page and the hub that served it; the Android build
//! (the `phone` feature) the phone core in the app and files next to its records; the desktop build
//! plain stand-ins for now.

use crate::format::Clock;

#[cfg(target_arch = "wasm32")]
mod imp {
    use wasm_bindgen::{closure::Closure, JsCast};

    pub fn now() -> f64 {
        js_sys::Date::now()
    }

    pub fn tz_minutes() -> i32 {
        // getTimezoneOffset is UTC minus local
        -(js_sys::Date::new_0().get_timezone_offset() as i32)
    }

    pub fn random() -> f64 {
        js_sys::Math::random()
    }

    pub async fn sleep(ms: u32) {
        gloo_timers::future::TimeoutFuture::new(ms).await
    }

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn load(key: &str) -> Option<String> {
        storage()?.get_item(key).ok().flatten()
    }

    pub fn save(key: &str, value: &str) {
        if let Some(s) = storage() {
            let _ = s.set_item(key, value);
        }
    }

    /// The theme the web app's setting asks for (`inyon.settings.v2`), or the system's.
    fn resolved(mq: &web_sys::MediaQueryList) -> &'static str {
        let setting =
            load("inyon.settings.v2").and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v.get("theme").and_then(|t| t.as_str()).map(str::to_owned));
        match setting.as_deref() {
            Some("light") => "light",
            Some("dark") => "dark",
            _ if mq.matches() => "dark",
            _ => "light",
        }
    }

    fn apply(mq: &web_sys::MediaQueryList) {
        let Some(root) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) else { return };
        let t = resolved(mq);
        let _ = root.set_attribute("data-theme", t);
        if let Some(el) = root.dyn_ref::<web_sys::HtmlElement>() {
            let _ = el.style().set_property("background", if t == "dark" { "#1b1a18" } else { "#e9e8e4" });
        }
        // pages in their frames follow the app's theme
        crate::dom::post_theme_all();
    }

    fn dark_query() -> Option<web_sys::MediaQueryList> {
        web_sys::window()?.match_media("(prefers-color-scheme: dark)").ok().flatten()
    }

    /// Puts the saved theme on the page now (after Settings changed it).
    pub fn apply_theme() {
        if let Some(mq) = dark_query() {
            apply(&mq);
        }
    }

    /// /theme.js (shared with the web app) sets the theme before the first paint; this keeps it in
    /// step when the system switches between light and dark.
    pub fn follow_theme() {
        let Some(mq) = dark_query() else { return };
        apply(&mq);
        let m = mq.clone();
        let cb = Closure::<dyn Fn()>::new(move || apply(&m));
        let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use crate::api::ApiError;

    pub fn now() -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
    }

    /// The phone's time zone, as minutes east of UTC.
    #[cfg(target_os = "android")]
    pub fn tz_minutes() -> i32 {
        crate::android::tz_minutes()
    }

    #[cfg(not(target_os = "android"))]
    pub fn tz_minutes() -> i32 {
        0
    }

    /// The app's own settings next to the core's records: one small JSON object of strings.
    #[cfg(feature = "phone")]
    pub(crate) mod saved {
        use std::sync::Mutex;

        static LOCK: Mutex<()> = Mutex::new(());

        fn file() -> Option<std::path::PathBuf> {
            crate::phone::data_dir().map(|d| d.join("app.json"))
        }

        fn read(f: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
            std::fs::read_to_string(f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        }

        pub fn load(key: &str) -> Option<String> {
            let _held = LOCK.lock().ok()?;
            read(&file()?).get(key)?.as_str().map(str::to_owned)
        }

        pub fn save(key: &str, value: &str) {
            let (Ok(_held), Some(f)) = (LOCK.lock(), file()) else { return };
            write(&f, key, value);
        }

        /// [`save`] before the core is installed (the upgrade from the Capacitor app runs then).
        #[cfg_attr(not(target_os = "android"), allow(dead_code))]
        pub fn save_in(dir: &std::path::Path, key: &str, value: &str) {
            let Ok(_held) = LOCK.lock() else { return };
            write(&dir.join("app.json"), key, value);
        }

        /// Sets one value in the file, made with its folder if need be (the core makes the folder
        /// on its own thread, maybe not yet).
        pub(crate) fn write(f: &std::path::Path, key: &str, value: &str) -> bool {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let mut all = read(f);
            all.insert(key.to_owned(), value.into());
            // write then rename, so a crash never leaves half a file
            let tmp = f.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::Value::Object(all).to_string()).is_ok() && std::fs::rename(&tmp, f).is_ok()
        }
    }

    #[cfg(all(feature = "phone", target_os = "android"))]
    pub use saved::save_in;
    #[cfg(feature = "phone")]
    pub use saved::{load, save};

    #[cfg(not(feature = "phone"))]
    pub fn load(_key: &str) -> Option<String> {
        None
    }

    #[cfg(not(feature = "phone"))]
    pub fn save(_key: &str, _value: &str) {}

    #[cfg(not(target_os = "android"))]
    pub fn follow_theme() {}

    /// Puts the saved theme on the page now (after Settings changed it); the browser build does
    /// this in the page, Android re-runs what its head did.
    pub fn apply_theme() {
        #[cfg(all(feature = "phone", target_os = "android"))]
        {
            let t = super::theme_setting();
            let _ = dioxus::document::eval(&format!(
                "var t='{t}',r=t==='system'?(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'):t,d=document.documentElement;window.inyonTheme=t;d.dataset.theme=r;d.style.background=r==='dark'?'#1b1a18':'#e9e8e4';"
            ));
        }
    }

    #[cfg_attr(not(feature = "phone"), allow(unused_variables))]
    pub async fn sleep(ms: u32) {
        #[cfg(feature = "phone")]
        tokio::time::sleep(std::time::Duration::from_millis(ms.into())).await
    }

    /// A request to whatever the app talks to: the phone core in the app, or nothing yet.
    #[cfg(feature = "phone")]
    pub async fn request(method: &str, path: &str, body: Option<String>) -> Result<String, ApiError> {
        crate::phone::request(method, path, body).await
    }

    #[cfg(not(feature = "phone"))]
    pub async fn request(_method: &str, _path: &str, _body: Option<String>) -> Result<String, ApiError> {
        // the desktop build will talk to a hub it was given
        Err(ApiError { status: 0, message: "Only the browser and phone builds reach a hub so far.".into() })
    }
}

/// The theme the saved settings ask for (`inyon.settings.v2`): light, dark or system.
#[cfg(all(feature = "phone", any(target_os = "android", test)))]
pub fn theme_setting() -> &'static str {
    let saved = load("inyon.settings.v2").and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    match saved.as_ref().and_then(|v| v.get("theme")).and_then(|t| t.as_str()) {
        Some("light") => "light",
        Some("dark") => "dark",
        _ => "system",
    }
}

/// The page's head in the Android build: the viewport under the system bars, and the theme set
/// before the first paint (what index.html and /theme.js do in the browser), following the system
/// when it switches.
#[cfg(all(feature = "phone", any(target_os = "android", test)))]
pub fn android_head(theme: &str) -> String {
    let theme = if matches!(theme, "light" | "dark") { theme } else { "system" };
    format!(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content\">\
         <script>(function(){{var t='{theme}',m=matchMedia('(prefers-color-scheme: dark)');\
         function a(){{var u=window.inyonTheme||t,r=u==='system'?(m.matches?'dark':'light'):u,d=document.documentElement;d.dataset.theme=r;d.style.background=r==='dark'?'#1b1a18':'#e9e8e4'}}\
         a();m.addEventListener('change',a)}})()</script>"
    )
}

/// Whether the theme is dark now: set so, or following a system in dark mode.
#[cfg(all(feature = "phone", any(target_os = "android", test)))]
pub fn dark(theme: &str, night: bool) -> bool {
    theme == "dark" || (theme == "system" && night)
}

/// The ground the window shows before the page paints, in the theme's colours.
#[cfg(all(feature = "phone", any(target_os = "android", test)))]
pub fn ground(theme: &str, night: bool) -> (u8, u8, u8, u8) {
    if dark(theme, night) {
        (0x1b, 0x1a, 0x18, 0xff)
    } else {
        (0xe9, 0xe8, 0xe4, 0xff)
    }
}

#[cfg(all(feature = "phone", target_os = "android"))]
pub use imp::save_in;
pub use imp::{apply_theme, load, save, sleep};
// Android sets the theme in the page's head instead (android_head)
#[cfg(not(target_os = "android"))]
pub use imp::follow_theme;
#[cfg(not(target_arch = "wasm32"))]
pub use imp::request;
#[cfg(target_arch = "wasm32")]
pub use imp::{now, random};

pub fn clock() -> Clock {
    Clock { now: imp::now(), tz_minutes: imp::tz_minutes() }
}

#[cfg(all(test, feature = "phone"))]
mod tests {
    use super::*;

    #[test]
    fn the_android_head_sets_only_a_known_theme() {
        assert!(android_head("dark").contains("var t='dark'"));
        assert!(android_head("system").contains("var t='system'"));
        // a theme Settings put on later wins over the one the app opened with
        assert!(android_head("dark").contains("var u=window.inyonTheme||t"));
        // nothing else reaches the script
        assert!(android_head("'</script><script>x").contains("var t='system'"));
        assert!(android_head("light").contains("viewport-fit=cover"));
    }

    #[test]
    fn the_ground_follows_the_theme_then_the_system() {
        assert_eq!(ground("dark", false), (0x1b, 0x1a, 0x18, 0xff));
        assert_eq!(ground("light", true), (0xe9, 0xe8, 0xe4, 0xff));
        assert_eq!(ground("system", true), (0x1b, 0x1a, 0x18, 0xff));
        assert_eq!(ground("system", false), (0xe9, 0xe8, 0xe4, 0xff));
    }

    #[test]
    fn the_system_bars_follow_the_chosen_theme_over_the_system() {
        // a light theme on a phone in dark mode keeps dark icons on the bars, and the other way round
        assert!(!dark("light", true));
        assert!(dark("dark", false));
        assert!(dark("system", true));
        assert!(!dark("system", false));
    }

    #[test]
    fn a_setting_saved_before_the_core_made_its_folder_is_kept() {
        let dir = std::env::temp_dir().join(format!("inyon-app-saved-{}", std::process::id()));
        let f = dir.join("inyon").join("app.json");
        assert!(imp::saved::write(&f, "inyon.keepAlive", "0"));
        assert!(imp::saved::write(&f, "inyon.settings.v2", "{}"));
        let all: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(all, serde_json::json!({ "inyon.keepAlive": "0", "inyon.settings.v2": "{}" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_core_nothing_is_saved_and_the_theme_is_the_systems() {
        save("inyon.settings.v2", r#"{"theme":"dark"}"#);
        assert_eq!(load("inyon.settings.v2"), None);
        assert_eq!(theme_setting(), "system");
    }
}
