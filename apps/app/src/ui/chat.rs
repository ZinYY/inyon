//! The conversation (ChatView.tsx): a virtualized list of chat-core's rows with only the ones near
//! the screen in the page, the message box or the agent's question under it, and ChatView's scroll
//! rules (follow new output at the bottom, keep the row being read in place otherwise, fetch older
//! messages before the reader reaches the top).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dioxus::prelude::*;
use inyon_chat_core::pending::PendingStatus;
use inyon_protocol::ClientMsg2;

use super::composer::Composer;
use super::icons::{Glyph, Icon};
use super::prompt::PromptCard;
use super::rows::{Ctx, RowView};
use super::{now, outbox};
use crate::list::{Scroll, Window};
use crate::socket::Outbox;
use crate::state::{self, Chats};
use crate::{dom, platform};

const SCROLLER: &str = "chat-scroll";
const LIST: &str = "chat-list";

/// Asks the hub for the messages before the first one this chat has.
fn load_more(chats: Signal<Chats>, key: &str, out: &Outbox) {
    let before = {
        let c = chats.peek();
        let Some(e) = c.get(key) else { return };
        if e.loading_more || !e.chat.has_more {
            return;
        }
        e.chat.items.first().map(|i| i.seq())
    };
    let Some(before) = before else { return };
    let mut c = chats;
    c.write().entry(key).loading_more = true;
    out.send(state::wire2(&ClientMsg2::More { stream: key.to_string(), before }));
}

