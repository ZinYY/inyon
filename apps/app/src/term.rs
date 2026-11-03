//! A session's terminal, between the hub and xterm.js (TerminalView.tsx): the hub attaches tmux to
//! the pane for each `term.open` and streams it as `term.data`, which goes straight to the drawing
//! in term/term.ts without passing through Dioxus; what you type goes back as `term.in`. The key
//! bar's keys and its sticky Ctrl and Alt, the colour replies that must not reach tmux, the text
//! size, and letting go of the window while nobody looks (so it rests at 200×50) are decided here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde::Deserialize;

/// How long the page stays hidden before its terminal lets go of the window. tmux then has no
/// client showing it, so an agent's window rests at 200×50 instead of the phone's size; it comes
/// back as it was when the page is shown again.
pub const REST_AFTER_MS: u32 = 15_000;

/// The text sizes the key bar steps through (the web app's).
pub const FONT_MIN: f64 = 9.0;
pub const FONT_MAX: f64 = 22.0;

/// A key on the key bar: what it shows, what it's called, and what it types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub label: &'static str,
    pub name: &'static str,
    pub seq: &'static str,
}

/// The keys a phone keyboard lacks, as the web app's key bar has them; Ctrl and Alt come between
/// Tab and the arrows.
pub const KEYS: [Key; 13] = [
    Key { label: "esc", name: "Escape", seq: "\x1b" },
    Key { label: "tab", name: "Tab", seq: "\t" },
    Key { label: "↑", name: "Up", seq: "\x1b[A" },
    Key { label: "↓", name: "Down", seq: "\x1b[B" },
    Key { label: "←", name: "Left", seq: "\x1b[D" },
    Key { label: "→", name: "Right", seq: "\x1b[C" },
    Key { label: "^C", name: "Interrupt (Ctrl+C)", seq: "\x03" },
    Key { label: "^R", name: "Search history (Ctrl+R)", seq: "\x12" },
    Key { label: "|", name: "|", seq: "|" },
    Key { label: "/", name: "/", seq: "/" },
    Key { label: "-", name: "-", seq: "-" },
    Key { label: "~", name: "~", seq: "~" },
    Key { label: "`", name: "`", seq: "`" },
];

/// One key typed while the key bar's Ctrl or Alt is down: Ctrl makes a control character of a
/// letter or `@[\]^_`, Alt puts Escape in front. Anything longer than one character (a paste, a
/// key's sequence) goes as it is.
pub fn modded(d: &str, ctrl: bool, alt: bool) -> String {
    let mut chars = d.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return d.to_string() };
    let mut out = c.to_string();
    if ctrl {
        let up = c.to_ascii_uppercase() as u32;
        if (64..=95).contains(&up) {
            out = char::from_u32(up - 64).map(String::from).unwrap_or(out);
        }
    }
    if alt {
        out.insert(0, '\x1b');
    }
    out
}

/// xterm's answer to a colour question (OSC 4, 10, 11 or 12): sent on, it would reach the agent
/// through tmux as keystrokes and show in its input box as junk like "b3b/5454…".
pub fn is_color_reply(d: &str) -> bool {
    let Some(rest) = d.strip_prefix("\x1b]") else { return false };
    let Some((code, body)) = rest.split_once(';') else { return false };
    let body = body.strip_suffix('\x07').or_else(|| body.strip_suffix("\x1b\\"));
    matches!(code, "4" | "10" | "11" | "12") && body.is_some_and(|b| !b.contains(['\x07', '\x1b']))
}

/// The text size a step up or down the key bar's buttons gives.
pub fn font_step(size: f64, by: f64) -> f64 {
    (size + by).clamp(FONT_MIN, FONT_MAX)
}

/// The saved settings (`inyon.settings.v2`) with a new terminal text size, keeping every other key
/// as it was, the ones this app doesn't know included: the web app reads the same copy.
pub fn with_font_size(saved: Option<&str>, px: f64) -> String {
    let mut v = saved.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v["termFontSize"] = serde_json::json!(px);
    v.to_string()
}

