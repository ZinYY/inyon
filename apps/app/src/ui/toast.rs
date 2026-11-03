//! Toasts (components/Toasts.tsx) and the question asked before something that can't be undone
//! (components/Confirm.tsx). Both are app-wide: any screen or task calls [`toast`] or awaits
//! [`confirm`], and the layers here, mounted once in the shell, draw them.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use futures_channel::oneshot;

use super::icons::{Glyph, Icon};
use super::kit::Sheet;
use crate::platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastTone {
    Info,
    Success,
    /// needs you: an agent asking, a machine gone quiet
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub title: String,
    pub body: Option<String>,
    pub tone: ToastTone,
    /// a route inside the app that a tap on the toast opens
    pub href: Option<String>,
    /// a button on it (Undo) and what it does
    pub action: Option<(String, super::menu::Action)>,
    /// the agent it's about, whose mark it shows in place of the icon
    pub agent: Option<inyon_protocol::AgentKind>,
}

static TOASTS: GlobalSignal<Vec<Toast>> = Signal::global(Vec::new);
static NEXT: GlobalSignal<u64> = Signal::global(|| 0);

/// How long a toast stays: errors and one with an action (Undo) a little longer, so there's time
/// to read or reach them.
pub fn toast_ms(tone: ToastTone, action: bool) -> u32 {
    if tone == ToastTone::Error || action {
        6000
    } else {
        4500
    }
}

/// The toasts after a new one: one per `href` (a newer notice about the same thing replaces the
/// older), at most three.
pub fn push(list: &[Toast], t: Toast) -> Vec<Toast> {
    let mut v: Vec<Toast> = list.iter().filter(|x| t.href.is_none() || x.href != t.href).cloned().collect();
    let keep = v.len().saturating_sub(2);
    v.drain(..keep);
    v.push(t);
    v
}

/// Shows a toast for a few seconds.
pub fn toast(title: impl Into<String>, body: Option<String>, tone: ToastTone) {
    show(Toast { id: 0, title: title.into(), body: body.filter(|b| !b.is_empty()), tone, href: None, action: None, agent: None });
}

pub fn show(mut t: Toast) {
    let id = *NEXT.peek() + 1;
    *NEXT.write() = id;
    t.id = id;
    let ms = toast_ms(t.tone, t.action.is_some());
    let next = push(&TOASTS.peek(), t);
    *TOASTS.write() = next;
    dioxus::core::spawn_forever(async move {
        platform::sleep(ms).await;
        dismiss(id);
    });
}

pub fn dismiss(id: u64) {
    if TOASTS.peek().iter().any(|t| t.id == id) {
        TOASTS.write().retain(|t| t.id != id);
    }
}

/// "Couldn't copy" or "<what> copied", after putting text on the clipboard.
pub async fn copy(text: &str, what: &str) {
    if crate::dom::copy(text).await {
        toast(format!("{what} copied"), None, ToastTone::Success);
    } else {
        toast("Could not copy", Some("Select it and copy it by hand.".into()), ToastTone::Error);
    }
}

/// How far a toast has to be flicked sideways or up before it goes.
const FLING: f64 = 70.0;

#[component]
pub fn Toasts() -> Element {
    let list = TOASTS.read().clone();
    rsx! {
        div { class: "toasts", "aria-live": "polite",
            for t in list {
                ToastCard { key: "{t.id}", t }
            }
        }
    }
}

