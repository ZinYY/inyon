//! A session's terminal view (TerminalView.tsx): the pane's real tmux window, drawn by xterm.js,
//! with the key bar a phone keyboard needs under it. It opens with the hub when the socket is up,
//! opens again after the socket comes back, lets go of the window when the page has been hidden for
//! a while (so an agent's window rests at 200×50) and takes it back when the page is shown, and
//! says so when tmux lets go of it, with Reconnect.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use inyon_protocol::ClientMsg;

use super::icons::{Glyph, Icon};
use super::menu::{self, Action, Item};
use super::outbox;
use crate::socket::Outbox;
use crate::state::{self, Hub};
use crate::term::{self, MenuOp, KEYS};
use crate::{dom, platform};

const SCREEN: &str = "term-screen";
const SETTINGS: &str = "inyon.settings.v2";

/// Where the view is, for what it shows over the terminal.
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Loading,
    Open,
    /// tmux let go (the reason the hub gave), or term.ts couldn't load
    Closed(String),
    Failed(String),
}

/// What reaches the view from outside Dioxus (xterm.js, the hub, the page), in order.
enum Ev {
    Data(String),
    Resize(u16, u16),
    Exit(String),
    Visible(bool),
    /// the page has been hidden for [`term::REST_AFTER_MS`]: let go of the window
    Rest(u64),
}

#[cfg(target_arch = "wasm32")]
struct Live {
    x: term::js::Xterm,
    /// the hub's terminal, while it has one open for this view
    id: Option<String>,
    resting: bool,
    _data: wasm_bindgen::closure::Closure<dyn FnMut(String)>,
    _resize: wasm_bindgen::closure::Closure<dyn FnMut(u16, u16)>,
    /// the page hidden and shown again
    visibility: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Live {
    id: Option<String>,
    resting: bool,
}

type Shared = Rc<RefCell<Option<Live>>>;
type Tx = futures_channel::mpsc::UnboundedSender<Ev>;

fn font_size() -> f64 {
    platform::load(SETTINGS).map(|s| inyon_chat_core::store::Settings::load(&s)).unwrap_or_default().term_font_size.clamp(term::FONT_MIN, term::FONT_MAX)
}

/// Asks the hub for a terminal on the pane, at the size the view has, under a new id.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
fn attach(live: &Shared, out: &Outbox, tx: &Tx, host: &str, pane: &str) {
    let mut l = live.borrow_mut();
    let Some(l) = l.as_mut() else { return };
    if let Some(old) = l.id.take() {
        term::unregister(&old);
    }
    l.resting = false;
    #[cfg(target_arch = "wasm32")]
    {
        let id = dom::random_id();
        // tmux draws the whole window again for the new client
        l.x.reset();
        let (x, tx) = (l.x.clone(), tx.clone());
        term::register(&id, move |ev| match ev {
            term::HubEvent::Data(d) => x.write(&d),
            term::HubEvent::Exit(reason) => drop(tx.unbounded_send(Ev::Exit(reason))),
        });
        out.send(state::wire(&ClientMsg::TermOpen { id: id.clone(), host: host.into(), pane: pane.into(), cols: l.x.cols(), rows: l.x.rows() }));
        l.id = Some(id);
    }
}

/// Lets go of the hub's terminal, if there is one.
fn detach(live: &Shared, out: &Outbox) {
    let mut l = live.borrow_mut();
    if let Some(id) = l.as_mut().and_then(|l| l.id.take()) {
        term::unregister(&id);
        out.send(state::wire(&ClientMsg::TermClose { id }));
    }
}

#[component]
pub fn TerminalView(host: String, pane: String) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let out = outbox();
    let mut phase = use_signal(|| Phase::Loading);
    // the key bar's Ctrl and Alt, down until the next key
    let mut mods = use_signal(|| (false, false));
    let mut font = use_signal(font_size);
    let live: Shared = use_hook(|| Rc::new(RefCell::new(None)));
    let (tx, rx) = use_hook(|| {
        let (tx, rx) = futures_channel::mpsc::unbounded::<Ev>();
        (tx, Rc::new(RefCell::new(Some(rx))))
    });
    // counts hidings, so a rest timer from an earlier one does nothing
    let hidden = use_hook(|| Rc::new(std::cell::Cell::new(0u64)));