/// What the hub says about a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    Data(String),
    /// tmux let go: the pane closed or the hub dropped it (`Detached` when nothing more is known)
    Exit(String),
}

#[derive(Deserialize)]
struct Msg {
    t: String,
    id: String,
    d: Option<String>,
    reason: Option<String>,
}

/// A `term.data` or `term.exit` message, with the terminal it's for.
pub fn parse(text: &str) -> Option<(String, HubEvent)> {
    let m: Msg = serde_json::from_str(text).ok()?;
    let ev = match m.t.as_str() {
        "term.data" => HubEvent::Data(m.d?),
        "term.exit" => HubEvent::Exit(m.reason.filter(|r| !r.is_empty()).unwrap_or_else(|| "Detached".into())),
        _ => return None,
    };
    Some((m.id, ev))
}

type Sink = Rc<dyn Fn(HubEvent)>;

thread_local! {
    /// The open terminals by id, each with what takes its messages.
    static SINKS: RefCell<HashMap<String, Sink>> = RefCell::new(HashMap::new());
}

pub fn register(id: &str, sink: impl Fn(HubEvent) + 'static) {
    SINKS.with(|s| s.borrow_mut().insert(id.to_string(), Rc::new(sink)));
}

pub fn unregister(id: &str) {
    SINKS.with(|s| s.borrow_mut().remove(id));
}

/// Hands a `term.*` message from the hub to its terminal; one for a terminal that has closed is
/// dropped.
pub fn dispatch(text: &str) {
    let Some((id, ev)) = parse(text) else { return };
    // out of the map before it runs: an exit unregisters its terminal
    let sink = SINKS.with(|s| s.borrow().get(&id).cloned());
    if let Some(sink) = sink {
        sink(ev);
    }
}

/// What the terminal's right-click menu does, to the terminal on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOp {
    Copy,
    Paste,
    SelectAll,
}

#[cfg(target_arch = "wasm32")]
pub mod js {
    //! xterm.js through term/term.ts, loaded once, the first time a terminal opens.

    use std::cell::RefCell;

    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    #[wasm_bindgen]
    extern "C" {
        /// `window.inyonTerm`
        pub type Api;
        #[wasm_bindgen(method)]
        pub fn open(this: &Api, el: &web_sys::HtmlElement, font_size: f64, on_data: &js_sys::Function, on_resize: &js_sys::Function) -> Xterm;

        /// One terminal's drawing (term.ts `Handle`).
        #[derive(Clone)]
        pub type Xterm;
        #[wasm_bindgen(method)]
        pub fn write(this: &Xterm, d: &str);
        #[wasm_bindgen(method)]
        pub fn reset(this: &Xterm);
        #[wasm_bindgen(method)]
        pub fn focus(this: &Xterm);
        #[wasm_bindgen(method)]
        pub fn blur(this: &Xterm);
        #[wasm_bindgen(method)]
        pub fn focused(this: &Xterm) -> bool;
        #[wasm_bindgen(method)]
        pub fn cols(this: &Xterm) -> u16;
        #[wasm_bindgen(method)]
        pub fn rows(this: &Xterm) -> u16;
        #[wasm_bindgen(method, js_name = setFontSize)]
        pub fn set_font_size(this: &Xterm, px: f64);
        #[wasm_bindgen(method, js_name = hasSelection)]
        pub fn has_selection(this: &Xterm) -> bool;
        #[wasm_bindgen(method)]
        pub fn selection(this: &Xterm) -> String;
        #[wasm_bindgen(method, js_name = clearSelection)]
        pub fn clear_selection(this: &Xterm);
        #[wasm_bindgen(method, js_name = selectAll)]
        pub fn select_all(this: &Xterm);
        #[wasm_bindgen(method)]
        pub fn paste(this: &Xterm, text: &str);
        #[wasm_bindgen(method)]
        pub fn dispose(this: &Xterm);
    }

