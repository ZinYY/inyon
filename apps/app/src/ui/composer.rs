//! The message box under the chat (Composer.tsx): Enter sends (Shift+Enter for a new line), a
//! message sent while the agent works waits in its queue, Esc in an empty box stops the agent and
//! Alt+Enter holds the message until it's free. What you type is the pane's draft, kept in step
//! with the terminal's input line and other devices through the hub's `draft` message.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dioxus::prelude::*;
use inyon_chat_core::pending::{PendingMessage, PendingStatus};
use inyon_protocol::ClientMsg;

use super::icons::{Glyph, Icon};
use super::{now, outbox};
use crate::state::{self, Chats};
use crate::{api, dom, platform};

pub const BOX: &str = "composer-input";
/// How long typing pauses before the draft goes to the hub.
const DRAFT_MS: u32 = 450;

/// What the one round button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Send,
    /// the agent is working: the message waits in its queue
    Queue,
    /// the agent is working and the box is empty
    Stop,
}

pub fn button(working: bool, can_send: bool) -> Button {
    match (working, can_send) {
        (true, false) => Button::Stop,
        (true, true) => Button::Queue,
        _ => Button::Send,
    }
}

/// Whether a draft from the terminal (or another device) takes the box: unless it's what the box
/// last synced, or you're in the box with your own edit.
pub fn adopt(server: &str, synced: &str, text: &str, focused: bool) -> bool {
    server != synced && !(focused && text != synced)
}

/// Sends a message: it shows as on its way at once, then as sent, or as failed with a Retry.
pub fn send(chats: Signal<Chats>, host: String, pane: String, text: String) {
    let id = dom::random_id();
    let mut c = chats;
    let key = inyon_protocol::chat_key(&host, &pane);
    let p = PendingMessage::new(id.clone(), text.clone(), PendingStatus::Sending, now());
    c.write().patch(&key, now(), move |d| d.pending.push(p));
    deliver(chats, host, pane, id, text, false);
}

/// Sends a message that failed again, under the same request id (the hub submits it once).
pub fn retry(chats: Signal<Chats>, host: String, pane: String, id: String, text: String) {
    let key = inyon_protocol::chat_key(&host, &pane);
    let mut c = chats;
    let i = id.clone();
    c.write().patch(&key, now(), move |d| {
        for p in d.pending.iter_mut().filter(|p| p.id == i) {
            p.status = PendingStatus::Sending;
            p.at = now();
            p.retryable = false;
            p.error = None;
        }
    });
    deliver(chats, host, pane, id, text, true);
}

pub fn dismiss(chats: Signal<Chats>, key: &str, id: &str) {
    let mut c = chats;
    let id = id.to_string();
    c.write().patch(key, now(), move |d| d.pending.retain(|p| p.id != id));
}

fn deliver(chats: Signal<Chats>, host: String, pane: String, id: String, text: String, again: bool) {
    let key = inyon_protocol::chat_key(&host, &pane);
    spawn(async move {
        let result = api::send(&host, &pane, &text, &id).await;
        let mut c = chats;
        c.write().patch(&key, now(), move |d| {
            for p in d.pending.iter_mut().filter(|p| p.id == id) {
                match &result {
                    Ok(()) => p.status = PendingStatus::Sent,
                    Err(e) => {
                        p.status = PendingStatus::Failed;
                        p.error = Some(e.message.clone());
                        // a dropped connection sends it again once it's back; a retry that failed
                        // too waits for a tap
                        p.retryable = !again && api::is_transient(e);
                    }
                }
            }
        });
    });
}

