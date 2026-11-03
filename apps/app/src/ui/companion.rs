//! The companion agent (components/CompanionAgent.tsx): an agent you talk to about your other
//! sessions. The hub runs it on its own machine, in a tmux session called companion; its chat is
//! an ordinary session screen. Settings starts and stops it, the sidebar and the Overview lead to
//! it while it runs. Pairing a companion app such as Legato (CompanionSheet.tsx) is here too: a
//! short code the app types with the hub's address. Voice stays in Legato, which talks to the hub.

use dioxus::prelude::*;
use inyon_protocol::{AgentKind, AgentState, HostState, PairedCompanion};

use super::icons::{AgentMark, Glyph, Icon};
use super::kit::{Busy, Field, Segmented, Sheet, WaitRows};
use super::toast::{self, Toast, ToastTone};
use crate::api::{self, CompanionStatus};
use crate::model::{self, PaneRef, Shown};
use crate::route::Route;
use crate::settings as st;
use crate::state::Hub;
use crate::{dom, platform};

const OLD_HUB: &str = "This hub runs on Bun, which has no companion. Run the default Rust hub to use it.";

/// A companion's pane while it runs: each hub marks its own, so an app that shows several hubs can
/// have one from each.
#[derive(Debug, Clone, PartialEq)]
pub struct Companion {
    pub host: String,
    pub host_name: String,
    pub pane: String,
    pub kind: AgentKind,
    pub shown: Shown,
    pub busy: bool,
    pub waiting_for: Option<String>,
}

pub fn companions(hosts: &[HostState]) -> Vec<Companion> {
    model::all_panes(hosts)
        .into_iter()
        .filter(|r: &PaneRef| r.pane.companion == Some(true))
        .filter_map(|r| {
            let a = r.pane.agent.as_ref()?;
            Some(Companion {
                host: r.host.config.id.clone(),
                host_name: r.host.config.name.clone(),
                pane: r.pane.id.clone(),
                kind: a.kind,
                shown: Shown::of(a, r.host),
                busy: model::busy(a),
                waiting_for: if a.state == AgentState::Waiting { a.waiting_for.clone() } else { None },
            })
        })
        .collect()
}

/// "Companion", with its machine when there's more than one.
pub fn name(c: &Companion, all: usize) -> String {
    if all > 1 {
        format!("Companion on {}", c.host_name)
    } else {
        "Companion".into()
    }
}

/// The line under its name on the Overview.
pub fn line(c: &Companion) -> String {
    if c.shown == Shown::Waiting {
        c.waiting_for.clone().unwrap_or_else(|| "It needs your answer.".into())
    } else if c.busy {
        "Working on it…".into()
    } else {
        "Ask about your sessions, or tell it what to send.".into()
    }
}

fn href(c: &Companion) -> String {
    model::pane_href(&c.host, &c.pane)
}

/// What Settings › Companion shows.
#[derive(Debug, Clone, PartialEq)]
enum Status {
    Loading,
    /// the Bun hub, which has none
    Old,
    Failed(String),
    Got(CompanionStatus),
}

