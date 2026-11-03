//! "Can't connect?" (components/ConnectSheet.tsx): opened from a machine that isn't connected, or
//! from the hub's own "Reconnecting", it runs the checks in crate::connect one after another, says
//! what each found and what to do, and copies a report with the secrets taken out. The connection
//! log it reads is kept here too, fed by the socket.

use dioxus::prelude::*;

use super::icons::{Glyph, Icon};
use super::kit::Sheet;
use super::toast::{self, ToastTone};
use crate::connect::{self as c, ConnLog, Run, Step, StepAction, StepState, Target};
use crate::socket::Dropped;
use crate::state::Hub;
use crate::{dom, platform};

/// The log of the hub's connection, as the web app keeps it in this site's storage.
static LOG: GlobalSignal<ConnLog> = Signal::global(|| ConnLog::load(platform::load(c::LOG_KEY).as_deref()));
/// What the sheet checks: `self` for the hub, or a machine's id. Kept after closing so the sheet
/// can slide away with its content.
static ABOUT: GlobalSignal<String> = Signal::global(|| c::SELF.to_string());
static OPEN: GlobalSignal<bool> = Signal::global(|| false);

/// Opens "Can't connect?" for a machine, or for the hub with `None`.
pub fn open_connect_help(id: Option<String>) {
    *ABOUT.write() = id.unwrap_or_else(|| c::SELF.to_string());
    *OPEN.write() = true;
}

/// "Last drop 5m ago, the network went away (down 12s)", once a connection has dropped
/// (ConnectSheet.tsx's LastDrop). `self` is this page's own connection.
pub fn drop_line(id: &str) -> Option<String> {
    let log = LOG.read();
    let d = log.last_drop(id)?;
    let clock = platform::clock();
    let back = d.back.map(|_| format!(" (down {})", c::down_for(d, clock.now))).unwrap_or_default();
    Some(format!("Last drop {}, {}{back}", crate::format::time_ago(d.at, clock), c::why(d)))
}

fn hub_name() -> String {
    dom::host()
}

/// What was going on when the connection dropped.
fn context() -> Vec<String> {
    let mut v = vec![];
    if !dom::online() {
        v.push("offline".to_string());
    }
    if !dom::visible() {
        v.push("background".to_string());
    }
    v
}

pub fn record_up() {
    let mut log = LOG.write();
    if log.up(c::SELF, "hub", &hub_name(), platform::clock().now) {
        platform::save(c::LOG_KEY, &log.save());
    }
}

pub fn record_down(d: &Dropped) {
    let cause = if d.timeout { "timeout" } else { "closed" };
    let detail = (!d.reason.is_empty()).then_some(d.reason.as_str());
    let mut log = LOG.write();
    log.down(c::SELF, "hub", &hub_name(), cause, d.code, detail, context(), platform::clock().now);
    platform::save(c::LOG_KEY, &log.save());
}

fn target_for(id: &str, hub: &Hub) -> Option<Target> {
    let origin = dom::origin();
    if id == c::SELF {
        return Some(Target::Hub { origin });
    }
    let h = hub.hosts.iter().find(|h| h.config.id == id)?;
    Some(Target::Machine { id: id.into(), name: h.config.name.clone(), origin })
}

fn report_text(t: Option<&Target>, steps: &[Step]) -> String {
    let log = LOG.read();
    let ua = dom::user_agent();
    c::report(&c::ReportInput {
        now: platform::clock().now,
        version: env!("CARGO_PKG_VERSION"),
        user_agent: (!ua.is_empty()).then_some(ua.as_str()),
        online: dom::online(),
        hidden: !dom::visible(),
        target: t,
        steps,
        drops: log.recent(10),
    })
}