    // xterm.js into the screen, then the hub's terminal on it
    {
        let (live, tx, out, host, pane) = (live.clone(), tx.clone(), out.clone(), host.clone(), pane.clone());
        use_effect(move || {
            let (live, tx, out, host, pane) = (live.clone(), tx.clone(), out.clone(), host.clone(), pane.clone());
            spawn(async move {
                #[cfg(target_arch = "wasm32")]
                {
                    use wasm_bindgen::{closure::Closure, JsCast};
                    let api = match term::js::load().await {
                        Ok(a) => a,
                        Err(e) => return phase.set(Phase::Failed(e)),
                    };
                    let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(SCREEN)).and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                    else {
                        return;
                    };
                    let t = tx.clone();
                    let data = Closure::<dyn FnMut(String)>::new(move |d: String| drop(t.unbounded_send(Ev::Data(d))));
                    let t = tx.clone();
                    let resize = Closure::<dyn FnMut(u16, u16)>::new(move |c: u16, r: u16| drop(t.unbounded_send(Ev::Resize(c, r))));
                    let x = api.open(&el, *font.peek(), data.as_ref().unchecked_ref(), resize.as_ref().unchecked_ref());
                    term::js::set_screen(Some(x.clone()));
                    if !dom::coarse() {
                        x.focus();
                    }
                    let t = tx.clone();
                    let visibility = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| drop(t.unbounded_send(Ev::Visible(dom::visible()))));
                    if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                        let _ = d.add_event_listener_with_callback("visibilitychange", visibility.as_ref().unchecked_ref());
                    }
                    *live.borrow_mut() = Some(Live { x, id: None, resting: false, _data: data, _resize: resize, visibility });
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    *live.borrow_mut() = Some(Live { id: None, resting: false });
                }
                phase.set(Phase::Open);
                if hub.peek().connected {
                    attach(&live, &out, &tx, &host, &pane);
                }
            });
        });
    }

    // the socket came back: the hub forgot its terminals, so open one again
    {
        let (live, tx, out, host, pane) = (live.clone(), tx.clone(), out.clone(), host.clone(), pane.clone());
        use_effect(move || {
            let up = hub.read().connected;
            let ready = *phase.peek() == Phase::Open;
            let resting = live.borrow().as_ref().is_some_and(|l| l.resting);
            if !up {
                if let Some(l) = live.borrow_mut().as_mut() {
                    if let Some(id) = l.id.take() {
                        term::unregister(&id);
                    }
                }
            } else if ready && !resting && dom::visible() && live.borrow().as_ref().is_some_and(|l| l.id.is_none()) {
                attach(&live, &out, &tx, &host, &pane);
            }
        });
    }

    // what xterm.js, the hub and the page say, one at a time
    {
        let (live, tx, out, host, pane, hidden) = (live.clone(), tx.clone(), out.clone(), host.clone(), pane.clone(), hidden.clone());
        let rx = rx.clone();
        use_future(move || {
            let (live, tx, out, host, pane, hidden) = (live.clone(), tx.clone(), out.clone(), host.clone(), pane.clone(), hidden.clone());
            let rx = rx.borrow_mut().take();
            async move {
                use futures_util::StreamExt;
                let Some(mut rx) = rx else { return };
                while let Some(ev) = rx.next().await {
                    match ev {
                        Ev::Data(d) => {
                            if term::is_color_reply(&d) {
                                continue;
                            }
                            let (ctrl, alt) = *mods.peek();
                            let d = if ctrl || alt {
                                mods.set((false, false));
                                term::modded(&d, ctrl, alt)
                            } else {
                                d
                            };
                            if let Some(id) = live.borrow().as_ref().and_then(|l| l.id.clone()) {
                                out.send(state::wire(&ClientMsg::TermIn { id, d }));
                            }
                        }
                        Ev::Resize(cols, rows) => {
                            if let Some(id) = live.borrow().as_ref().and_then(|l| l.id.clone()) {
                                out.send(state::wire(&ClientMsg::TermResize { id, cols, rows }));
                            }
                        }
                        Ev::Exit(reason) => {
                            if let Some(l) = live.borrow_mut().as_mut() {
                                if let Some(id) = l.id.take() {
                                    term::unregister(&id);
                                }
                            }
                            phase.set(Phase::Closed(reason));
                        }
                        Ev::Visible(false) => {
                            let n = hidden.get() + 1;
                            hidden.set(n);
                            let tx = tx.clone();
                            spawn(async move {
                                platform::sleep(term::REST_AFTER_MS).await;
                                let _ = tx.unbounded_send(Ev::Rest(n));
                            });
                        }
                        Ev::Rest(n) => {
                            if n == hidden.get() && !dom::visible() && live.borrow().as_ref().is_some_and(|l| l.id.is_some()) {
                                detach(&live, &out);
                                if let Some(l) = live.borrow_mut().as_mut() {
                                    l.resting = true;
                                }
                            }
                        }
                        Ev::Visible(true) => {
                            hidden.set(hidden.get() + 1);
                            let resting = live.borrow().as_ref().is_some_and(|l| l.resting);
                            if resting && hub.peek().connected && *phase.peek() == Phase::Open {
                                attach(&live, &out, &tx, &host, &pane);
                            }
                        }
                    }
                }
            }
        });
    }

    {
        let (live, out) = (live.clone(), out.clone());
        use_drop(move || {
            detach(&live, &out);
            #[cfg(target_arch = "wasm32")]
            {
                term::js::set_screen(None);
                if let Some(l) = live.borrow_mut().take() {
                    use wasm_bindgen::JsCast;
                    if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                        let _ = d.remove_event_listener_with_callback("visibilitychange", l.visibility.as_ref().unchecked_ref());
                    }
                    l.x.dispose();
                }
            }
        });
    }

    let send = {
        let (live, out) = (live.clone(), out.clone());
        move |d: &str| {
            if let Some(id) = live.borrow().as_ref().and_then(|l| l.id.clone()) {
                out.send(state::wire(&ClientMsg::TermIn { id, d: d.to_string() }));
            }
            focus_screen(&live);
        }
    };
    let reconnect = {
        let (live, tx, out, host, pane) = (live.clone(), tx.clone(), out.clone(), host.clone(), pane.clone());
        move |_| {
            phase.set(Phase::Open);
            attach(&live, &out, &tx, &host, &pane);
            focus_screen(&live);
        }
    };
    let set_font = {
        let live = live.clone();
        move |by: f64| {
            let px = term::font_step(*font.peek(), by);
            font.set(px);
            platform::save(SETTINGS, &term::with_font_size(platform::load(SETTINGS).as_deref(), px));
            #[cfg(target_arch = "wasm32")]
            if let Some(l) = live.borrow().as_ref() {
                l.x.set_font_size(px);
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = &live;
        }
    };
    let keyboard = {
        let live = live.clone();
        move |_| {
            #[cfg(target_arch = "wasm32")]
            if let Some(l) = live.borrow().as_ref() {
                if l.x.focused() {
                    l.x.blur();
                } else {
                    l.x.focus();
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = &live;
        }
    };
    let on_menu = menu::open_with(|| {
        let mut items = Vec::new();
        if term::has_selection() {
            items.push(Item::new("Copy", Icon::Copy, Action::Term(MenuOp::Copy)));
        }
        items.push(Item::new("Paste", Icon::ClipboardPaste, Action::Term(MenuOp::Paste)));
        items.push(Item::new("Select all", Icon::TextSelect, Action::Term(MenuOp::SelectAll)));
        items
    });

    let connected = hub.read().connected;
    let (ctrl, alt) = mods();
    let size = font();
    let touch = dom::coarse();
    let focus_click = {
        let live = live.clone();
        move |_| focus_screen(&live)
    };
    rsx! {
        div { class: "term",
            div { class: "term-stage",
                div { id: SCREEN, class: "term-screen", onclick: focus_click, oncontextmenu: on_menu }
                match phase() {
                    Phase::Loading => rsx! {
                        div { class: "term-cover", role: "status", "aria-label": "Opening the terminal",
                            span { class: "skeleton-line" }
                            span { class: "skeleton-line skeleton-short" }
                        }
                    },
                    Phase::Closed(reason) => rsx! {
                        div { class: "term-cover term-closed fade-in",
                            p { class: "term-closed-text", if reason == "Detached" { "Terminal closed" } else { "{reason}" } }
                            button { class: "button button-md button-primary press", r#type: "button", onclick: reconnect,
                                Glyph { icon: Icon::RotateCw, size: 16 }
                                "Reconnect"
                            }
                        }
                    },
                    Phase::Failed(why) => rsx! {
                        div { class: "term-cover term-closed fade-in",
                            p { class: "term-closed-text", "{why}" }
                        }
                    },
                    Phase::Open => rsx! {},
                }
                if !connected && phase() == Phase::Open {
                    div { class: "chat-status term-status fade-in", role: "status", "Reconnecting…" }
                }
            }
            div { class: "term-keys", role: "toolbar", "aria-label": "Keys",
                for k in KEYS.iter().take(2) {
                    KeyButton { key: "{k.name}", label: k.name, onpress: { let send = send.clone(); move |_| send(k.seq) }, "{k.label}" }
                }
                KeyButton { label: "Ctrl (for the next key)", toggle: true, active: ctrl, onpress: move |_| mods.set((!ctrl, alt)), "ctrl" }
                KeyButton { label: "Alt (for the next key)", toggle: true, active: alt, onpress: move |_| mods.set((ctrl, !alt)), "alt" }
                for k in KEYS.iter().skip(2) {
                    KeyButton { key: "{k.name}", label: k.name, onpress: { let send = send.clone(); move |_| send(k.seq) },
                        match arrow(k.label) {
                            Some(icon) => rsx! { Glyph { icon, size: 16 } },
                            None => rsx! { "{k.label}" },
                        }
                    }
                }
                span { class: "term-keys-rule" }
                KeyButton { label: "Copy the selection", onpress: move |_| term::menu(MenuOp::Copy), Glyph { icon: Icon::Copy, size: 16 } }
                KeyButton { label: "Paste", onpress: move |_| term::menu(MenuOp::Paste), Glyph { icon: Icon::ClipboardPaste, size: 16 } }
                KeyButton { label: "Smaller text", disabled: size <= term::FONT_MIN, onpress: { let mut f = set_font.clone(); move |_| f(-1.0) }, Glyph { icon: Icon::Minus, size: 16 } }
                KeyButton { label: "Larger text", disabled: size >= term::FONT_MAX, onpress: { let mut f = set_font.clone(); move |_| f(1.0) }, Glyph { icon: Icon::Plus, size: 16 } }
                if touch {
                    KeyButton { label: "Keyboard", onpress: keyboard, Glyph { icon: Icon::Keyboard, size: 16 } }
                }
            }
        }
    }
}

fn arrow(label: &str) -> Option<Icon> {
    match label {
        "↑" => Some(Icon::ArrowUp),
        "↓" => Some(Icon::ArrowDown),
        "←" => Some(Icon::ArrowLeft),
        "→" => Some(Icon::ArrowRight),
        _ => None,
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
fn focus_screen(live: &Shared) {
    #[cfg(target_arch = "wasm32")]
    if let Some(l) = live.borrow().as_ref() {
        l.x.focus();
    }
}

/// A key on the key bar. Pressing it leaves focus (and a phone's keyboard) on the terminal.
#[component]
fn KeyButton(
    label: String,
    #[props(default)] toggle: bool,
    #[props(default)] active: bool,
    #[props(default)] disabled: bool,
    onpress: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        button {
            class: if active { "term-key press term-key-on" } else { "term-key press" },
            r#type: "button",
            "aria-label": "{label}",
            "aria-pressed": toggle.then(|| active.to_string()),
            title: "{label}",
            disabled,
            onmousedown: move |e| e.prevent_default(),
            onclick: move |_| onpress.call(()),
            {children}
        }
    }
}
