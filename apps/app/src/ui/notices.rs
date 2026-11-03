//! The app's own settings (Settings › Appearance and Notifications, kept in the browser as the web
//! app keeps them), what the hub's notices turn into (lib/store.ts's notify, page.notify and
//! companion.reply: a toast, a buzz, or the browser's own notification while the app is hidden),
//! and "Inyon was updated" (components/UpdatePrompt.tsx) once the hub runs newer code than this page.

use dioxus::prelude::*;
use inyon_chat_core::overview::{all_panes, PaneView};
use inyon_chat_core::store::{Effect, Haptic, Settings, Tone};
use inyon_chat_core::Context;
use inyon_protocol::{HostState, ServerMsg};

use super::icons::{Glyph, Icon};
use super::toast::{self, Toast, ToastTone};
use crate::route::{Route, BASE};
use crate::settings::{self as st, Build, Permission};
use crate::{api, dom, platform, state};

/// Where the web app keeps its settings; this app reads and writes the same.
pub const SETTINGS_KEY: &str = "inyon.settings.v2";

fn saved() -> Settings {
    platform::load(SETTINGS_KEY).map(|s| Settings::load(&s)).unwrap_or_default()
}

/// The settings as they are now; a change redraws what reads them.
pub static PREFS: GlobalSignal<Settings> = Signal::global(saved);

/// The saved settings with `next` over them, keeping keys this app doesn't know.
pub fn merged_json(saved: Option<&str>, next: &Settings) -> String {
    let mut base = saved.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    if let (Some(o), Ok(serde_json::Value::Object(n))) = (base.as_object_mut(), serde_json::to_value(next)) {
        o.extend(n);
    }
    base.to_string()
}

/// Changes a setting, saves it and puts it to work (the theme at once).
pub fn set_prefs(change: impl FnOnce(&mut Settings)) {
    let before = PREFS.peek().clone();
    let mut next = before.clone();
    change(&mut next);
    if next == before {
        return;
    }
    platform::save(SETTINGS_KEY, &merged_json(platform::load(SETTINGS_KEY).as_deref(), &next));
    *PREFS.write() = next.clone();
    if next.theme != before.theme {
        platform::apply_theme();
    }
    if next.haptics && !before.haptics {
        haptic(Haptic::Success);
    }
}

/// Two short pulses for a confirmed outcome, a longer pair for a warning, when haptics are on.
pub fn haptic(kind: Haptic) {
    if !PREFS.peek().haptics {
        return;
    }
    dom::vibrate(match kind {
        Haptic::Success => &[10, 40, 14],
        Haptic::Warning => &[18, 60, 18],
    });
}

/// A route as the notices compare it: without the base path.
fn route_path(r: &Route) -> String {
    let href = r.href();
    let path = href.strip_prefix(BASE).unwrap_or(&href);
    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() {
        "/".into()
    } else {
        path.into()
    }
}

/// A notice from the hub: a toast unless what it's about is on screen, a buzz, and the browser's
/// notification while the app is hidden (when it's allowed).
pub fn handle(msg: &ServerMsg, here: &Route, hosts: &[HostState]) {
    let path = route_path(here);
    let terminal_view = terminal_on_screen(here, hosts);
    let ctx = Context { route: &path, terminal_view, visible: dom::visible(), native: false, browser_notices: dom::notice_permission() == Permission::Granted };
    let prefs = PREFS.peek().clone();
    let (effects, toasts) = state::with_feed(|f| f.notice(msg, &prefs, &ctx, platform::clock().now as u64));
    for t in toasts {
        toast::show(Toast {
            id: 0,
            title: t.title,
            body: t.body.filter(|b| !b.is_empty()),
            tone: match t.tone {
                Tone::Info => ToastTone::Info,
                Tone::Success => ToastTone::Success,
                Tone::Warn => ToastTone::Warn,
                Tone::Error => ToastTone::Error,
            },
            href: t.href.map(|h| format!("{BASE}{h}")),
            action: None,
            agent: t.agent,
        });
    }
    for e in effects {
        match e {
            Effect::Haptic(k) => haptic(k),
            Effect::BrowserNotify { title, body, href, tag } => {
                dom::show_notice(&title, &body, &tag, Some(&format!("{BASE}{href}")));
            }
            Effect::ClearNotification { tag } => dom::close_notice(&tag),
            Effect::Toast { .. } | Effect::NativeNotify { .. } => {}
        }
    }
}