/// Settings › Companion: start it with Claude Code or Codex, open its chat, stop it.
#[component]
pub fn CompanionRows() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut status = use_signal(|| Status::Loading);
    let mut agent = use_signal(|| AgentKind::Claude);
    let mut busy = use_signal(|| None::<&'static str>);
    let mut tries = use_signal(|| 0u32);
    let all = companions(&hub.read().hosts);
    // this hub's own, among the ones from other hubs an app may show
    let own = match &*status.read() {
        Status::Got(s) if !s.host.is_empty() => s.host.clone(),
        _ => "local".into(),
    };
    // a companion just stopped: its pane stays in the overview until the machine's next report
    let mut stopped = use_signal(|| None::<String>);
    let running = all.iter().find(|c| c.host == own && stopped.read().as_deref() != Some(c.pane.as_str())).cloned();
    // once the overview has dropped the stopped pane, a companion in that pane again is a new one
    let reported_gone = stopped.read().as_ref().is_some_and(|p| !all.iter().any(|c| c.host == own && c.pane == *p));
    use_effect(use_reactive!(|reported_gone| {
        if reported_gone {
            stopped.set(None);
        }
    }));
    let running_id = running.as_ref().map(|c| c.pane.clone());
    let t = tries();
    use_effect(use_reactive!(|running_id, t| {
        let _ = (&running_id, t);
        spawn(async move {
            status.set(match api::companion().await {
                Ok(s) => Status::Got(s),
                Err(e) if e.status == 404 => Status::Old,
                Err(e) => Status::Failed(e.message),
            });
        });
    }));

    let start = move |kind: AgentKind| {
        busy.set(Some("start"));
        spawn(async move {
            match api::start_companion(kind).await {
                Ok(s) => {
                    stopped.set(None);
                    let link = s.pane.as_ref().map(|p| model::pane_href(&s.host, p));
                    status.set(Status::Got(s));
                    toast::show(Toast {
                        id: 0,
                        title: "Companion started".into(),
                        body: Some(format!("{} in the tmux session companion.", kind.def().name)),
                        tone: ToastTone::Success,
                        href: link,
                        action: None,
                        agent: None,
                    });
                }
                Err(e) => toast::toast("Could not start the companion", Some(e.message), ToastTone::Error),
            }
            busy.set(None);
        });
    };
    // the pane it ran in, kept off the list until the overview drops it
    let stop = move |pane: Option<String>| {
        busy.set(Some("stop"));
        spawn(async move {
            match api::stop_companion().await {
                Ok(()) => {
                    stopped.set(pane);
                    status.set(Status::Got(CompanionStatus { agent: None, host: "local".into(), pane: None }));
                    toast::toast("Companion stopped", None, ToastTone::Info);
                }
                Err(e) => toast::toast("Could not stop the companion", Some(e.message), ToastTone::Error),
            }
            busy.set(None);
        });
    };

    let s = status();
    match s {
        Status::Loading => rsx! { WaitRows { label: "Loading the companion", count: 1, class: "set-wait" } },
        Status::Old => rsx! { p { class: "set-empty", "{OLD_HUB}" } },
        Status::Failed(e) => rsx! {
            div { class: "set-row",
                div { class: "set-words",
                    div { class: "set-label", "Couldn’t reach the companion" }
                    div { class: "set-hint", "{e}" }
                }
                button {
                    class: "button button-sm button-ghost press",
                    r#type: "button",
                    onclick: move |_| {
                        status.set(Status::Loading);
                        tries += 1;
                    },
                    "Try again"
                }
            }
        },
        Status::Got(s) => {
            if let Some(r) = running {
                let to = Route::Session { host: r.host.clone(), pane: r.pane.clone(), view: None };
                let pane = r.pane.clone();
                let mut stop = stop;
                rsx! {
                    div { class: "set-row",
                        div { class: "set-words set-agent",
                            AgentMark { kind: r.kind, size: 28 }
                            span { class: "set-agent-words",
                                span { class: "set-agent-name", "{r.kind.def().name}" }
                                span { class: "set-agent-line truncate", "On {r.host_name}, in the tmux session companion" }
                            }
                        }
                        span { class: "pill",
                            span { class: "dot dot-{r.shown.class()}" }
                            "{r.shown.label()}"
                        }
                        div { class: "button-row",
                            button { class: "button button-sm button-primary press", r#type: "button", onclick: move |_| crate::route::go(&mut here, to.clone()), "Open" }
                            button { class: "button button-sm button-ghost press", r#type: "button", disabled: busy().is_some(), onclick: move |_| stop(Some(pane.clone())),
                                Busy { on: busy() == Some("stop") }
                                "Stop"
                            }
                        }
                    }
                }
            } else if let Some(kind) = s.agent {
                // turned on, but its agent quit or its session was closed
                let mut stop = stop;
                let mut start = start;
                rsx! {
                    div { class: "set-row",
                        div { class: "set-words",
                            div { class: "set-label", "{kind.def().name} isn’t running" }
                            div { class: "set-hint", "It quit, or its session was closed." }
                        }
                        div { class: "button-row",
                            button { class: "button button-sm button-primary press", r#type: "button", disabled: busy().is_some(), onclick: move |_| start(kind),
                                Busy { on: busy() == Some("start") }
                                "Start again"
                            }
                            button { class: "button button-sm button-ghost press", r#type: "button", disabled: busy().is_some(), onclick: move |_| stop(None),
                                Busy { on: busy() == Some("stop") }
                                "Turn off"
                            }
                        }
                    }
                }
            } else {
                let mut start = start;
                rsx! {
                    div { class: "set-row",
                        div { class: "set-words",
                            div { class: "set-label", "Agent" }
                            div { class: "set-hint", "Runs on the hub’s machine with your own sign-in." }
                        }
                        div { class: "button-row",
                            Segmented {
                                value: agent().as_str(),
                                options: vec![("claude".to_string(), "Claude Code".to_string()), ("codex".to_string(), "Codex".to_string())],
                                label: "Companion agent",
                                on_change: move |v: String| agent.set(if v == "codex" { AgentKind::Codex } else { AgentKind::Claude }),
                            }
                            button { class: "button button-sm button-primary press", r#type: "button", disabled: busy().is_some(), onclick: move |_| start(agent()),
                                Busy { on: busy() == Some("start") }
                                "Start"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The sidebar's way to each companion, under the Overview, while it runs.
#[component]
pub fn CompanionNav(current: Option<(String, String)>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let all = companions(&hub.read().hosts);
    let n = all.len();
    rsx! {
        for c in all {
            {
                let on = current.as_ref().is_some_and(|(h, p)| *h == c.host && *p == c.pane);
                rsx! {
                    a {
                        key: "{c.host}/{c.pane}",
                        class: if on { "nav-item nav-active" } else { "nav-item" },
                        href: "{href(&c)}",
                        "aria-current": on.then_some("page"),
                        Glyph { icon: Icon::MessageCircle, size: 16 }
                        span { class: "truncate", {name(&c, n)} }
                        span { class: "nav-dot dot dot-{c.shown.class()}", title: "{c.shown.label()}" }
                    }
                }
            }
        }
    }
}

/// The Overview's line about each companion, while it runs: what it's doing and a way into its chat.
#[component]
pub fn CompanionBar() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let all = companions(&hub.read().hosts);
    if all.is_empty() {
        return rsx! {};
    }
    let n = all.len();
    rsx! {
        div { class: "companion-bar",
            for c in all {
                a { key: "{c.host}/{c.pane}", class: "companion-line press", href: "{href(&c)}",
                    AgentMark { kind: c.kind, size: 22 }
                    span { class: "companion-words",
                        span { class: "companion-name",
                            {name(&c, n)}
                            span { class: "dot dot-{c.shown.class()}", title: "{c.shown.label()}" }
                        }
                        span { class: if c.shown == Shown::Waiting { "companion-text truncate" } else { "companion-text truncate fg-3" }, {line(&c)} }
                    }
                    Glyph { icon: Icon::ArrowRight, size: 16, class: "fg-3 companion-arrow" }
                }
            }
        }
    }
}

// ---------- pairing a companion app ----------

/// Pair a companion app (Legato and the like): a six-character code it types with the hub's
/// address. The code works once, for five minutes, and a new one is made when it runs out while
/// the sheet is open. The sheet watches the list of companions and says so when the app has
/// claimed it. Like the phone code, it needs a fresh sign-in.
#[component]
pub fn CompanionSheet(open: bool, on_close: EventHandler<()>, #[props(default)] on_paired: Option<EventHandler<PairedCompanion>>) -> Element {
    let mut code = use_signal(|| None::<inyon_protocol::CompanionPairCode>);
    let mut paired = use_signal(|| None::<PairedCompanion>);
    let mut need_password = use_signal(|| false);
    let mut password = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut now = use_signal(|| platform::clock().now);
    // the companions there were when the sheet opened; a new id is the one that just claimed
    let mut known = use_signal(|| None::<Vec<String>>);
    let mut gen = use_signal(|| 0u64);

    // asks the hub for a code; false when it didn't make one (the old code, if any, still works).
    // An answer for an earlier opening of the sheet changes nothing in this one.
    let fetch = move || async move {
        let g = *gen.peek();
        error.set(None);
        busy.set(true);
        let answer = api::companion_pair().await;
        if *gen.peek() != g {
            return false;
        }
        let ok = match answer {
            Ok(c) => {
                code.set(Some(c));
                now.set(platform::clock().now);
                need_password.set(false);
                true
            }
            Err(e) if e.status == 403 => {
                need_password.set(true);
                false
            }
            Err(e) => {
                error.set(Some(e.message));
                false
            }
        };
        busy.set(false);
        ok
    };

    use_effect(use_reactive!(|open| {
        let g = *gen.peek() + 1;
        gen.set(g);
        if !open {
            return;
        }
        code.set(None);
        paired.set(None);
        password.set(String::new());
        known.set(None);
        busy.set(false);
        spawn(async move {
            // the companions there are before the code exists, so the one that claims it is the new
            // one; a read that fails is tried again, here and then while the code shows
            for wait in [0, 700, 2000] {
                platform::sleep(wait).await;
                let list = api::companions().await;
                if *gen.peek() != g {
                    return;
                }
                if let Ok(list) = list {
                    known.set(Some(list.into_iter().map(|c| c.id).collect()));
                    break;
                }
            }
            fetch().await;
            // the countdown, a new code when it runs out, and the app's claim, looked for in the list
            let mut renewed_for = 0u64;
            let mut tick = 0u32;
            loop {
                platform::sleep(1000).await;
                if *gen.peek() != g {
                    break;
                }
                tick += 1;
                let at = platform::clock().now;
                now.set(at);
                if paired.peek().is_some() {
                    continue;
                }
                let expired = code.peek().as_ref().map(|c| c.expires_at).filter(|e| at >= *e as f64 && renewed_for != *e);
                if let Some(e) = expired {
                    renewed_for = e;
                    // a renewal that fails drops the dead code, which shows the error and Try again;
                    // the list is still watched, for an app that claimed it just before it ran out
                    if !fetch().await && *gen.peek() == g {
                        code.set(None);
                    }
                    continue;
                }
                // nothing to claim before the first code
                if code.peek().is_none() && renewed_for == 0 {
                    continue;
                }
                if tick.is_multiple_of(2) && known.peek().is_none() {
                    // still no list from before: one read now is the next best thing
                    if let Ok(list) = api::companions().await {
                        if *gen.peek() == g {
                            known.set(Some(list.into_iter().map(|c| c.id).collect()));
                        }
                    }
                } else if tick.is_multiple_of(2) {
                    if let (Ok(list), Some(k)) = (api::companions().await, known.peek().clone()) {
                        if let Some(fresh) = list.into_iter().find(|c| !k.contains(&c.id)) {
                            if *gen.peek() == g {
                                paired.set(Some(fresh.clone()));
                                if let Some(f) = on_paired {
                                    f.call(fresh);
                                }
                            }
                        }
                    }
                }
            }
        });
    }));

    let address = dom::origin();
    let renew = move |_| {
        spawn(async move {
            if fetch().await {
                toast::toast("New code", Some("The old one no longer works.".into()), ToastTone::Info);
            } else {
                toast::toast("Couldn't make a new code", Some("The one shown still works.".into()), ToastTone::Error);
            }
        });
    };
    let c = code();
    let footer = (c.is_some() && !need_password()).then(|| {
        rsx! {
            div { class: "sheet-actions sheet-actions-end",
                if paired().is_none() {
                    button { class: "button button-md button-ghost press", r#type: "button", disabled: busy(), onclick: renew,
                        if busy() {
                            Busy { on: true }
                        } else {
                            Glyph { icon: Icon::RefreshCw, size: 14 }
                        }
                        "New code"
                    }
                }
                button { class: "button button-md button-primary press", r#type: "button", onclick: move |_| on_close.call(()), "Done" }
            }
        }
    });
    rsx! {
        Sheet { open, on_close: move |_| on_close.call(()), title: "Pair a companion app", icon: rsx! { Glyph { icon: Icon::Mic, size: 16 } }, footer,
            if need_password() {
                form {
                    class: "stack",
                    onsubmit: move |e| {
                        e.prevent_default();
                        let pw = password();
                        busy.set(true);
                        spawn(async move {
                            match api::login(&pw).await {
                                Ok(()) => {
                                    password.set(String::new());
                                    fetch().await;
                                }
                                Err(e) => {
                                    error.set(Some(e.message));
                                    busy.set(false);
                                }
                            }
                        });
                    },
                    p { class: "sheet-text", "Pairing an app needs a fresh sign-in. Enter the hub's password once more." }
                    Field { label: "Password",
                        input { class: "field", r#type: "password", autocomplete: "current-password", "data-autofocus": "true", value: "{password}", oninput: move |e| password.set(e.value()) }
                    }
                    if let Some(e) = error() {
                        p { class: "form-error", "{e}" }
                    }
                    button { class: "button button-md button-primary press button-block", r#type: "submit", disabled: password().is_empty() || busy(),
                        Busy { on: busy() }
                        "Continue"
                    }
                }
            } else if let Some(c) = c {
                {
                    let chars: Vec<char> = c.code.chars().collect();
                    let spoken = chars.iter().map(char::to_string).collect::<Vec<_>>().join(" ");
                    let link = st::companion_link(&address, &c.code, c.tls_fingerprint.as_deref());
                    let addr = address.clone();
                    rsx! {
                        div { class: "stack",
                            ol { class: "pair-steps",
                                li {
                                    span { class: "pair-step mono", "1" }
                                    span {
                                        "In Legato, open "
                                        span { class: "strong", "Settings › Connections › Inyon" }
                                        "."
                                    }
                                }
                                li {
                                    span { class: "pair-step mono", "2" }
                                    span { "Type this code and the hub address." }
                                }
                            }
                            div { class: "pair-code", role: "img", "aria-label": "Pairing code {spoken}",
                                for (i , ch) in chars.iter().enumerate() {
                                    span { key: "{i}", class: "contents",
                                        if i == 3 {
                                            span { class: "pair-dash", "aria-hidden": "true" }
                                        }
                                        span { class: "pair-box mono", "aria-hidden": "true", "{ch}" }
                                    }
                                }
                            }
                            p { class: "pair-left tabular",
                                if paired().is_some() {
                                    "Used"
                                } else {
                                    {format!("Works once · changes in {}", st::mmss(c.expires_at as f64 - now()))}
                                }
                            }
                            div { class: "pair-address",
                                span { class: "mono truncate", "{address}" }
                                button {
                                    class: "icon-button press",
                                    r#type: "button",
                                    "aria-label": "Copy the hub address",
                                    title: "Copy the hub address",
                                    onclick: move |_| {
                                        let a = addr.clone();
                                        spawn(async move {
                                            if dom::copy(&a).await {
                                                toast::toast("Hub address copied", None, ToastTone::Success);
                                            }
                                        });
                                    },
                                    Glyph { icon: Icon::Copy, size: 14 }
                                }
                            }
                            if st::local_address(&address) {
                                p { class: "pair-hint", "{st::LOCAL_HINT}" }
                            }
                            if paired().is_none() {
                                button {
                                    class: "pair-link press",
                                    r#type: "button",
                                    onclick: move |_| {
                                        let l = link.clone();
                                        spawn(async move {
                                            if dom::copy(&l).await {
                                                toast::toast("Pairing link copied", Some("It works once, like the code.".into()), ToastTone::Success);
                                            }
                                        });
                                    },
                                    Glyph { icon: Icon::Link2, size: 14 }
                                    "Copy pairing link"
                                }
                            }
                            if let Some(p) = paired() {
                                PairDone { name: p.name }
                            } else {
                                p { class: "pair-wait", role: "status",
                                    span { class: "pulse-dot" }
                                    "Waiting for the app to send the code…"
                                }
                            }
                        }
                    }
                }
            } else if let Some(p) = paired() {
                // claimed just before its code ran out, with no new code to show
                PairDone { name: p.name }
            } else if let Some(e) = error() {
                div { class: "stack",
                    p { class: "form-error", "{e}" }
                    button {
                        class: "button button-md button-primary press",
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            spawn(async move {
                                fetch().await;
                            });
                        },
                        Busy { on: busy() }
                        "Try again"
                    }
                }
            } else {
                div { class: "skeleton skeleton-panel", role: "status", "aria-label": "Making a code" }
            }
        }
    }
}

/// The app that claimed the code, and what it may do.
#[component]
fn PairDone(name: String) -> Element {
    rsx! {
        div { class: "pair-done", role: "status",
            div { class: "pair-done-title",
                Glyph { icon: Icon::CircleCheck, size: 16, class: "ok" }
                "{name} is paired"
            }
            ul { class: "pair-can",
                for s in st::COMPANION_CAN {
                    li { key: "{s}",
                        Glyph { icon: Icon::Check, size: 14, class: "ok" }
                        "{s}"
                    }
                }
                li { class: "fg-3",
                    Glyph { icon: Icon::Minus, size: 14 }
                    "{st::COMPANION_CANT}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(shown: Shown, busy: bool, waiting_for: Option<&str>) -> Companion {
        Companion { host: "local".into(), host_name: "hub".into(), pane: "%9".into(), kind: AgentKind::Claude, shown, busy, waiting_for: waiting_for.map(Into::into) }
    }

    #[test]
    fn the_companion_line_says_what_it_is_doing() {
        assert_eq!(line(&comp(Shown::Waiting, false, Some("Send it to box?"))), "Send it to box?");
        assert_eq!(line(&comp(Shown::Waiting, false, None)), "It needs your answer.");
        assert_eq!(line(&comp(Shown::Working, true, None)), "Working on it…");
        assert_eq!(line(&comp(Shown::Idle, false, None)), "Ask about your sessions, or tell it what to send.");
        assert_eq!(name(&comp(Shown::Idle, false, None), 1), "Companion");
        assert_eq!(name(&comp(Shown::Idle, false, None), 2), "Companion on hub");
        assert_eq!(href(&comp(Shown::Idle, false, None)), "/next/m/local/p/9");
    }
}
