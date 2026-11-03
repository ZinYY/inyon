//! Inyon's app in Rust, on Dioxus 0.7: one codebase for the browser, desktop and Android. It is the
//! browser copy the hub serves at /next (sign-in, the Overview and a session's chat, over the same
//! API as the React app in apps/web and the hub's `/ws2` protocol), and the Android app (the
//! `mobile` feature), where the same screens talk to the phone core running in the app. The React
//! app keeps `/` until this one reaches parity.

#[cfg(target_os = "android")]
mod android;
// only the browser build has every part of the wire code in use so far (the phone build most of
// it), so elsewhere some goes unused until the desktop transport lands
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod api;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod connect;
mod dom;
mod format;
mod list;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod machines;
mod markdown;
mod model;
// the Android side drives these; the host build only tests them
#[cfg(feature = "phone")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod notify;
mod pages;
#[cfg(feature = "phone")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod phone;
mod platform;
mod prompt;
// Web Push is the browser build's; the key handling is tested everywhere
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod push;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod route;
mod settings;
mod socket;
#[cfg(feature = "phone")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod ssh_bridge;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod state;
#[cfg(feature = "phone")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod upgrade;
// the terminal drives xterm.js, which only the browser build has
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod term;
mod ui;

#[cfg(not(target_os = "android"))]
fn main() {
    platform::follow_theme();
    dioxus::launch(ui::App);
}

/// Android runs this for each window the app opens; the phone core is already running by then
/// (the Application starts it), so the saved theme is there to read before the first paint.
#[cfg(target_os = "android")]
fn main() {
    let theme = platform::theme_setting();
    let cfg = dioxus::mobile::Config::new().with_custom_head(platform::android_head(theme)).with_background_color(platform::ground(theme, android::night()));
    dioxus::LaunchBuilder::new().with_cfg(cfg).launch(ui::App);
}