    thread_local! {
        /// The terminal on screen, for its right-click menu.
        static SCREEN: RefCell<Option<Xterm>> = const { RefCell::new(None) };
    }

    pub fn set_screen(x: Option<Xterm>) {
        SCREEN.with(|s| *s.borrow_mut() = x);
    }

    pub fn screen() -> Option<Xterm> {
        SCREEN.with(|s| s.borrow().clone())
    }

    /// How terminals draw on this device, as Settings › About says it: WebGL, or the page (also
    /// before any terminal has opened, as in the web app).
    pub fn drawing() -> &'static str {
        let r = api().and_then(|a| {
            let a = JsValue::from(a);
            js_sys::Reflect::get(&a, &"renderer".into()).ok()?.dyn_into::<js_sys::Function>().ok()?.call0(&a).ok()?.as_string()
        });
        if r.as_deref() == Some("webgl") {
            "WebGL"
        } else {
            "Page"
        }
    }

    fn api() -> Option<Api> {
        let win: JsValue = web_sys::window()?.into();
        let v = js_sys::Reflect::get(&win, &"inyonTerm".into()).ok()?;
        (!v.is_undefined()).then(|| v.unchecked_into())
    }

    /// xterm.js, loading term.ts first if no terminal has opened yet. The page names the file
    /// (build.sh puts it in a meta tag).
    pub async fn load() -> Result<Api, String> {
        if let Some(a) = api() {
            return Ok(a);
        }
        let doc = web_sys::window().and_then(|w| w.document()).ok_or("no page")?;
        let src =
            doc.query_selector("meta[name=inyon-term]").ok().flatten().and_then(|m| m.get_attribute("content")).ok_or("This copy of the app was built without its terminal.")?;
        // one script for every terminal that opens while it loads
        let script: web_sys::HtmlScriptElement = match doc.query_selector("script[data-inyon-term]").ok().flatten() {
            Some(s) => s.unchecked_into(),
            None => {
                let s: web_sys::HtmlScriptElement = doc.create_element("script").map_err(|_| "no script")?.unchecked_into();
                s.set_type("module");
                s.set_src(&src);
                let _ = s.set_attribute("data-inyon-term", "");
                doc.head().ok_or("no head")?.append_child(&s).map_err(|_| "no head")?;
                s
            }
        };
        let done = js_sys::Promise::new(&mut |resolve, reject| {
            let _ = script.add_event_listener_with_callback("load", &resolve);
            let _ = script.add_event_listener_with_callback("error", &reject);
        });
        let loaded = wasm_bindgen_futures::JsFuture::from(done).await;
        if let Some(a) = api() {
            return Ok(a);
        }
        // a failed script is taken out, so the next try loads it again
        script.remove();
        Err(if loaded.is_err() { "The terminal couldn't load. Check the connection and try again." } else { "The terminal didn't start." }.into())
    }
}

/// Does `op` to the terminal on screen.
pub fn menu(op: MenuOp) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(x) = js::screen() else { return };
        match op {
            MenuOp::Copy => {
                let text = x.selection();
                if !text.is_empty() {
                    x.clear_selection();
                    dioxus::prelude::spawn(async move {
                        crate::dom::copy(&text).await;
                    });
                }
                x.focus();
            }
            MenuOp::Paste => {
                dioxus::prelude::spawn(async move {
                    if let Some(t) = crate::dom::paste().await.filter(|t| !t.is_empty()) {
                        x.paste(&t);
                    }
                    x.focus();
                });
            }
            MenuOp::SelectAll => x.select_all(),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = op;
}