/// `controls`: the model, effort and mode as one line, for the message box.
#[component]
pub fn ChatView(ctx: Ctx, connected: bool, controls: String) -> Element {
    let chats = use_context::<Signal<Chats>>();
    let out = outbox();
    let key = ctx.key.clone();
    let scroll = use_hook(|| Rc::new(RefCell::new(Scroll::default())));
    let drawn = use_hook(|| Rc::new(Cell::new(Window { start: 0, end: 0, before: 0, after: 0 })));
    let mut layout = use_signal(|| 0u64);
    let mut away = use_signal(|| false);

    // after each change: scroll to the bottom while following, else keep the row being read where
    // it was; and draw the rows that brings into view
    let settle = {
        let (scroll, drawn, key) = (scroll.clone(), drawn.clone(), key.clone());
        move || {
            let c = chats.peek();
            let Some(e) = c.get(&key) else { return };
            let top = scroll.borrow_mut().settle(&e.list);
            if scroll.borrow().stick {
                dom::set_scroll_top(SCROLLER, 1e9);
            } else {
                dom::set_scroll_top(SCROLLER, dom::offset_top(LIST).unwrap_or(0.0) + top);
            }
            if scroll.borrow().window(&e.list) != drawn.get() {
                layout += 1;
            }
        }
    };
    {
        let mut settle = settle.clone();
        use_effect(move || {
            let _ = chats.read();
            settle();
        });
    }
    // a send that never landed says so after ten minutes; the web app checks every five seconds
    {
        let key = key.clone();
        use_future(move || {
            let key = key.clone();
            async move {
                loop {
                    platform::sleep(5000).await;
                    let waiting = chats.peek().get(&key).is_some_and(|e| e.chat.pending.iter().any(|p| p.status == PendingStatus::Sent && !p.remote));
                    if waiting {
                        let mut c = chats;
                        c.write().tick(&key, now());
                    }
                }
            }
        });
    }

    let on_scroll = {
        let (scroll, drawn, key, out) = (scroll.clone(), drawn.clone(), key.clone(), out.clone());
        move |_| {
            let Some((top, height, view)) = dom::scroll_metrics(SCROLLER) else { return };
            let offset = dom::offset_top(LIST).unwrap_or(0.0);
            let (r, redraw) = {
                let c = chats.peek();
                let Some(e) = c.get(&key) else { return };
                let r = scroll.borrow_mut().scrolled(&e.list, top - offset, view, height - top - view, platform::clock().now);
                (r, scroll.borrow().window(&e.list) != drawn.get())
            };
            if r.away != *away.peek() {
                away.set(r.away);
            }
            if r.more {
                load_more(chats, &key, &out);
            }
            if redraw {
                layout += 1;
            }
        }
    };
    let input = {
        let scroll = scroll.clone();
        move |up: bool| scroll.borrow_mut().input(platform::clock().now, up)
    };

    let c = chats.read();
    let _ = layout();
    let Some(e) = c.get(&key) else { return rsx! { Loading {} } };
    let chat = &e.chat;
    let prompt = chat.screen.as_ref().and_then(|s| s.prompt.clone());
    let working = chat.screen.as_ref().is_some_and(|s| s.working);
    let running = chat.info.as_ref().is_none_or(|i| i.running);
    let rows = chat.rows();
    let w = scroll.borrow().window(&e.list);
    drawn.set(w);
    let first_loaded = chat.loaded;
    let has_more = chat.has_more;
    let loading_more = e.loading_more;
    let draft = chat.draft.as_ref().map(|d| (d.text.clone(), d.at, d.source == inyon_protocol::DraftSource::Terminal));
    let shown: Vec<_> = rows[w.start.min(rows.len())..w.end.min(rows.len())].to_vec();

    let (wheel_in, touch_in, touch_move, press_in, key_in) = (input.clone(), input.clone(), input.clone(), input.clone(), input.clone());
    let jump = {
        let mut settle = settle.clone();
        let scroll = scroll.clone();
        move |_| {
            // straight there in one tap: a glide is slow over a long chat and new output cuts it short
            scroll.borrow_mut().follow();
            away.set(false);
            settle();
        }
    };
    let sent = {
        let mut settle = settle.clone();
        let scroll = scroll.clone();
        move |_| {
            scroll.borrow_mut().follow();
            settle();
        }
    };
    let on_resize = {
        let mut settle = settle.clone();
        let scroll = scroll.clone();
        move |_| {
            if let Some((_, _, view)) = dom::scroll_metrics(SCROLLER) {
                scroll.borrow_mut().view = view;
            }
            settle();
        }
    };
    let more_key = key.clone();
    let more_out = out.clone();
    rsx! {
        div { class: "chat",
            div {
                id: SCROLLER,
                class: "chat-scroll",
                tabindex: "-1",
                onscroll: on_scroll,
                onresize: on_resize,
                onwheel: move |ev: WheelEvent| wheel_in(ev.delta().strip_units().y < 0.0),
                ontouchstart: move |_| touch_in(false),
                // the scrollbar, dragged
                onpointerdown: move |_| press_in(false),
                ontouchmove: move |_| touch_move(false),
                onkeydown: move |ev: KeyboardEvent| key_in(matches!(ev.key(), Key::PageUp | Key::ArrowUp | Key::Home)),
                div { class: "chat-column",
                    if !first_loaded {
                        Loading {}
                    } else if rows.is_empty() && !running {
                        div { class: "empty chat-empty",
                            div { class: "empty-title", "No agent is running in this pane" }
                            div { class: "empty-text", "The session ended or the pane is at a shell prompt. The terminal has it as it is." }
                        }
                    } else {
                        if has_more {
                            div { class: "chat-earlier",
                                if loading_more {
                                    div { class: "chat-loading", role: "status", "aria-label": "Loading earlier messages",
                                        span { class: "skeleton-line" }
                                        span { class: "skeleton-line skeleton-short" }
                                    }
                                } else {
                                    button {
                                        class: "button button-sm button-ghost press",
                                        r#type: "button",
                                        onclick: move |_| load_more(chats, &more_key, &more_out),
                                        Glyph { icon: Icon::History, size: 16 }
                                        "Earlier messages"
                                    }
                                }
                            }
                        }
                        if rows.is_empty() {
                            div { class: "empty chat-empty",
                                div { class: "empty-title", "New conversation" }
                                div { class: "empty-text", "Messages you send here are typed into the terminal, exactly as if you typed them there." }
                            }
                        }
                        div { id: LIST, class: "chat-list",
                            div { style: "height:{w.before}px" }
                            for row in shown {
                                Measured { key: "{row.key()}", id: row.key().into_owned(), chat: key.clone(),
                                    RowView { row: row.clone(), ctx: ctx.clone(), working }
                                }
                            }
                            div { style: "height:{w.after}px" }
                        }
                    }
                }
            }
            div { class: "chat-foot",
                button {
                    class: if away() { "jump press jump-on" } else { "jump press" },
                    r#type: "button",
                    "aria-label": "Jump to latest",
                    "aria-hidden": "{!away()}",
                    tabindex: if away() { "0" } else { "-1" },
                    onclick: jump,
                    Glyph { icon: Icon::ArrowDown, size: 16 }
                }
                if running {
                    div { class: "chat-foot-column",
                        if !connected {
                            div { class: "chat-status fade-in", role: "status", "Reconnecting…" }
                        }
                        if let Some(p) = prompt {
                            PromptCard { prompt: p, host: ctx.host.clone(), pane: ctx.pane.clone(), disabled: !connected, agent: ctx.agent }
                            div { class: "prompt-why",
                                "{ctx.agent_name} is asking you something. Answer it here, and the model, effort and mode controls come back."
                            }
                        } else {
                            Composer {
                                host: ctx.host.clone(),
                                pane: ctx.pane.clone(),
                                agent: ctx.agent_name.clone(),
                                working,
                                disabled: !connected,
                                draft,
                                controls,
                                on_sent: sent,
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A row in the list, its drawn height noted for the list's spacers.
#[component]
fn Measured(id: String, chat: String, children: Element) -> Element {
    let chats = use_context::<Signal<Chats>>();
    rsx! {
        div {
            class: "vrow",
            onresize: move |r: Event<ResizeData>| {
                let Ok(size) = r.get_border_box_size() else { return };
                let h = size.height;
                if chats.peek().get(&chat).is_some_and(|e| e.list.differs(&id, h)) {
                    let mut c = chats;
                    c.write().entry(&chat).list.measured(&id, h);
                }
            },
            {children}
        }
    }
}

#[component]
fn Loading() -> Element {
    rsx! {
        div { class: "chat-loading chat-loading-first", role: "status", "aria-label": "Loading the conversation",
            span { class: "skeleton-line" }
            span { class: "skeleton-line skeleton-short" }
            span { class: "skeleton-line" }
        }
    }
}