/// Whether a session's terminal is what's on screen: the view the address names, else the pane's
/// own (the terminal, for an agent Inyon doesn't read as chat).
fn terminal_on_screen(here: &Route, hosts: &[HostState]) -> bool {
    let Route::Session { host, pane, view } = here else { return false };
    let r = all_panes(hosts).into_iter().find(|r| r.host.config.id == *host && r.pane.id == *pane);
    let agent = r.as_ref().and_then(|r| r.pane.agent.as_ref().map(|a| a.kind));
    super::session::shown_view(*view, agent, r.is_some()) == Some(PaneView::Terminal)
}

// ---------- "Inyon was updated" ----------

/// What the hub runs, as it last said.
pub static HUB_BUILD: GlobalSignal<Option<Build>> = Signal::global(|| None);
/// The commit the hub ran when this page loaded: this page's own, since the hub served it.
static PAGE_COMMIT: GlobalSignal<Option<Option<String>>> = Signal::global(|| None);

/// This page's own code: the version it was built as, and the commit the hub ran when it loaded.
pub fn page_build() -> Build {
    Build { version: env!("CARGO_PKG_VERSION").into(), commit: PAGE_COMMIT.peek().clone().flatten() }
}

/// Asks the hub what it runs now.
pub fn check_build() {
    spawn(async move {
        let Ok(s) = api::session().await else { return };
        let Some(version) = s.version.filter(|v| !v.is_empty()) else { return };
        let b = Build { version, commit: s.commit.filter(|c| !c.is_empty()) };
        if PAGE_COMMIT.peek().is_none() {
            // the first answer: the build that served this page, when it's the same version
            *PAGE_COMMIT.write() = Some(if b.version == env!("CARGO_PKG_VERSION") { b.commit.clone() } else { None });
        }
        if HUB_BUILD.peek().as_ref() != Some(&b) {
            *HUB_BUILD.write() = Some(b);
        }
    });
}

