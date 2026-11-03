//! A session's screen (PanePage.tsx and SessionHeader.tsx): the header with the agent, the title,
//! where the pane is and its state, then the chat. Opening it follows the chat's `/ws2` stream from
//! where this app left it; leaving stops following it. Chat and Terminal switch between the two
//! views; a plain shell, or an agent that stays in its terminal, opens on the terminal.

use dioxus::prelude::*;
use inyon_chat_core::format::model_name;
use inyon_chat_core::overview::{all_panes, pane_href, pane_title, terminal_name, PaneView};
use inyon_chat_core::store::{ReplyFontSetting, Settings, ToolDetail};
use inyon_protocol::{chat_key, AgentDef, AgentKind, ClientMsg2, ReplyFont, SkinTokens};

use super::chat::ChatView;
use super::icons::{AgentMark, Glyph, Icon};
use super::outbox;
use super::rows::Ctx;
use super::terminal::TerminalView;
use super::toast::{self, confirm, Ask, ToastTone};
use crate::model::Shown;
use crate::route::BASE;
use crate::state::{self, Chats, Hub};
use crate::{dom, format, platform};

// each view rises in over the one it replaces
const CHAT_KEY: &str = "chat";
const TERMINAL_KEY: &str = "terminal";

/// "Opus 4.8 · high · Plan": the model, effort and mode, as far as they're known.
pub fn controls_line(model: Option<&str>, effort: Option<&str>, mode: Option<&str>, def: Option<&AgentDef>) -> String {
    let mode = mode.filter(|m| !m.is_empty()).map(|m| def.and_then(|d| d.modes.iter().find(|o| o.id == m)).map_or(m.to_string(), |o| o.label.clone()));
    [Some(model_name(model)), effort.map(str::to_string), mode].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The reply font's class: the setting, or the agent's own when it's on Auto.
pub fn font_class(setting: ReplyFontSetting, def: Option<&AgentDef>) -> &'static str {
    let font = match setting {
        ReplyFontSetting::Serif => ReplyFont::Serif,
        ReplyFontSetting::Sans => ReplyFont::Sans,
        ReplyFontSetting::Mono => ReplyFont::Mono,
        ReplyFontSetting::Auto => def.map_or(ReplyFont::Sans, |d| d.reply_font),
    };
    match font {
        ReplyFont::Serif => "reply-serif",
        ReplyFont::Mono => "reply-mono",
        ReplyFont::Sans => "",
    }
}

fn skin_vars(t: &SkinTokens) -> String {
    let pairs = [
        ("bg", &t.bg),
        ("panel", &t.panel),
        ("raised", &t.raised),
        ("hover", &t.hover),
        ("line", &t.line),
        ("line-strong", &t.line_strong),
        ("fg", &t.fg),
        ("fg-2", &t.fg2),
        ("fg-3", &t.fg3),
        ("accent", &t.accent),
        ("accent-fg", &t.accent_fg),
        ("accent-soft", &t.accent_soft),
        ("accent-text", &t.accent_text),
        ("bubble", &t.bubble),
        ("code", &t.code),
    ];
    pairs.iter().map(|(k, v)| format!("--{k}:{v};")).collect()
}

/// The agent's skin for everything inside the session (lib/skin.tsx), light and dark, so it follows
/// the theme as it changes.
pub fn skin_css(kind: AgentKind) -> String {
    let s = &kind.def().skin;
    let k = kind.as_str();
    format!(".skin-{k}{{{}background:var(--bg);color:var(--fg)}}[data-theme='dark'] .skin-{k}{{{}}}", skin_vars(&s.light), skin_vars(&s.dark))
}