/// Whether the terminal on screen has text selected (its menu offers Copy then).
pub fn has_selection() -> bool {
    #[cfg(target_arch = "wasm32")]
    return js::screen().is_some_and(|x| x.has_selection());
    #[cfg(not(target_arch = "wasm32"))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_ctrl_and_alt_change_one_key() {
        assert_eq!(modded("c", true, false), "\x03");
        assert_eq!(modded("C", true, false), "\x03");
        assert_eq!(modded("[", true, false), "\x1b");
        assert_eq!(modded("x", false, true), "\x1bx");
        assert_eq!(modded("d", true, true), "\x1b\x04");
        // a digit has no control character: it goes as typed
        assert_eq!(modded("1", true, false), "1");
        // a paste or a key's whole sequence is left alone
        assert_eq!(modded("ls -la", true, true), "ls -la");
        assert_eq!(modded("\x1b[A", true, false), "\x1b[A");
        assert_eq!(modded("é", false, true), "\x1bé");
    }

    #[test]
    fn colour_replies_stay_out_of_tmux() {
        assert!(is_color_reply("\x1b]11;rgb:1b1b/1a1a/1818\x07"));
        assert!(is_color_reply("\x1b]10;rgb:f1f1/f0f0/ecec\x1b\\"));
        assert!(is_color_reply("\x1b]4;1;rgb:b5b5/4545/3b3b\x07"));
        // other OSC answers, keys and text go through
        assert!(!is_color_reply("\x1b]52;c;aGk=\x07"));
        assert!(!is_color_reply("\x1b[A"));
        assert!(!is_color_reply("hello"));
        assert!(!is_color_reply("\x1b]11;rgb:1b1b/1a1a/1818"));
    }

    #[test]
    fn the_text_size_stays_in_its_range_and_keeps_other_settings() {
        assert_eq!(font_step(13.0, 1.0), 14.0);
        assert_eq!(font_step(22.0, 1.0), 22.0);
        assert_eq!(font_step(9.0, -1.0), 9.0);
        let saved = with_font_size(Some(r#"{"theme":"dark","futureSetting":[1,2]}"#), 15.0);
        let v: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(v["termFontSize"], 15.0);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["futureSetting"], serde_json::json!([1, 2]));
        assert_eq!(with_font_size(None, 12.0), r#"{"termFontSize":12.0}"#);
        assert_eq!(with_font_size(Some("not json"), 12.0), r#"{"termFontSize":12.0}"#);
    }

    #[test]
    fn hub_messages_reach_their_terminal() {
        assert_eq!(parse(r#"{"t":"term.data","id":"a","d":"$ ls\r\n"}"#), Some(("a".into(), HubEvent::Data("$ ls\r\n".into()))));
        assert_eq!(parse(r#"{"t":"term.exit","id":"a","reason":"Pane closed"}"#), Some(("a".into(), HubEvent::Exit("Pane closed".into()))));
        assert_eq!(parse(r#"{"t":"term.exit","id":"a"}"#), Some(("a".into(), HubEvent::Exit("Detached".into()))));
        assert_eq!(parse(r#"{"t":"pong"}"#), None);

        let got = Rc::new(RefCell::new(Vec::new()));
        let g = got.clone();
        register("t1", move |ev| g.borrow_mut().push(ev));
        dispatch(r#"{"t":"term.data","id":"t1","d":"hi"}"#);
        dispatch(r#"{"t":"term.data","id":"other","d":"not mine"}"#);
        unregister("t1");
        dispatch(r#"{"t":"term.data","id":"t1","d":"after it closed"}"#);
        assert_eq!(*got.borrow(), vec![HubEvent::Data("hi".into())]);
    }

    #[test]
    fn the_key_bar_types_what_the_web_apps_does() {
        let seq = |label: &str| KEYS.iter().find(|k| k.label == label).map(|k| k.seq);
        assert_eq!(seq("esc"), Some("\x1b"));
        assert_eq!(seq("↑"), Some("\x1b[A"));
        assert_eq!(seq("→"), Some("\x1b[C"));
        assert_eq!(seq("^C"), Some("\x03"));
        assert_eq!(seq("^R"), Some("\x12"));
    }
}
