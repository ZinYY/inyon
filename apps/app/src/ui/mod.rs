//! The screens. The root asks the hub whether this browser is signed in, then shows the sign-in
//! form or the app with its socket.

mod add;
mod chat;
mod composer;
mod connect;
mod icons;
mod kit;
mod login;
mod machines;
mod md;
mod menu;
mod notices;
mod overview;
mod pages;
mod phone;
mod prompt;
mod rows;
mod session;
mod session_menu;
mod settings;
mod setup;
mod shell;
// xterm.js and the page's events reach it only in the browser build
mod companion;
mod intro;
mod temp;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod terminal;
mod toast;

use dioxus::prelude::*;
use inyon_protocol::v2::{OVERVIEW, PAGES};

use crate::route;
#[cfg(any(target_arch = "wasm32", feature = "phone"))]
use crate::route::Route;
use crate::socket::{self, Outbox};
use crate::state::{self, Chats, Hub, Incoming};
use crate::{api, platform};

// in index.html's head, so the first paint is styled (the link Dioxus adds would come after the wasm)
const CSS: Asset = asset!("/assets/app.css", AssetOptions::css().with_static_head(true));
const GEIST: Asset = asset!("/assets/fonts/geist-latin-wght-normal.woff2");
const GEIST_MONO: Asset = asset!("/assets/fonts/geist-mono-latin-wght-normal.woff2");
/// the serif replies are set in (Claude's); the browser fetches it only when a reply uses it
const SERIF: Asset = asset!("/assets/fonts/source-serif-4-latin-wght-normal.woff2");

/// Where the app is with the hub's sign-in.
#[derive(Debug, Clone, PartialEq)]
pub enum Auth {
    Checking,
    /// `setup` when the hub has no password yet; `code` when making one needs the setup code
    SignedOut {
        setup: bool,
        code: bool,
    },
    SignedIn,
}

fn fonts() -> String {
    // the Latin subsets only, as apps/web/src/fonts.css, under the names styles.css uses
    let face = |family: &str, weights: &str, url: &Asset| {
        format!("@font-face{{font-family:'{family}';font-style:normal;font-display:swap;font-weight:{weights};src:url({url}) format('woff2-variations');unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD}}")
    };
    face("Geist Variable", "100 900", &GEIST) + &face("Geist Mono Variable", "100 900", &GEIST_MONO) + &face("Source Serif 4 Variable", "200 900", &SERIF)
}

async fn check(mut auth: Signal<Auth>) {
    auth.set(match api::session().await {
        Ok(s) if s.authed => Auth::SignedIn,
        Ok(s) => Auth::SignedOut { setup: s.setup, code: s.setup_code },
        // the hub didn't answer: the form, whose submit says why it can't go on
        Err(_) => Auth::SignedOut { setup: false, code: false },
    });
}