/// Mounted once, in the shell.
#[component]
pub fn ConnectHelpHost() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut auth = use_context::<Signal<super::Auth>>();
    let mut run = use_signal(|| None::<Run>);
    let mut target = use_signal(|| None::<Target>);
    let mut text = use_signal(String::new);
    let mut details = use_signal(|| false);
    let mut gen = use_signal(|| 0u32);
    let open = OPEN();
    let id = ABOUT();

    let mut start = move || {
        let g = *gen.peek() + 1;
        gen.set(g);
        text.set(String::new());
        let t = target_for(&ABOUT.peek(), &hub.peek());
        target.set(t.clone());
        let Some(t) = t else {
            run.set(None);
            return;
        };
        run.set(Some(Run::new(&t)));
        spawn(async move {
            let mut session = c::Session::Failed(String::new());
            loop {
                if *gen.peek() != g {
                    return;
                }
                let next = run.write().as_mut().and_then(|r| r.next());
                let Some(step) = next else { break };
                // a moment on each step, so the sheet reads as a sequence
                platform::sleep(150).await;
                let found = match step {
                    "network" => c::network(dom::online()),
                    "address" => c::address(&t),
                    "answers" => {
                        session = probe::session().await;
                        c::answers(&t, &session)
                    }
                    "host" => c::right_address(&t, &session),
                    "signed-in" => c::signed_in(&session),
                    "socket" => c::socket(&probe::socket().await),
                    "machine" => {
                        let (name, id) = match &t {
                            Target::Machine { name, id, .. } => (name.clone(), id.clone()),
                            Target::Hub { .. } => (String::new(), String::new()),
                        };
                        let h = hub.peek().hosts.iter().find(|h| h.config.id == id).cloned();
                        c::machine(&name, h.as_ref())
                    }
                    _ => continue,
                };
                if *gen.peek() != g {
                    return;
                }
                if let Some(r) = run.write().as_mut() {
                    r.found(step, found);
                }
            }
            let steps = run.peek().as_ref().map(|r| r.steps.clone()).unwrap_or_default();
            text.set(report_text(Some(&t), &steps));
        });
    };

    // every opening runs the checks afresh; closing stops a run that's still going
    use_effect(move || {
        if OPEN() {
            let _ = ABOUT();
            details.set(false);
            start();
        } else {
            let g = *gen.peek() + 1;
            gen.set(g);
        }
    });

    let close = move |_| *OPEN.write() = false;
    let r = run();
    let t = target();
    let name =
        t.as_ref().map(|t| t.name()).or_else(|| hub.read().hosts.iter().find(|h| h.config.id == id).map(|h| h.config.name.clone())).unwrap_or_else(|| "this connection".into());
    let done = r.as_ref().is_some_and(|r| r.done());
    let running = r.is_some() && !done;
    let all_good = r.as_ref().is_some_and(|r| r.all_good());
    let steps = r.map(|r| r.steps).unwrap_or_default();
    let drop_line = {
        let log = LOG.read();
        log.last_drop(&id).map(|d| {
            let clock = platform::clock();
            let back = d.back.map(|_| format!(" (down {})", c::down_for(d, clock.now))).unwrap_or_default();
            format!(" · last drop {}, {}{back}", crate::format::time_ago(d.at, clock), c::why(d))
        })
    };
    let copy_steps = steps.clone();
    let act = move |a: StepAction| match a {
        StepAction::SignIn => {
            *OPEN.write() = false;
            auth.set(super::Auth::SignedOut { setup: false, code: false });
        }
    };
    rsx! {
        Sheet {
            open,
            on_close: close,
            title: "Can't connect?",
            footer: rsx! {
                div { class: "sheet-actions",
                    button {
                        class: "button button-sm button-quiet press",
                        r#type: "button",
                        disabled: t.is_none(),
                        onclick: move |_| {
                            let body = if text().is_empty() { report_text(target().as_ref(), &copy_steps) } else { text() };
                            spawn(async move {
                                if dom::copy(&body).await {
                                    toast::toast("Report copied", Some("Paste it to whoever is helping. Tokens, codes and keys are left out.".into()), ToastTone::Success);
                                } else {
                                    toast::toast("Could not copy", Some("Open Details, select the report and copy it by hand.".into()), ToastTone::Error);
                                }
                            });
                        },
                        Glyph { icon: Icon::Copy, size: 14 }
                        "Copy report"
                    }
                    button { class: "button button-sm button-primary press", r#type: "button", disabled: running || t.is_none(), onclick: move |_| start(),
                        Glyph { icon: Icon::RotateCcw, size: 14 }
                        "Check again"
                    }
                }
            },
            p { class: "check-about",
                span { class: "fg-2", "{name}" }
                if let Some(t) = &t {
                    " · {t.kind()}"
                }
                if let Some(d) = drop_line {
                    "{d}"
                }
            }
            if t.is_none() {
                p { class: "sheet-text", "There's nothing this app can check for it from here." }
            }
            ol { class: "checks", "aria-label": "Checks",
                for s in steps.iter().cloned() {
                    StepRow { key: "{s.id}", step: s, on_action: act }
                }
            }
            if all_good {
                p { class: "check-good fade-in", "Everything here works, so it should reconnect by itself. If it doesn't, copy the report for whoever is helping." }
            }
            if done {
                div { class: "check-details",
                    button { class: "fold-button", r#type: "button", "aria-expanded": "{details()}", onclick: move |_| details.toggle(),
                        Glyph { icon: Icon::ChevronDown, size: 16, class: if details() { "chevron-down" } else { "chevron-down chevron-closed" } }
                        "Details"
                    }
                    div { class: "expand", "data-open": "{details()}",
                        div {
                            pre { class: "check-report mono", if text().is_empty() { "…" } else { "{text}" } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StepRow(step: Step, on_action: EventHandler<StepAction>) -> Element {
    let s = step;
    let sentence = if s.state == StepState::Running { Some(s.doing.clone()) } else { s.say.clone() };
    let help = s.state == StepState::Fail;
    let quiet = matches!(s.state, StepState::Pending | StepState::Skip);
    let state = match s.state {
        StepState::Pending => "pending",
        StepState::Running => "running",
        StepState::Pass => "pass",
        StepState::Fail => "fail",
        StepState::Skip => "skip",
    };
    rsx! {
        li { class: "check", "data-state": state,
            span { class: "check-icon check-icon-{state}",
                match s.state {
                    StepState::Running => rsx! { Glyph { icon: Icon::Spinner, size: 16, class: "spin" } },
                    StepState::Pass => rsx! { span { "aria-label": "Passed", Glyph { icon: Icon::Check, size: 14 } } },
                    StepState::Fail => rsx! { span { "aria-label": "Failed", Glyph { icon: Icon::X, size: 14 } } },
                    StepState::Skip => rsx! { span { "aria-label": "Skipped", Glyph { icon: Icon::Minus, size: 14 } } },
                    StepState::Pending => rsx! { span { class: "check-ring", "aria-label": "Waiting" } },
                }
            }
            div { class: "check-words",
                div { class: if quiet { "check-title fg-3" } else { "check-title" }, "{s.title}" }
                if let Some(t) = sentence {
                    p {
                        key: "{state}",
                        class: if s.state == StepState::Skip { "check-say fade-in fg-3" } else { "check-say fade-in" },
                        role: if s.state == StepState::Running { "status" } else { "" },
                        "{t}"
                    }
                }
                if help {
                    if let Some(f) = &s.fix {
                        p { class: "check-fix fade-in", "{f}" }
                    }
                    if let Some(a) = s.action {
                        button { class: "button button-xs button-quiet press check-action", r#type: "button", onclick: move |_| on_action.call(a), "{a.label()}" }
                    }
                }
            }
        }
    }
}

/// The two things only the browser can try: asking the hub who's signed in, and opening the live
/// connection.
#[cfg(target_arch = "wasm32")]
mod probe {
    use crate::connect::{Opened, Session, TIMEOUT_MS};
    use futures_util::future::{select, Either};
    use wasm_bindgen::{closure::Closure, JsCast};
    use wasm_bindgen_futures::JsFuture;

    async fn get(path: &str) -> Result<(u16, String), String> {
        let init = web_sys::RequestInit::new();
        init.set_method("GET");
        init.set_credentials(web_sys::RequestCredentials::SameOrigin);
        let req = web_sys::Request::new_with_str_and_init(path, &init).map_err(|e| format!("{e:?}"))?;
        let win = web_sys::window().ok_or("no window")?;
        let res: web_sys::Response = JsFuture::from(win.fetch_with_request(&req))
            .await
            .map_err(|e| e.dyn_ref::<js_sys::Error>().map(|e| format!("{}: {}", e.name(), e.message())).unwrap_or_else(|| "TypeError: Failed to fetch".into()))?
            .dyn_into()
            .map_err(|_| "not a response".to_string())?;
        let text = match res.text() {
            Ok(p) => JsFuture::from(p).await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
            Err(_) => String::new(),
        };
        Ok((res.status(), text))
    }

    pub async fn session() -> Session {
        let ask = Box::pin(get("/api/session"));
        let wait = Box::pin(gloo_timers::future::TimeoutFuture::new(TIMEOUT_MS));
        match select(ask, wait).await {
            Either::Left((Ok((status, body)), _)) => {
                let authed = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v.get("authed").and_then(|a| a.as_bool()));
                Session::Answered { status, authed }
            }
            Either::Left((Err(e), _)) => Session::Failed(e),
            Either::Right(_) => Session::TimedOut,
        }
    }

    pub async fn socket() -> Opened {
        let loc = web_sys::window().map(|w| w.location());
        let https = loc.as_ref().and_then(|l| l.protocol().ok()).is_some_and(|p| p == "https:");
        let host = loc.and_then(|l| l.host().ok()).unwrap_or_default();
        let url = format!("{}://{host}/ws2?catalog={}", if https { "wss" } else { "ws" }, inyon_protocol::compat::CATALOG);
        let Ok(ws) = web_sys::WebSocket::new(&url) else { return Opened::Closed { code: 1006, reason: String::new() } };
        let (tx, rx) = futures_channel::oneshot::channel::<Opened>();
        let tx = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));
        let t = tx.clone();
        let opened = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            if let Some(tx) = t.borrow_mut().take() {
                let _ = tx.send(Opened::Open);
            }
        });
        let t = tx.clone();
        let closed = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
            let c = e.dyn_ref::<web_sys::CloseEvent>();
            if let Some(tx) = t.borrow_mut().take() {
                let _ = tx.send(Opened::Closed { code: c.map_or(1006, |c| c.code()), reason: c.map(|c| c.reason()).unwrap_or_default() });
            }
        });
        ws.set_onopen(Some(opened.as_ref().unchecked_ref()));
        ws.set_onclose(Some(closed.as_ref().unchecked_ref()));
        let wait = Box::pin(gloo_timers::future::TimeoutFuture::new(super::c::TIMEOUT_MS));
        let out = match select(rx, wait).await {
            Either::Left((Ok(o), _)) => o,
            Either::Left((Err(_), _)) | Either::Right(_) => Opened::TimedOut,
        };
        ws.set_onopen(None);
        ws.set_onclose(None);
        let _ = ws.close();
        out
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod probe {
    use crate::connect::{Opened, Session};

    pub async fn session() -> Session {
        Session::Failed("Only the browser build reaches a hub so far.".into())
    }

    pub async fn socket() -> Opened {
        Opened::TimedOut
    }
}
