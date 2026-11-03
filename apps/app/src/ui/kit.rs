//! The shared controls the web app keeps in components/ui (primitives.tsx, Select.tsx): a sheet that
//! comes up from the bottom on a phone and sits in the middle on a wider screen, a select that is
//! never the browser's own picker, a switch, a segmented choice, and rows that hold a list's place
//! while it loads. One component per job; screens use these rather than their own copies.

use dioxus::prelude::*;

use super::icons::{Glyph, Icon};
use crate::{dom, platform};

/// How long a sheet takes to go away (its exit animation), before it leaves the page.
const SHEET_EXIT_MS: u32 = 200;

/// A sheet: from the bottom on a phone, a centred dialog from tablet width up. Esc and the
/// backdrop close it, focus goes in and stays in while it's open, and comes back where it was.
#[component]
pub fn Sheet(
    open: bool,
    on_close: EventHandler<()>,
    title: String,
    #[props(default)] icon: Option<Element>,
    #[props(default)] wide: bool,
    #[props(default)] footer: Option<Element>,
    children: Element,
) -> Element {
    let mut shown = use_signal(|| open);
    let mut gen = use_signal(|| 0u32);
    let id = use_hook(|| format!("sheet-{}", dom::random_id()));
    {
        let id = id.clone();
        use_effect(use_reactive!(|open| {
            let g = *gen.peek() + 1;
            gen.set(g);
            if open {
                dom::push_focus();
                shown.set(true);
                let id = id.clone();
                // once it's drawn: focus into it
                spawn(async move {
                    platform::sleep(20).await;
                    dom::focus_into(&id);
                });
            } else if *shown.peek() {
                dom::pop_focus();
                spawn(async move {
                    platform::sleep(SHEET_EXIT_MS).await;
                    if *gen.peek() == g {
                        shown.set(false);
                    }
                });
            }
        }));
    }
    if !shown() {
        return rsx! {};
    }
    let title_id = format!("{id}-title");
    let state = if open { "sheet-open" } else { "sheet-closing" };
    let size = if wide { "sheet-panel sheet-wide" } else { "sheet-panel" };
    let trap = id.clone();
    rsx! {
        div { class: "sheet {state}", role: "dialog", "aria-modal": "true", "aria-labelledby": "{title_id}",
            div { class: "sheet-backdrop", onclick: move |_| on_close.call(()) }
            div {
                id: "{id}",
                class: "{size}",
                tabindex: "-1",
                onkeydown: move |e: KeyboardEvent| {
                    match e.key() {
                        // a sheet over another one closes alone
                        Key::Escape => {
                            e.stop_propagation();
                            e.prevent_default();
                            on_close.call(());
                        }
                        Key::Tab => {
                            e.stop_propagation();
                            if dom::trap_tab(&trap, e.modifiers().shift()) {
                                e.prevent_default();
                            }
                        }
                        _ => {}
                    }
                },
                div { class: "sheet-grip" }
                div { class: "sheet-head",
                    {icon}
                    h2 { id: "{title_id}", class: "sheet-title", "{title}" }
                    button { class: "icon-button icon-button-lg press sheet-close", r#type: "button", "aria-label": "Close", title: "Close", onclick: move |_| on_close.call(()),
                        Glyph { icon: Icon::X, size: 16 }
                    }
                }
                div { class: "sheet-body", {children} }
                if let Some(f) = footer {
                    div { class: "sheet-foot", {f} }
                }
            }
        }
    }
}

/// A label over a field, with a hint under it.
#[component]
pub fn Field(label: String, #[props(default)] hint: Option<String>, #[props(default)] group: bool, children: Element) -> Element {
    // `group` for a control that isn't a form input, such as a Select: inside a <label> every click
    // on its list, sheet or backdrop would also click the field again and open the list back up
    if group {
        return rsx! {
            div { class: "field-wrap", role: "group", "aria-label": "{label}",
                span { class: "field-label", "{label}" }
                {children}
                if let Some(h) = hint {
                    span { class: "field-hint", "{h}" }
                }
            }
        };
    }
    rsx! {
        label { class: "field-wrap",
            span { class: "field-label", "{label}" }
            {children}
            if let Some(h) = hint {
                span { class: "field-hint", "{h}" }
            }
        }
    }
}

/// An on/off switch.
#[component]
pub fn Toggle(on: bool, label: String, on_change: EventHandler<bool>, #[props(default)] disabled: bool) -> Element {
    rsx! {
        button {
            class: if on { "toggle toggle-on" } else { "toggle" },
            r#type: "button",
            role: "switch",
            "aria-checked": "{on}",
            "aria-label": "{label}",
            disabled,
            onclick: move |_| on_change.call(!on),
            span { class: "toggle-knob" }
        }
    }
}