#[component]
pub fn App() -> Element {
    let auth = use_signal(|| Auth::Checking);
    use_hook(move || spawn(check(auth)));
    // the intro once per tab session, over the page as it loads; the mark lands on the header's
    let intro = use_hook(intro::due);
    rsx! {
        document::Stylesheet { href: CSS }
        document::Link { rel: "preload", href: GEIST, r#as: "font", r#type: "font/woff2", crossorigin: "anonymous" }
        style { {fonts()} }
        match auth() {
            Auth::Checking => rsx! {},
            Auth::SignedOut { setup, code } => rsx! {
                login::Login { setup, needs_code: code, on_done: move |_| { spawn(check(auth)); } }
            },
            Auth::SignedIn => rsx! { Connected { auth } },
        }
        if intro {
            intro::Intro {}
        }
    }
}

/// The signed-in app: the hub's state, fed by the socket for as long as this is on screen, and the
/// route.
#[component]
fn Connected(auth: Signal<Auth>) -> Element {
    let mut hub = use_context_provider(|| Signal::new(Hub::default()));
    let mut chats = use_context_provider(|| Signal::new(Chats::default()));
    #[cfg_attr(not(any(target_arch = "wasm32", feature = "phone")), allow(unused_mut, unused_variables))]
    let mut here = use_context_provider(|| Signal::new(route::current()));
    use_context_provider(|| Signal::new(None::<menu::Menu>));
    // "Sign in" in Can't connect? goes back to the form
    use_context_provider(|| auth);
    #[cfg(any(target_arch = "wasm32", feature = "phone"))]
    use_hook(move || {
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<Route>();
        route::listen(tx);
        spawn(async move {
            use futures_util::StreamExt;
            while let Some(r) = rx.next().await {
                here.set(r);
            }
        });
    });
    let outbox = use_hook(move || {
        // a new sign-in: the screens start empty, so the streams start over
        state::reset_feed();
        pages::PAGES.write().pages = None;
        let (outbox, inbox) = socket::channel();
        let out = outbox.clone();
        spawn(socket::run(inbox, move |ev| match ev {
            socket::Event::Up => {
                connect::record_up();
                // a hub that restarted comes back on a new connection, maybe running newer code
                notices::check_build();
                if !hub.peek().connected {
                    hub.write().set_connected(true);
                }
                // follow the overview, the pages and every open chat from where they were left
                for sub in state::with_feed(|f| f.subs()) {
                    out.send(state::wire2(&sub));
                }
                for sub in chats.peek().subs() {
                    out.send(state::wire2(&sub));
                }
            }
            socket::Event::Down(why) => {
                connect::record_down(&why);
                if hub.peek().connected {
                    hub.write().set_connected(false);
                }
                // the hub refuses the socket once the session has expired
                spawn(async move {
                    if let Ok(s) = api::session().await {
                        if !s.authed {
                            auth.set(Auth::SignedOut { setup: s.setup, code: s.setup_code });
                        }
                    }
                });
            }
            socket::Event::Message { text, binary } => match state::read(text.as_deref(), binary.as_deref()) {
                Incoming::Frame(f) if f.stream() == Some(OVERVIEW) => {
                    // read without subscribing, so an unchanged overview doesn't redraw the screen
                    let (hosts, resync) = state::with_feed(|feed| feed.overview(&f, &hub.peek(), now()));
                    if let Some(hosts) = hosts {
                        hub.write().set_hosts(hosts);
                    }
                    if let Some(m) = resync {
                        out.send(state::wire2(&m));
                    }
                }
                Incoming::Frame(f) if f.stream() == Some(PAGES) => {
                    let (list, resync) = state::with_feed(|feed| feed.pages(&f));
                    if let Some(list) = list {
                        pages::PAGES.write().pages = Some(list);
                    }
                    if let Some(m) = resync {
                        out.send(state::wire2(&m));
                    }
                }
                Incoming::Frame(f) => {
                    // a chat nobody has open is dropped without a redraw
                    if f.stream().is_some_and(|k| chats.peek().get(k).is_some()) {
                        if let Some(m) = chats.write().apply(&f, now()) {
                            out.send(state::wire2(&m));
                        }
                    }
                }
                // a terminal's output goes straight to its drawing; notices become toasts
                Incoming::Hub { t, text } if t.starts_with("term.") => crate::term::dispatch(&text),
                Incoming::Hub { t, text } => {
                    if let Some(m) = state::notice_msg(&t, &text) {
                        notices::handle(&m, &here.peek(), &hub.peek().hosts);
                    }
                }
                Incoming::Other => {}
            },
        }));
        outbox
    });
    use_context_provider(|| outbox);
    rsx! {
        shell::Shell {}
        notices::PushWatch {}
    }
}

/// Unix ms, for the chat model.
pub fn now() -> u64 {
    platform::clock().now as u64
}

/// The outbox for messages to the hub.
pub fn outbox() -> Outbox {
    use_context::<Outbox>()
}