#[component]
fn ToastCard(t: Toast) -> Element {
    let mut start = use_signal(|| None::<(f64, f64)>);
    let mut drag = use_signal(|| (0.0f64, 0.0f64));
    let mut moved = use_signal(|| false);
    let mut here = use_context::<Signal<crate::route::Route>>();
    let chats = use_context::<Signal<crate::state::Chats>>();
    let hub = use_context::<Signal<crate::state::Hub>>();
    let (icon, tone) = match t.tone {
        ToastTone::Info => (Icon::Info, "fg-2"),
        ToastTone::Success => (Icon::CircleCheck, ""),
        ToastTone::Warn => (Icon::MessageCircleQuestion, "warn"),
        ToastTone::Error => (Icon::CircleAlert, "danger"),
    };
    let (x, y) = drag();
    let style = if x != 0.0 || y != 0.0 { format!("transform:translate({x}px,{y}px);opacity:{};transition:none", 1.0 - (x.abs() / 300.0).min(0.6)) } else { String::new() };
    let id = t.id;
    let href = t.href.clone();
    rsx! {
        div {
            class: if href.is_some() { "toast toast-link" } else { "toast" },
            role: "status",
            style: "{style}",
            onpointerdown: move |e: PointerEvent| {
                if e.pointer_type() == "touch" {
                    let p = e.client_coordinates();
                    start.set(Some((p.x, p.y)));
                    moved.set(false);
                }
            },
            onpointermove: move |e: PointerEvent| {
                let Some((sx, sy)) = start() else { return };
                let p = e.client_coordinates();
                let (dx, dy) = (p.x - sx, (p.y - sy).min(0.0));
                if dx.abs() > 6.0 || dy < -6.0 {
                    moved.set(true);
                }
                drag.set((dx, dy));
            },
            onpointerup: move |_| {
                if start().is_none() {
                    return;
                }
                start.set(None);
                let (dx, dy) = drag();
                if dx.abs() > FLING || dy < -FLING / 2.0 {
                    dismiss(id);
                } else {
                    drag.set((0.0, 0.0));
                }
            },
            onpointercancel: move |_| {
                start.set(None);
                drag.set((0.0, 0.0));
            },
            onclick: move |_| {
                if moved() {
                    return;
                }
                if let Some(r) = href.as_deref().and_then(crate::route::internal) {
                    crate::route::go(&mut here, r);
                }
                dismiss(id);
            },
            if let Some(k) = t.agent {
                super::icons::AgentMark { kind: k, size: 22 }
            } else {
                Glyph { icon, size: 18, class: "toast-icon {tone}" }
            }
            div { class: "toast-words",
                p { class: "toast-title truncate", "{t.title}" }
                if let Some(b) = &t.body {
                    p { class: "toast-body", "{b}" }
                }
            }
            if let Some((label, action)) = t.action.clone() {
                button {
                    class: "button button-xs button-quiet press toast-action",
                    r#type: "button",
                    onclick: move |e| {
                        e.stop_propagation();
                        dismiss(id);
                        super::menu::run(action.clone(), chats, here, hub);
                    },
                    "{label}"
                }
            }
            button {
                class: "icon-button press",
                r#type: "button",
                "aria-label": "Dismiss",
                onclick: move |e| {
                    e.stop_propagation();
                    dismiss(id);
                },
                Glyph { icon: Icon::X, size: 16 }
            }
        }
    }
}

// ---------- asking first ----------

/// A question before something that can't be undone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ask {
    pub title: String,
    pub body: Option<String>,
    /// what it acts on, as label and value rows
    pub details: Vec<(String, String)>,
    pub yes: String,
    pub danger: bool,
}

#[derive(Clone)]
struct Asking {
    ask: Ask,
    reply: Rc<RefCell<Option<oneshot::Sender<bool>>>>,
}

static ASKING: GlobalSignal<Option<Asking>> = Signal::global(|| None);
static ASK_OPEN: GlobalSignal<bool> = Signal::global(|| false);

fn answer(ok: bool) {
    if let Some(a) = ASKING.peek().as_ref() {
        if let Some(tx) = a.reply.borrow_mut().take() {
            let _ = tx.send(ok);
        }
    }
    *ASK_OPEN.write() = false;
}

/// Asks in the app's own sheet; false when dismissed. A new question answers an open one with no.
pub async fn confirm(ask: Ask) -> bool {
    answer(false);
    let (tx, rx) = oneshot::channel();
    *ASKING.write() = Some(Asking { ask, reply: Rc::new(RefCell::new(Some(tx))) });
    *ASK_OPEN.write() = true;
    rx.await.unwrap_or(false)
}

/// The confirm sheet, mounted once in the shell.
#[component]
pub fn ConfirmHost() -> Element {
    let Some(a) = ASKING.read().clone() else { return rsx! {} };
    let ask = a.ask;
    rsx! {
        Sheet {
            open: ASK_OPEN(),
            on_close: move |_| answer(false),
            title: ask.title.clone(),
            footer: rsx! {
                div { class: "sheet-actions",
                    button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| answer(false), "Cancel" }
                    button {
                        class: if ask.danger { "button button-md button-danger press" } else { "button button-md button-primary press" },
                        r#type: "button",
                        "data-autofocus": "true",
                        onclick: move |_| answer(true),
                        "{ask.yes}"
                    }
                }
            },
            if !ask.details.is_empty() {
                dl { class: "confirm-details",
                    for (k , v) in ask.details.iter().cloned() {
                        div { key: "{k}", class: "contents",
                            dt { class: "fg-3", "{k}" }
                            dd { class: "truncate", title: "{v}", "{v}" }
                        }
                    }
                }
            }
            if let Some(b) = &ask.body {
                p { class: "sheet-text", "{b}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, href: Option<&str>) -> Toast {
        Toast { id, title: format!("t{id}"), body: None, tone: ToastTone::Info, href: href.map(str::to_string), action: None, agent: None }
    }

    #[test]
    fn at_most_three_toasts_and_one_per_thing() {
        let list = push(&[t(1, None), t(2, Some("/next/m/a/p/1")), t(3, None)], t(4, None));
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), [2, 3, 4]);
        let list = push(&list, t(5, Some("/next/m/a/p/1")));
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), [3, 4, 5]);
        assert_eq!(toast_ms(ToastTone::Error, false), 6000);
        assert_eq!(toast_ms(ToastTone::Success, false), 4500);
        assert_eq!(toast_ms(ToastTone::Info, true), 6000, "time to reach Undo");
    }
}