/// Offered once per hub build, and out of the way: nothing is blocked, and closing it is the end
/// of it.
#[component]
pub fn UpdatePrompt() -> Element {
    let mut shown = use_signal(|| None::<Build>);
    let mut closing = use_signal(|| false);
    let hub = HUB_BUILD();
    use_effect(use_reactive!(|hub| {
        let Some(h) = hub else { return };
        if !st::reload_due(&h, &page_build(), platform::load(st::RELOAD_SEEN).as_deref()) {
            return;
        }
        platform::save(st::RELOAD_SEEN, &st::build_key(&h));
        closing.set(false);
        shown.set(Some(h));
    }));
    if shown().is_none() {
        return rsx! {};
    }
    rsx! {
        div { class: "update-prompt",
            div { class: if closing() { "update-prompt-card fade-out" } else { "update-prompt-card toast-in" }, role: "status",
                Glyph { icon: Icon::RotateCw, size: 16, class: "fg-3" }
                div { class: "update-prompt-words",
                    p { class: "update-prompt-title", "Inyon was updated" }
                    p { class: "update-prompt-text", "Reload to get the new version." }
                }
                button { class: "button button-sm button-primary press", r#type: "button", onclick: move |_| dom::reload(), "Reload" }
                button {
                    class: "icon-button press",
                    r#type: "button",
                    "aria-label": "Not now",
                    title: "Not now",
                    onclick: move |_| {
                        closing.set(true);
                        spawn(async move {
                            platform::sleep(200).await;
                            shown.set(None);
                            closing.set(false);
                        });
                    },
                    Glyph { icon: Icon::X, size: 16 }
                }
            }
        }
    }
}

/// Hands the push subscription to the hub at start (subscriptions can change) and whenever the
/// notice switches change, so a closed Inyon is told what an open one would show.
#[component]
pub fn PushWatch() -> Element {
    let (done, pages) = {
        let p = PREFS.read();
        (p.notify_done, p.notify_pages)
    };
    use_hook(|| {
        if crate::push::supported() {
            crate::push::listen();
        }
    });
    use_effect(use_reactive!(|done, pages| {
        if crate::push::supported() {
            spawn(async move {
                let _ = crate::push::enable(done, pages).await;
            });
        }
    }));
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_chat_core::store::Theme;
    use inyon_protocol::NotifyKind;
    use serde_json::json;

    #[test]
    fn saving_a_setting_keeps_the_ones_this_app_does_not_know() {
        let next = Settings { theme: Theme::Dark, ..Settings::default() };
        let out: serde_json::Value = serde_json::from_str(&merged_json(Some(r#"{"theme":"light","sidePanelWidth":420}"#), &next)).unwrap();
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["sidePanelWidth"], 420);
        let out: serde_json::Value = serde_json::from_str(&merged_json(Some("not json"), &next)).unwrap();
        assert_eq!(out["notifyDone"], true);
    }

    #[test]
    fn notices_compare_the_route_without_the_base_path() {
        assert_eq!(route_path(&Route::Overview), "/");
        assert_eq!(route_path(&Route::Session { host: "box".into(), pane: "%3".into(), view: Some(PaneView::Terminal) }), "/m/box/p/3");
        assert_eq!(route_path(&Route::Pages { page: Some(("local".into(), "plan".into())) }), "/pages/local/plan");
    }

    fn done(pane: &str) -> ServerMsg {
        ServerMsg::Notify { host: "box".into(), pane: pane.into(), kind: NotifyKind::Done, title: "Claude Code finished".into(), body: "Fixed the test".into(), agent: None }
    }

    #[test]
    fn a_finished_agent_is_a_toast_unless_its_chat_is_open_or_finished_notices_are_off() {
        let mut feed = state::Feed::default();
        let home = Context { route: "/", terminal_view: false, visible: true, native: false, browser_notices: false };
        let (_, toasts) = feed.notice(&done("%1"), &Settings::default(), &home, 1);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].href.as_deref(), Some("/m/box/p/1"));
        let open = Context { route: "/m/box/p/2", ..home };
        assert!(feed.notice(&done("%2"), &Settings::default(), &open, 2).1.is_empty());
        let off = Settings { notify_done: false, ..Settings::default() };
        assert!(feed.notice(&done("%3"), &off, &home, 3).1.is_empty());
        // hidden, with the browser's permission: its own notification
        let hidden = Context { visible: false, browser_notices: true, ..home };
        let (effects, _) = feed.notice(&done("%4"), &Settings::default(), &hidden, 4);
        assert!(effects.iter().any(|e| matches!(e, Effect::BrowserNotify { tag, .. } if tag == "box:%4")));
    }

    #[test]
    fn a_terminal_on_screen_is_the_view_the_session_shows() {
        let pane = |id: &str, kind: &str| json!({ "id": id, "index": 0, "active": true, "command": kind, "cwd": "/home/sam", "title": "t", "width": 80, "height": 40, "dead": false, "agent": { "kind": kind, "pid": 1, "state": "idle" } });
        let hosts: Vec<HostState> = vec![serde_json::from_value(json!({ "config": { "id": "box", "name": "box", "kind": "node" }, "status": "online", "integration": "installed", "tmux": "ok", "agents": [], "updatedAt": 0,
            "sessions": [{ "id": "$1", "name": "a", "attached": 0, "windows": [{ "id": "@1", "index": 1, "name": "", "active": true, "activity": 0, "panes": [pane("%1", "claude"), pane("%2", "cursor")] }] }] }))
        .unwrap()];
        let at = |pane: &str, view: Option<PaneView>| Route::Session { host: "box".into(), pane: pane.into(), view };
        // Claude Code opens on its chat, Cursor (not read as chat) on its terminal
        assert!(!terminal_on_screen(&at("%1", None), &hosts));
        assert!(terminal_on_screen(&at("%1", Some(PaneView::Terminal)), &hosts));
        assert!(terminal_on_screen(&at("%2", None), &hosts));
        assert!(!terminal_on_screen(&Route::Overview, &hosts));
    }

    #[test]
    fn only_notices_are_read_from_outside_the_streams() {
        let text = r#"{"t":"page.notify","host":"local","slug":"plan","title":"Plan","body":"3 of 12 done"}"#;
        assert!(matches!(state::notice_msg("page.notify", text), Some(ServerMsg::PageNotify { .. })));
        assert!(state::notice_msg("pong", r#"{"t":"pong"}"#).is_none());
    }
}