/// `draft`: the pane's draft as the hub last sent it (its text, when, and whether the terminal
/// wrote it). `controls`: the model, effort and mode, as one line.
#[component]
pub fn Composer(
    host: String,
    pane: String,
    agent: String,
    working: bool,
    disabled: bool,
    draft: Option<(String, u64, bool)>,
    controls: String,
    on_sent: EventHandler<()>,
) -> Element {
    let chats = use_context::<Signal<Chats>>();
    let out = outbox();
    let mut text = use_signal(|| draft.as_ref().map(|d| d.0.clone()).unwrap_or_default());
    let mut from_terminal = use_signal(|| draft.as_ref().is_some_and(|d| d.2 && !d.0.is_empty()));
    let mut focused = use_signal(|| false);
    let mut stopping = use_signal(|| false);
    // what the hub has as the draft, as far as this box knows
    let synced = use_hook(|| Rc::new(RefCell::new(draft.as_ref().map(|d| d.0.clone()).unwrap_or_default())));
    let typed = use_hook(|| Rc::new(Cell::new(0u32)));

    // a draft from the terminal or another device
    {
        let synced = synced.clone();
        use_effect(use_reactive!(|draft| {
            let Some((server, _, terminal)) = draft else { return };
            if adopt(&server, &synced.borrow(), &text.peek(), *focused.peek()) {
                *synced.borrow_mut() = server.clone();
                text.set(server);
                from_terminal.set(terminal);
            }
        }));
    }
    // ready to type on arrival, except on a phone, where that would pop the keyboard up
    use_hook(|| {
        if !dom::coarse() {
            spawn(async { dom::focus(BOX) });
        }
    });
    use_effect(move || {
        let _ = text.read();
        dom::autosize(BOX, 0.4);
    });

    let push_draft = {
        let (synced, out, host, pane) = (synced.clone(), out.clone(), host.clone(), pane.clone());
        move |value: String| {
            if *synced.borrow() != value {
                *synced.borrow_mut() = value.clone();
                out.send(state::wire(&ClientMsg::Draft { host: host.clone(), pane: pane.clone(), text: value }));
            }
        }
    };
    let on_input = {
        let (typed, push_draft) = (typed.clone(), push_draft.clone());
        move |e: FormEvent| {
            let v = e.value();
            text.set(v.clone());
            from_terminal.set(false);
            let n = typed.get() + 1;
            typed.set(n);
            let (typed, push) = (typed.clone(), push_draft.clone());
            spawn(async move {
                platform::sleep(DRAFT_MS).await;
                if typed.get() == n {
                    push(v);
                }
            });
        }
    };
    let can_send = !text.read().trim().is_empty() && !disabled;
    let mode = button(working, can_send);
    let do_send = {
        let (host, pane, synced, typed) = (host.clone(), pane.clone(), synced.clone(), typed.clone());
        move || {
            let msg = text.peek().trim().to_string();
            if msg.is_empty() || disabled {
                return;
            }
            // the box empties here and on the hub; the terminal's line is cleared by the send
            typed.set(typed.get() + 1);
            *synced.borrow_mut() = String::new();
            text.set(String::new());
            from_terminal.set(false);
            if dom::coarse() {
                dom::blur(BOX);
            }
            send(chats, host.clone(), pane.clone(), msg);
            on_sent.call(());
        }
    };
    let stop = {
        let (host, pane) = (host.clone(), pane.clone());
        move || {
            stopping.set(true);
            let (host, pane) = (host.clone(), pane.clone());
            spawn(async move {
                let _ = api::interrupt(&host, &pane).await;
                stopping.set(false);
            });
        }
    };
    let hold = {
        let (host, pane, synced, typed) = (host.clone(), pane.clone(), synced.clone(), typed.clone());
        move || {
            let msg = text.peek().trim().to_string();
            if msg.is_empty() {
                return;
            }
            let (host, pane, synced, typed) = (host.clone(), pane.clone(), synced.clone(), typed.clone());
            spawn(async move {
                if api::hold(&host, &pane, &msg).await.is_ok() {
                    typed.set(typed.get() + 1);
                    *synced.borrow_mut() = String::new();
                    text.set(String::new());
                }
            });
        }
    };
    let on_key = {
        let (mut do_send, mut stop, hold) = (do_send.clone(), stop.clone(), hold.clone());
        move |e: KeyboardEvent| {
            if e.is_composing() {
                return;
            }
            let m = e.modifiers();
            match e.key() {
                Key::Escape if working && text.peek().is_empty() => {
                    e.prevent_default();
                    stop();
                }
                Key::Enter if m.alt() => {
                    e.prevent_default();
                    hold();
                }
                // on a touch screen Enter is a new line; the button sends
                Key::Enter if !m.shift() && !dom::coarse() => {
                    e.prevent_default();
                    do_send();
                }
                _ => {}
            }
        }
    };
    let placeholder = if working { format!("Queue a message for {agent}…") } else { format!("Message {agent}…") };
    let (label, title) = match mode {
        Button::Stop => ("Stop", "Stop (Esc in the terminal)"),
        Button::Queue => ("Queue follow-up", "Queue a follow-up"),
        Button::Send => ("Send", "Send"),
    };
    let note = if !text.read().is_empty() && from_terminal() { "From the terminal" } else { "" };
    let (mut do_send, mut stop) = (do_send, stop);
    rsx! {
        div { class: "composer fade-in",
            div {
                class: if disabled { "composer-box composer-off" } else { "composer-box" },
                // measured again once the page's styles and fonts settle, or the width changes
                onresize: move |_| dom::autosize(BOX, 0.4),
                textarea {
                    id: BOX,
                    class: "composer-text",
                    rows: "1",
                    value: "{text}",
                    disabled,
                    placeholder: "{placeholder}",
                    "aria-label": "Message {agent}",
                    oninput: on_input,
                    onkeydown: on_key,
                    onfocus: move |_| focused.set(true),
                    onblur: move |_| focused.set(false),
                }
                div { class: "composer-bar",
                    if !controls.is_empty() {
                        span { class: "composer-controls mono truncate", title: "Model, effort and mode", "{controls}" }
                    }
                    span { class: "composer-note truncate", "{note}" }
                    button {
                        class: match mode {
                            Button::Stop => "send-button press send-stop",
                            _ if can_send => "send-button press send-ready",
                            _ => "send-button press",
                        },
                        r#type: "button",
                        "aria-label": label,
                        title,
                        disabled: if mode == Button::Stop { stopping() } else { !can_send },
                        onmousedown: move |e| e.prevent_default(),
                        onclick: move |_| if mode == Button::Stop { stop() } else { do_send() },
                        span { key: "{label}", class: "fade-in send-icon",
                            if mode == Button::Stop {
                                Glyph { icon: Icon::Square, size: 12, class: "fill" }
                            } else {
                                Glyph { icon: Icon::ArrowUp, size: 17 }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_button_sends_queues_or_stops() {
        assert_eq!(button(false, true), Button::Send);
        assert_eq!(button(false, false), Button::Send);
        assert_eq!(button(true, true), Button::Queue);
        assert_eq!(button(true, false), Button::Stop);
    }

    #[test]
    fn a_draft_from_elsewhere_waits_while_you_edit() {
        // the terminal typed something new and the box is as last synced: it takes it
        assert!(adopt("fix the tests", "", "", false));
        assert!(adopt("fix the tests", "fix", "fix", true));
        // you're mid-edit with other text: yours stays
        assert!(!adopt("fix the tests", "fix", "fix the build", true));
        // the same text the box last sent back: nothing to do
        assert!(!adopt("fix", "fix", "", false));
    }
}
