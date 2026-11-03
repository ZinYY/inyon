//! Spike S1: can Inyon's phone app be a Dioxus 0.7 app? A sidebar, a chat view, xterm.js at the
//! 200x50 resting grid, a composer on the soft keyboard and the keep-alive service started from
//! Rust. Everything it measures goes to logcat (tag InyonSpike) for the CI job to read.

use dioxus::prelude::*;

#[cfg(target_os = "android")]
mod android;

const CSS: &str = include_str!("style.css");
const TERM_JS: &str = include_str!("term.js");
// fetched and checked by ci/fetch-assets.sh (the same versions apps/web uses)
const XTERM_JS: &str = include_str!("../assets/vendor/xterm.js");
const XTERM_CSS: &str = include_str!("../assets/vendor/xterm.css");
const WEBGL_JS: &str = include_str!("../assets/vendor/addon-webgl.js");
const GEIST: &[u8] = include_bytes!("../assets/vendor/geist.woff2");
const GEIST_MONO: &[u8] = include_bytes!("../assets/vendor/geist-mono.woff2");

fn log(msg: &str) {
    #[cfg(target_os = "android")]
    android::log(msg);
    #[cfg(not(target_os = "android"))]
    eprintln!("InyonSpike: {msg}");
}

/// Milliseconds since Android started this process, so cold start can be read from inside.
fn since_process_start() -> Option<i64> {
    #[cfg(target_os = "android")]
    {
        android::ms_since_process_start()
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn main() {
    log(&format!("main started {} ms after the process", since_process_start().unwrap_or(-1)));
    let fonts = format!(
        "@font-face{{font-family:'Geist';font-weight:100 900;font-display:block;src:url(data:font/woff2;base64,{})format('woff2')}}\
         @font-face{{font-family:'Geist Mono';font-weight:100 900;font-display:block;src:url(data:font/woff2;base64,{})format('woff2')}}",
        base64(GEIST),
        base64(GEIST_MONO)
    );
    let head = format!(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content\">\
         <style>{fonts}</style><style>{XTERM_CSS}</style><style>{CSS}</style><script>{XTERM_JS}</script><script>{WEBGL_JS}</script>"
    );
    let cfg = dioxus::mobile::Config::new().with_custom_head(head).with_background_color((0xe9, 0xe8, 0xe4, 0xff));
    dioxus::LaunchBuilder::new().with_cfg(cfg).launch(App);
}

#[derive(Clone, PartialEq)]
struct Msg {
    user: bool,
    text: String,
}

struct Session {
    title: &'static str,
    sub: &'static str,
    state: &'static str,
}

const MACHINES: &[(&str, &[Session])] = &[
    (
        "Laptop",
        &[
            Session { title: "Fix the login redirect", sub: "Claude Code · working", state: "busy" },
            Session { title: "Release notes for 0.18", sub: "Codex · done", state: "done" },
            Session { title: "inyon/site", sub: "OpenCode · idle", state: "" },
        ],
    ),
    ("Pi at home", &[Session { title: "Backup script", sub: "Claude Code · waiting for you", state: "busy" }, Session { title: "shell", sub: "bash · idle", state: "" }]),
    ("Work desktop", &[Session { title: "Migrate the hub to axum", sub: "Codex · done", state: "done" }]),
];

#[component]
fn App() -> Element {
    let mut view = use_signal(|| "chat");
    let mut drawer = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut active = use_signal(|| 0usize);
    let mut messages = use_signal(|| {
        vec![
            Msg { user: true, text: "The login page sends people back to / after signing in. It should go to the page they came from.".into() },
            Msg { user: false, text: "I'll look at how the redirect target is kept.".into() },
            Msg { user: false, text: "tool:Read apps/web/src/lib/api.ts".into() },
            Msg { user: false, text: "The target is dropped when the session cookie is set. I'll keep it in the query string and read it back after the hub answers.".into() },
            Msg { user: false, text: "code:const next = new URL(location.href).searchParams.get('next') ?? '/'\nlocation.replace(next)".into() },
            Msg { user: true, text: "Sounds right. Keep the old behaviour when next points off-site.".into() },
        ]
    });

    // after the first render: open the terminal, then start the keep-alive service
    use_hook(|| {
        spawn(async move {
            log(&format!("first render {} ms after the process", since_process_start().unwrap_or(-1)));
            let mut ev = document::eval(TERM_JS);
            loop {
                let Ok(line) = ev.recv::<String>().await else {
                    log("terminal channel closed");
                    break;
                };
                if line.contains("\"term-ready\"") {
                    log(&format!("terminal ready {} ms after the process: {line}", since_process_start().unwrap_or(-1)));
                    keep_alive();
                } else if line.contains("\"term-input\"") {
                    log(&format!("terminal input: {line}"));
                } else {
                    log(&line);
                }
            }
        });
    });

    let session = MACHINES.iter().flat_map(|(_, s)| s.iter()).nth(active()).unwrap();
    let send = move |_| {
        let text = draft.peek().trim().to_string();
        if text.is_empty() {
            return;
        }
        log(&format!("sent: {text}"));
        messages.write().push(Msg { user: true, text });
        draft.set(String::new());
        document::eval(
            "document.querySelector('[aria-label=Message]').value = ''; \
             requestAnimationFrame(() => { const c = document.querySelector('.chat'); c.scrollTop = c.scrollHeight })",
        );
    };

    rsx! {
        div { class: if drawer() { "app drawer" } else { "app" },
            aside { class: "sidebar",
                div { class: "brand",
                    div { class: "logo", "i" }
                    "Inyon"
                }
                {
                    let mut idx = 0usize;
                    rsx! {
                        for (machine, sessions) in MACHINES.iter() {
                            div { class: "section", "{machine}" }
                            for s in sessions.iter() {
                                {
                                    let i = idx;
                                    idx += 1;
                                    rsx! {
                                        button {
                                            class: if active() == i { "row on" } else { "row" },
                                            onclick: move |_| {
                                                active.set(i);
                                                drawer.set(false);
                                                log(&format!("picked session {i}"));
                                            },
                                            div { class: "dot {s.state}" }
                                            div { class: "row-text",
                                                div { class: "row-title", "{s.title}" }
                                                div { class: "row-sub", "{s.sub}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "scrim", onclick: move |_| drawer.set(false) }
            div { class: "main",
                div { class: "top",
                    button { class: "icon menu", aria_label: "Open sidebar", onclick: move |_| drawer.set(true),
                        svg { width: "20", height: "20", view_box: "0 0 20 20", fill: "none", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round",
                            path { d: "M3 6h14M3 10h14M3 14h9" }
                        }
                    }
                    div { class: "title",
                        b { "{session.title}" }
                        span { "{session.sub}" }
                    }
                    div { class: "seg",
                        button { class: if view() == "chat" { "on" } else { "" }, aria_label: "Chat", onclick: move |_| view.set("chat"), "Chat" }
                        button { class: if view() == "term" { "on" } else { "" }, aria_label: "Terminal", onclick: move |_| view.set("term"), "Terminal" }
                    }
                }
                div { class: "body",
                    div { class: if view() == "chat" { "pane chat on" } else { "pane chat" },
                        for m in messages.read().iter() {
                            {render_msg(m)}
                        }
                    }
                    div { class: if view() == "term" { "pane term on" } else { "pane term" },
                        div { id: "term" }
                    }
                }
                div { class: "composer",
                    textarea {
                        aria_label: "Message",
                        placeholder: "Message the agent",
                        rows: "1",
                        // not bound to `draft`: writing the value back from Rust raced fast typing
                        // and dropped keys ("hello from the keyboard" arrived as "hm the keyrd")
                        oninput: move |e| {
                            let v = e.value();
                            log(&format!("composer: {v}"));
                            draft.set(v);
                        },
                    }
                    button { class: "send", aria_label: "Send", onclick: send, "Send" }
                }
            }
        }
    }
}

fn render_msg(m: &Msg) -> Element {
    if m.user {
        return rsx! { div { class: "msg user", "{m.text}" } };
    }
    if let Some(t) = m.text.strip_prefix("tool:") {
        return rsx! { div { class: "msg agent", div { class: "tool", code { "{t}" } } } };
    }
    if let Some(c) = m.text.strip_prefix("code:") {
        return rsx! { div { class: "msg agent", pre { "{c}" } } };
    }
    rsx! { div { class: "msg agent", p { "{m.text}" } } }
}

fn keep_alive() {
    #[cfg(target_os = "android")]
    match android::start_keep_alive("Connected to 3 machines") {
        Ok(()) => log("keep-alive service started from Rust"),
        Err(e) => log(&format!("keep-alive failed: {e}")),
    }
}