/// One of a few choices side by side. `full` stretches it across its row; `icons`, when given, go
/// before the labels in the same order.
#[component]
pub fn Segmented(
    value: String,
    options: Vec<(String, String)>,
    on_change: EventHandler<String>,
    #[props(default)] full: bool,
    #[props(default)] label: Option<String>,
    #[props(default)] icons: Vec<Icon>,
) -> Element {
    rsx! {
        div { class: if full { "segmented segmented-inline segmented-full" } else { "segmented segmented-inline" }, role: "radiogroup", "aria-label": label,
            for (i , (v , l)) in options.into_iter().enumerate() {
                button {
                    key: "{v}",
                    class: if v == value { "segment segment-on" } else { "segment" },
                    r#type: "button",
                    role: "radio",
                    "aria-checked": "{v == value}",
                    onclick: { let v = v.clone(); move |_| on_change.call(v.clone()) },
                    if let Some(icon) = icons.get(i) {
                        Glyph { icon: *icon, size: 14 }
                    }
                    "{l}"
                }
            }
        }
    }
}

/// Rows in a list's shape while it loads, with no spinner.
#[component]
pub fn WaitRows(label: String, #[props(default = 3)] count: usize, #[props(default)] class: String) -> Element {
    rsx! {
        div { class: "panel wait-rows {class}", role: "status", "aria-label": "{label}",
            for i in 0..count {
                div { key: "{i}", class: "wait-row",
                    div { class: "skeleton skeleton-tile" }
                    div { class: "wait-lines",
                        div { class: "skeleton skeleton-text skeleton-w28" }
                        div { class: "skeleton skeleton-text skeleton-w44" }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Error,
}

/// A short boxed sentence: something worked, something to know, or what went wrong.
#[component]
pub fn Notice(tone: Tone, children: Element) -> Element {
    let (class, icon) = match tone {
        Tone::Ok => ("notice notice-ok", Icon::Check),
        Tone::Warn => ("notice notice-warn", Icon::CircleAlert),
        Tone::Error => ("notice notice-error", Icon::TriangleAlert),
    };
    rsx! {
        div { class: "{class} slide-up", role: if tone == Tone::Error { "alert" } else { "status" },
            Glyph { icon, size: 16, class: "notice-icon" }
            div { class: "notice-text", {children} }
        }
    }
}

/// A spinner in a button that's busy.
#[component]
pub fn Busy(on: bool) -> Element {
    if !on {
        return rsx! {};
    }
    rsx! { Glyph { icon: Icon::Spinner, size: 14, class: "spin" } }
}

/// A choice in a [`Select`].
#[derive(Debug, Clone, PartialEq)]
pub struct Opt {
    pub value: String,
    pub label: String,
    pub hint: Option<String>,
    /// an agent's mark before the label
    pub mark: Option<inyon_protocol::AgentKind>,
}

impl Opt {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Opt {
        Opt { value: value.into(), label: label.into(), hint: None, mark: None }
    }

    pub fn hint(mut self, h: Option<String>) -> Opt {
        self.hint = h.filter(|h| !h.is_empty());
        self
    }
}

/// Where an anchored list opens: under the field, or over it when there's more room above;
/// `(top or bottom, max height)`.
pub fn place_list(field_top: f64, field_bottom: f64, wanted: f64, screen: f64) -> (bool, f64) {
    let below = (screen - 8.0 - field_bottom - 6.0).max(0.0);
    let above = (field_top - 6.0 - 8.0).max(0.0);
    let up = below < wanted.min(320.0) && above > below;
    (up, (if up { above } else { below }).min(320.0))
}

/// A select that looks like the rest of the app: a list under the field with a mouse, a sheet from
/// the bottom on a phone. ↑/↓, Home/End and Enter pick; Esc closes only the list.
#[component]
pub fn Select(value: String, options: Vec<Opt>, title: String, on_change: EventHandler<String>, #[props(default)] disabled: bool) -> Element {
    let mut open = use_signal(|| false);
    let mut sheet = use_signal(|| false);
    let mut active = use_signal(|| 0usize);
    let mut pos = use_signal(String::new);
    let id = use_hook(|| format!("select-{}", dom::random_id()));
    let current = options.iter().find(|o| o.value == value).cloned();
    let n = options.len();
    let list_id = format!("{id}-list");
    let show = {
        let id = id.clone();
        let list_id = list_id.clone();
        let options = options.clone();
        let value = value.clone();
        move |_| {
            let (w, h) = dom::viewport();
            let phone = dom::coarse() || w < 640.0;
            sheet.set(phone);
            active.set(options.iter().position(|o| o.value == value).unwrap_or(0));
            if !phone {
                if let Some((left, top, right, bottom)) = dom::rect(&id) {
                    let (up, max) = place_list(top, bottom, 12.0 + 40.0 * n as f64, h);
                    let edge = if up { format!("bottom:{}px", h - top + 6.0) } else { format!("top:{}px", bottom + 6.0) };
                    pos.set(format!("left:{left}px;width:{}px;{edge};max-height:{max}px", (right - left).max(180.0)));
                }
            }
            open.set(true);
            let list_id = list_id.clone();
            spawn(async move {
                platform::sleep(20).await;
                dom::focus(&list_id);
            });
        }
    };
    let field = id.clone();
    let close = move |refocus: bool| {
        open.set(false);
        if refocus {
            dom::focus(&field);
        }
    };
    let pick = {
        let value = value.clone();
        move |v: String| {
            if v != value {
                on_change.call(v);
            }
        }
    };
    let opts_for_keys = options.clone();
    let pick_k = pick.clone();
    let mut close_k = close.clone();
    let onkey = move |e: KeyboardEvent| {
        match e.key() {
            Key::Escape => {
                e.stop_propagation();
                close_k(true);
            }
            Key::Tab => close_k(false),
            Key::ArrowDown => active.set((active() + 1).min(n.saturating_sub(1))),
            Key::ArrowUp => active.set(active().saturating_sub(1)),
            Key::Home => active.set(0),
            Key::End => active.set(n.saturating_sub(1)),
            Key::Enter => {
                if let Some(o) = opts_for_keys.get(active()) {
                    pick_k(o.value.clone());
                }
                close_k(true);
            }
            Key::Character(c) if c == " " => {
                if let Some(o) = opts_for_keys.get(active()) {
                    pick_k(o.value.clone());
                }
                close_k(true);
            }
            _ => return,
        }
        e.prevent_default();
    };
    let items = rsx! {
        div { id: "{list_id}", class: "select-items", role: "listbox", "aria-label": "{title}", tabindex: "-1", onkeydown: onkey,
            for (i , o) in options.iter().cloned().enumerate() {
                {
                    let pick = pick.clone();
                    let mut close = close.clone();
                    let on = o.value == value;
                    rsx! {
                        button {
                            key: "{o.value}",
                            class: if i == active() { "select-item select-active" } else { "select-item" },
                            r#type: "button",
                            role: "option",
                            "aria-selected": "{on}",
                            onmouseenter: move |_| active.set(i),
                            onclick: move |_| {
                                pick(o.value.clone());
                                close(true);
                            },
                            if let Some(k) = o.mark {
                                super::icons::AgentMark { kind: k, size: 18 }
                            }
                            span { class: "select-words",
                                span { class: "select-label", "{o.label}" }
                                if let Some(h) = &o.hint {
                                    span { class: "select-hint", "{h}" }
                                }
                            }
                            if on {
                                Glyph { icon: Icon::Check, size: 16, class: "select-check" }
                            }
                        }
                    }
                }
            }
        }
    };
    rsx! {
        button {
            id: "{id}",
            class: "field select-field",
            r#type: "button",
            "aria-haspopup": "listbox",
            "aria-expanded": "{open()}",
            "aria-label": "{title}",
            disabled,
            onclick: show,
            if let Some(k) = current.as_ref().and_then(|c| c.mark) {
                super::icons::AgentMark { kind: k, size: 18 }
            }
            span { class: "select-value truncate", {current.as_ref().map(|c| c.label.clone()).unwrap_or_default()} }
            if let Some(h) = current.as_ref().and_then(|c| c.hint.clone()) {
                span { class: "select-value-hint truncate", "{h}" }
            }
            Glyph { icon: Icon::ChevronDown, size: 16, class: "fg-3" }
        }
        if sheet() {
            Sheet { open: open(), on_close: move |_| open.set(false), title: title.clone(), {items} }
        } else if open() {
            div { class: "select-backdrop", onclick: move |_| open.set(false) }
            div { class: "select-list", style: "{pos}", {items} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_opens_where_it_fits() {
        // room below: under the field
        assert_eq!(place_list(100.0, 140.0, 200.0, 844.0), (false, 320.0));
        // near the bottom: over it
        let (up, max) = place_list(700.0, 740.0, 200.0, 844.0);
        assert!(up);
        assert_eq!(max, 320.0);
        // little room either way: the bigger side, as tall as it allows
        assert_eq!(place_list(60.0, 100.0, 400.0, 260.0), (false, 146.0));
    }
}