#[component]
pub fn Session(host: String, pane: String, view: Option<PaneView>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut chats = use_context::<Signal<Chats>>();
    let out = outbox();
    let key = chat_key(&host, &pane);
    let settings: Settings = super::notices::PREFS.read().clone();

    // follow the chat while it's on screen, from where this app left it
    {
        let (key, out) = (key.clone(), out.clone());
        use_hook(move || {
            let mut c = chats.write();
            let e = c.entry(&key);
            e.followed = true;
            if hub.peek().connected {
                out.send(state::wire2(&ClientMsg2::Sub { stream: key.clone(), at: e.chat.position() }));
            }
        });
    }
    {
        let (key, out) = (key.clone(), out.clone());
        use_drop(move || {
            if let Some(e) = chats.write().map.get_mut(&key) {
                e.followed = false;
            }
            out.send(state::wire2(&ClientMsg2::Unsub { stream: key.clone() }));
        });
    }
    let h = hub.read();
    let c = chats.read();
    let info = c.get(&key).and_then(|e| e.chat.info.clone());
    let screen = c.get(&key).and_then(|e| e.chat.screen.clone());
    let refs = all_panes(&h.hosts);
    let r = refs.iter().find(|r| r.host.config.id == host && r.pane.id == pane);
    let agent_info = r.and_then(|r| r.pane.agent.as_ref());
    let agent: Option<AgentKind> = agent_info.map(|a| a.kind).or(info.as_ref().and_then(|i| i.agent));
    let def = agent.map(|k| k.def());
    let agent_name = agent_info.and_then(|a| a.custom.clone()).or(def.map(|d| d.name.clone())).unwrap_or_else(|| "the agent".into());
    let title = r
        .and_then(|r| r.pane.label.clone().filter(|l| !l.is_empty()).or_else(|| (r.pane.companion == Some(true)).then(|| "Companion".into())))
        .or_else(|| info.as_ref().and_then(|i| i.title.clone()).filter(|t| !t.is_empty()))
        .or_else(|| r.map(|r| if r.pane.agent.is_some() { pane_title(r) } else { terminal_name(r).map_or_else(|| pane_title(r), str::to_string) }))
        .unwrap_or_else(|| "Session".into());
    let cwd = info.as_ref().and_then(|i| i.cwd.clone()).or(r.map(|r| r.pane.cwd.clone()));
    let cwd_full = cwd.clone();
    let session_id = page_session(agent_info.and_then(|a| a.session_id.as_deref()), info.as_ref().and_then(|i| i.session_id.as_deref()));
    let subtitle = [
        Some(def.map_or("Terminal".to_string(), |d| d.name.clone())),
        r.map(|r| r.host.config.name.clone()),
        r.map(|r| format!("{}:{}", r.session.name, r.window.index)),
        cwd.map(|p| format::short_path(&p, 28)).filter(|p| !p.is_empty()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let shown = r.and_then(|r| r.pane.agent.as_ref().map(|a| Shown::of(a, r.host)));
    let controls = controls_line(
        screen.as_ref().and_then(|s| s.model.as_deref()).or(info.as_ref().and_then(|i| i.model.as_deref())),
        screen.as_ref().and_then(|s| s.effort.as_deref()),
        screen.as_ref().and_then(|s| s.mode.as_deref()).or(info.as_ref().and_then(|i| i.mode.as_deref())),
        def,
    );
    let gone = h.loaded && r.is_none() && info.is_none();
    let skin = agent.filter(|_| settings.agent_skins);
    let ctx = Ctx {
        key: key.clone(),
        host: host.clone(),
        pane: pane.clone(),
        agent,
        agent_name: agent_name.clone(),
        font: font_class(settings.reply_font, def),
        all_tools: settings.tool_detail != ToolDetail::Compact,
    };
    let connected = h.connected && r.is_none_or(|r| r.host.status == inyon_protocol::HostStatus::Online);
    let home = home_view(agent);
    let shows = shown_view(view, agent, r.is_some() || info.is_some());
    // each view's address: the pane's own view goes without ?view=
    let view_href = |v: PaneView| format!("{BASE}{}", pane_href(&host, &pane, (v != home).then_some(v)));
    let (chat_href, terminal_href) = (view_href(PaneView::Chat), view_href(PaneView::Terminal));
    dom::set_title(&format!("{title} · Inyon"));
    // the header's ⋯, as the web app's session menu
    let now = super::session_menu::Now {
        session_id: session_id.clone(),
        remote: info.as_ref().and_then(|i| i.rc.as_ref()).map(|rc| rc.url.clone()),
        ultracode: screen.as_ref().and_then(|s| s.ultracode).or(agent_info.and_then(|a| a.ultracode)).or(info.as_ref().and_then(|i| i.ultracode)) == Some(true),
        shared: info.as_ref().is_some_and(|i| i.share.is_some()),
    };
    let more = super::session_menu::items(&host, &pane, agent, &now);
    let mut menu_at = use_context::<Signal<Option<super::menu::Menu>>>();
    let has_pane = r.is_some();
    drop(c);
    drop(h);

    rsx! {
        if let Some(kind) = skin {
            style { {skin_css(kind)} }
        }
        div { class: match skin { Some(k) => format!("session skin-{}", k.as_str()), None => "session".into() },
            header { class: "session-head",
                div { class: "session-head-row",
                    a { class: "back-link press", href: "/next/", "aria-label": "Back",
                        Glyph { icon: Icon::ChevronLeft, size: 20 }
                    }
                    if let Some(kind) = agent {
                        span { class: "session-logo", AgentMark { kind, size: 26 } }
                    }
                    div { class: "session-titles",
                        h1 { class: "session-title",
                            if let Some(s) = shown {
                                span { class: "dot dot-{s.class()} phone-dot" }
                            }
                            span { class: "truncate", "{title}" }
                        }
                        p { class: "session-sub mono truncate", "{subtitle}" }
                    }
                    super::pages::SessionPages { host: host.clone(), cwd: cwd_full.clone(), session: session_id.clone() }
                    if let Some(s) = shown {
                        span { class: "pill session-pill",
                            span { class: "dot dot-{s.class()}" }
                            "{s.label()}"
                        }
                    }
                    if has_pane {
                        button {
                            id: "session-more",
                            class: "icon-button icon-button-lg press",
                            r#type: "button",
                            "aria-label": "More",
                            title: "More",
                            onclick: move |_| super::menu::drop_down(&mut menu_at, "session-more", more.clone()),
                            Glyph { icon: Icon::Ellipsis, size: 17 }
                        }
                    }
                    // a plain shell has only its terminal
                    if agent.is_some() {
                        nav { class: "segmented session-views", "aria-label": "View",
                            ViewSegment { href: chat_href, on: shows == Some(PaneView::Chat), icon: Icon::MessageSquare, word: "Chat" }
                            ViewSegment { href: terminal_href, on: shows == Some(PaneView::Terminal), icon: Icon::SquareTerminal, word: "Terminal" }
                        }
                    }
                }
            }
            if gone {
                div { class: "chat-column",
                    div { class: "empty chat-empty",
                        div { class: "empty-title", "This pane is gone" }
                        div { class: "empty-text", "Its tmux pane closed, or its machine no longer lists it." }
                        a { class: "button button-md button-primary press empty-action", href: "/next/", "Back to the Overview" }
                    }
                }
            } else if shows == Some(PaneView::Terminal) {
                div { key: "{TERMINAL_KEY}", class: "session-view fade-in", TerminalView { host: host.clone(), pane: pane.clone() } }
            } else if shows == Some(PaneView::Chat) {
                div { key: "{CHAT_KEY}", class: "session-view fade-in", ChatView { ctx, connected, controls } }
            } else {
                div { class: "session-view" }
            }
        }
    }
}

/// Asks, then ends a session: it leaves every list at once with a notice, while the machine stops
/// the agent and closes its pane. A failure brings it back with the reason. `back` goes to the
/// Overview once it's gone (from the session's own screen).
pub async fn end_session(mut hub: Signal<Hub>, host: &str, pane: &str, back: Option<Signal<crate::route::Route>>) -> bool {
    let (ask, found) = {
        let h = hub.peek();
        let refs = all_panes(&h.hosts);
        let Some(r) = refs.iter().find(|r| r.host.config.id == host && r.pane.id == pane) else { return false };
        let agent = r.pane.agent.as_ref().map(|a| a.custom.clone().unwrap_or_else(|| a.kind.def().name.clone()));
        let name = if r.pane.agent.is_some() { pane_title(r) } else { terminal_name(r).map_or_else(|| r.pane.command.clone(), str::to_string) };
        let body = crate::settings::end_text(agent.as_deref(), r.window.panes.len(), r.session.windows.len(), &r.session.name, r.window.index.0 as u32);
        let ask = Ask {
            title: "End this session?".into(),
            details: vec![("Session".into(), name), ("Machine".into(), r.host.config.name.clone()), ("tmux".into(), inyon_chat_core::groups::tmux_target(r))],
            body: Some(body),
            yes: "End session".into(),
            danger: true,
        };
        (ask, true)
    };
    if !found || !confirm(ask).await {
        return false;
    }
    // off every list at once; said to be ended once the machine has taken it
    let now = platform::clock().now as u64;
    let hosts = state::with_feed(|f| f.hide_pane(host, pane, true, now));
    hub.write().set_hosts(hosts);
    let answer = match crate::api::end(host, pane).await {
        // a pane the hub no longer reports closed while the question was open: it has ended
        Err(e) if e.status == 404 && state::with_feed(|f| f.reports_pane(host, pane)) == Some(false) => Ok(()),
        other => other,
    };
    if let Err(e) = answer {
        let hosts = state::with_feed(|f| f.hide_pane(host, pane, false, platform::clock().now as u64));
        hub.write().set_hosts(hosts);
        toast::toast("Could not end the session", Some(e.message), ToastTone::Error);
        return false;
    }
    super::notices::haptic(inyon_chat_core::store::Haptic::Success);
    toast::toast("Session ended", None, ToastTone::Success);
    // still on the session's screen: somewhere else meanwhile stays where it is
    if let Some(mut here) = back.filter(|h| matches!(&*h.peek(), crate::route::Route::Session { host: h, pane: p, .. } if h == host && p == pane)) {
        crate::route::go(&mut here, crate::route::Route::Overview);
    }
    true
}

/// The view on screen: the one the address names, else the pane's own once Inyon knows what runs
/// in it. Until then none, so an agent's pane doesn't open (and resize) its terminal while the
/// hub's list is still on its way.
pub fn shown_view(view: Option<PaneView>, agent: Option<AgentKind>, known: bool) -> Option<PaneView> {
    view.or_else(|| known.then(|| home_view(agent)))
}

/// The view a pane opens on: the chat for an agent Inyon reads as chat, the terminal otherwise.
pub fn home_view(agent: Option<AgentKind>) -> PaneView {
    if agent.is_some_and(|k| k.def().chat) {
        PaneView::Chat
    } else {
        PaneView::Terminal
    }
}

/// One of the header's two views: a link to it, or the one on screen.
#[component]
fn ViewSegment(href: String, on: bool, icon: Icon, word: &'static str) -> Element {
    rsx! {
        if on {
            span { class: "segment segment-on", "aria-current": "page",
                Glyph { icon, size: 14 }
                span { class: "segment-word", "{word}" }
            }
        } else {
            a { class: "segment", href: "{href}", title: "{word}",
                Glyph { icon, size: 14 }
                span { class: "segment-word", "{word}" }
            }
        }
    }
}

/// The conversation a session's pages are matched by: the pane's agent's, else the chat's own (which
/// the hub fills when the pane state has none).
fn page_session(pane: Option<&str>, chat: Option<&str>) -> Option<String> {
    pane.or(chat).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sessions_pages_follow_the_chats_conversation_when_the_pane_has_none() {
        assert_eq!(page_session(Some("a"), Some("b")).as_deref(), Some("a"));
        assert_eq!(page_session(None, Some("b")).as_deref(), Some("b"));
        assert_eq!(page_session(None, None), None);
    }

    #[test]
    fn the_controls_read_as_one_line() {
        let claude = AgentKind::Claude.def();
        assert_eq!(controls_line(Some("claude-opus-4-6"), Some("high"), None, Some(claude)), "Opus 4.6 · high");
        assert_eq!(controls_line(None, None, None, None), "");
        let mode = claude.modes.first().expect("Claude Code has modes");
        assert_eq!(controls_line(None, None, Some(&mode.id), Some(claude)), mode.label);
        assert_eq!(controls_line(None, None, Some("custom"), None), "custom");
    }

    #[test]
    fn replies_keep_their_agents_font() {
        let claude = AgentKind::Claude.def();
        assert_eq!(font_class(ReplyFontSetting::Auto, Some(claude)), "reply-serif");
        assert_eq!(font_class(ReplyFontSetting::Mono, Some(claude)), "reply-mono");
        assert_eq!(font_class(ReplyFontSetting::Auto, None), "");
    }

    #[test]
    fn a_pane_opens_on_the_view_its_agent_has() {
        assert_eq!(home_view(Some(AgentKind::Claude)), PaneView::Chat);
        assert_eq!(home_view(None), PaneView::Terminal);
        // an agent Inyon doesn't read as chat stays in its terminal
        let k = inyon_protocol::AGENT_KINDS.into_iter().find(|k| !k.def().chat).expect("a terminal-first agent");
        assert_eq!(home_view(Some(k)), PaneView::Terminal);
    }

    #[test]
    fn a_pane_waits_to_know_its_agent_before_it_picks_a_view() {
        // the hub's list not in yet: nothing, rather than a terminal that a chat replaces
        assert_eq!(shown_view(None, None, false), None);
        assert_eq!(shown_view(None, Some(AgentKind::Claude), true), Some(PaneView::Chat));
        assert_eq!(shown_view(None, None, true), Some(PaneView::Terminal));
        // an address that names its view needs no wait
        assert_eq!(shown_view(Some(PaneView::Terminal), None, false), Some(PaneView::Terminal));
        assert_eq!(shown_view(Some(PaneView::Terminal), Some(AgentKind::Claude), true), Some(PaneView::Terminal));
    }

    #[test]
    fn a_skin_covers_both_themes() {
        let css = skin_css(AgentKind::Claude);
        let s = &AgentKind::Claude.def().skin;
        assert!(css.starts_with(".skin-claude{"), "{css}");
        assert!(css.contains(&format!("--accent:{};", s.light.accent)));
        assert!(css.contains(&format!("[data-theme='dark'] .skin-claude{{--bg:{};", s.dark.bg)));
    }
}
