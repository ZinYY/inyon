//! An agent's question or approval as a card in place of the message box (PromptCard.tsx): its
//! title and text, the framed command or diff, and each choice as a button. Focus starts on the
//! likely choice; ↑/↓ move, Enter or a choice's number picks, Esc cancels. The answer carries the
//! key of the question it was given for, which the hub checks before it presses anything.

use dioxus::prelude::*;
use inyon_protocol::{AgentKind, ScreenPrompt, CONFIRM_OPTION};

use super::icons::{AgentMark, Glyph, Icon};
use crate::prompt::{self as rules, Block, DiffLine, KeyAction};
use crate::{api, dom, platform};

const FREE: &str = "prompt-free";
const CARD: &str = "prompt-card";

/// Focus on `id` once the screen has drawn it: the choices are disabled while an answer is on its
/// way, and a disabled button can't take focus, so it tries again until it lands (a slow phone can
/// take a few frames). With `yield_to_keys`, it gives up as soon as anything other than the card
/// itself has focus, so it never undoes an arrow key pressed in the meantime.
fn focus_soon(id: String, yield_to_keys: bool) {
    spawn(async move {
        for _ in 0..60 {
            platform::sleep(16).await;
            let at = dom::active_id();
            if at.as_deref() == Some(id.as_str()) || (yield_to_keys && at.is_some_and(|a| a != CARD)) {
                return;
            }
            dom::focus(&id);
        }
    });
}

fn option_id(i: usize) -> String {
    format!("prompt-option-{i}")
}

#[component]
pub fn PromptCard(prompt: ScreenPrompt, host: String, pane: String, disabled: bool, agent: Option<AgentKind>) -> Element {
    let mut busy = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut free = use_signal(|| None::<String>);
    let mut focused = use_signal(|| rules::likely(&prompt));
    let key = rules::key_of(&prompt);
    // a new question: nothing pressed yet, and focus on its likely choice
    {
        let likely = rules::likely(&prompt);
        use_effect(use_reactive!(|key| {
            let _ = &key;
            busy.set(None);
            error.set(None);
            free.set(None);
            focused.set(likely);
            // the card holds focus at once, so keys pressed before a choice can take it still
            // reach it, and the likely choice takes it as soon as it can
            dom::focus(CARD);
            focus_soon(option_id(likely), true);
        }));
    }
    let locked = busy().is_some() || disabled;
    let (title, blocks) = rules::parse_body(&prompt.body);
    let question = rules::is_question(&prompt, &title);

    let run = {
        let (host, pane) = (host.clone(), pane.clone());
        move |id: String, call: Call| {
            busy.set(Some(id));
            error.set(None);
            let (host, pane) = (host.clone(), pane.clone());
            spawn(async move {
                let result = match &call {
                    Call::Answer { key, n, text } => api::answer(&host, &pane, &rules::answer(key, *n, text.as_deref())).await,
                    Call::Keys(k) => api::keys(&host, &pane, &k.iter().map(String::as_str).collect::<Vec<_>>()).await,
                    Call::Input(t) => api::input(&host, &pane, t).await,
                };
                match result {
                    // the screen replaces this card once the agent moves on
                    Ok(()) => {
                        platform::sleep(2500).await;
                        busy.set(None);
                    }
                    Err(e) => {
                        error.set(Some(e.message));
                        busy.set(None);
                    }
                }
            });
        }
    };
    let free_option = prompt.free_text_option;
    let pick = {
        let (mut run, key) = (run.clone(), key.clone());
        move |n: u64| {
            if Some(n) == free_option {
                free.set(Some(String::new()));
                focus_soon(FREE.to_string(), false);
                return;
            }
            run(n.to_string(), Call::Answer { key: key.clone(), n, text: None });
        }
    };
    let on_key = {
        let (prompt, mut pick, mut run) = (prompt.clone(), pick.clone(), run.clone());
        move |e: KeyboardEvent| {
            // typing an answer: the keys are the text's
            if free.peek().is_some() || prompt.input.is_some() || locked {
                return;
            }
            let k = e.key().to_string();
            let at = *focused.peek();
            match rules::key_action(&prompt, &k, at) {
                KeyAction::Pick(n) => pick(n),
                KeyAction::Focus(i) => {
                    focused.set(i);
                    dom::focus(&option_id(i));
                }
                KeyAction::Cancel => run("cancel".into(), Call::Keys(vec!["Escape".into()])),
                KeyAction::None => return,
            }
            e.prevent_default();
        }
    };

    let tabs = prompt.tabs.clone().filter(|t| t.len() > 1).unwrap_or_default();
    let whole = prompt.whole_lines == Some(true);
    rsx! {
        div { id: CARD, class: "prompt slide-up", role: "group", tabindex: "-1", "aria-label": "{title}", onkeydown: on_key,
            if !tabs.is_empty() {
                div { class: "prompt-tabs", role: "tablist",
                    for t in tabs.iter() {
                        span {
                            key: "{t.label}",
                            role: "tab",
                            "aria-selected": "{t.current}",
                            class: if t.current { "prompt-tab prompt-tab-on" } else if t.done { "prompt-tab prompt-tab-done" } else { "prompt-tab" },
                            if t.done && !t.current {
                                Glyph { icon: Icon::Check, size: 12 }
                            }
                            "{t.label}"
                        }
                    }
                }
            }
            div { class: "prompt-scroll",
                div { class: "prompt-head",
                    span { class: "prompt-mark",
                        if let Some(kind) = agent {
                            AgentMark { kind, size: 19 }
                        }
                    }
                    div { class: "prompt-words",
                        div { class: if question { "prompt-title prompt-title-q" } else { "prompt-title" }, "{title}" }
                        for (i , b) in blocks.iter().enumerate() {
                            match b {
                                Block::Code(lines) => rsx! { CodeLines { key: "{i}", lines: rules::code_lines(lines) } },
                                Block::Text(lines) => rsx! {
                                    div { key: "{i}", class: "prompt-text",
                                        if whole {
                                            {lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n").trim().to_string()}
                                        } else {
                                            {rules::join_wrapped(lines)}
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
                if let Some(links) = prompt.links.as_ref().filter(|l| !l.is_empty()) {
                    div { class: "prompt-links",
                        for u in links.iter() {
                            div { key: "{u}", class: "prompt-link",
                                div { class: "prompt-link-url mono", "{u}" }
                                a { class: "button button-sm button-primary", href: "{u}", target: "_blank", rel: "noreferrer",
                                    Glyph { icon: Icon::ExternalLink, size: 14 }
                                    "Open"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(input) = prompt.input.clone() {
                InputForm { label: input.label, locked, on_submit: { let mut run = run.clone(); move |v: String| run("input".into(), Call::Input(v)) } }
            } else if let Some(t) = free() {
                form {
                    class: "prompt-form",
                    onsubmit: {
                        let (mut run, key) = (run.clone(), key.clone());
                        move |e: FormEvent| {
                            e.prevent_default();
                            let v = free.peek().clone().unwrap_or_default();
                            if let (Some(n), false) = (free_option, v.trim().is_empty()) {
                                run("free".into(), Call::Answer { key: key.clone(), n, text: Some(v) });
                            }
                        }
                    },
                    textarea {
                        id: FREE,
                        class: "prompt-free",
                        rows: "2",
                        value: "{t}",
                        placeholder: "Type your answer…",
                        "aria-label": "Your answer",
                        oninput: move |e| free.set(Some(e.value())),
                    }
                    button { class: "round-button press", r#type: "submit", disabled: t.trim().is_empty() || locked, "aria-label": "Send answer",
                        Glyph { icon: Icon::Send, size: 17 }
                    }
                }
            } else {
                div { class: "prompt-options", role: "listbox", "aria-label": "Choices",
                    for (i , o) in prompt.options.iter().enumerate() {
                        button {
                            key: "{o.n}",
                            id: option_id(i),
                            r#type: "button",
                            role: "option",
                            "aria-selected": "{o.selected}",
                            tabindex: if i == focused() { "0" } else { "-1" },
                            disabled: locked,
                            class: {
                                let mut c = String::from("option press");
                                if o.selected {
                                    c.push_str(" option-selected");
                                }
                                if busy().as_deref() == Some(o.n.to_string().as_str()) {
                                    c.push_str(" option-busy");
                                }
                                c
                            },
                            onfocus: move |_| focused.set(i),
                            onclick: {
                                let mut pick = pick.clone();
                                let n = o.n;
                                move |_| pick(n)
                            },
                            if let Some(checked) = o.checked {
                                span { class: if checked { "option-check option-check-on" } else { "option-check" },
                                    if checked {
                                        Glyph { icon: Icon::Check, size: 14 }
                                    }
                                }
                            } else {
                                span { class: "option-n mono",
                                    if prompt.navigate == Some(true) {
                                        span { class: "option-dot" }
                                    } else {
                                        "{o.n}"
                                    }
                                }
                            }
                            span { class: "option-words",
                                span { class: "option-label", "{o.label}" }
                                if let Some(d) = o.detail.as_ref() {
                                    span { class: "option-detail", "{d}" }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(e) = error() {
                div { class: "prompt-error", role: "alert", "Couldn't answer: {e}" }
            }
            div { class: "prompt-foot",
                button {
                    class: "button button-md button-ghost press",
                    r#type: "button",
                    disabled,
                    onclick: {
                        let mut run = run.clone();
                        move |_| if free.peek().is_some() { free.set(None) } else { run("cancel".into(), Call::Keys(vec!["Escape".into()])) }
                    },
                    if free().is_some() { "Back" } else { "Cancel" }
                }
                div { class: "prompt-foot-end",
                    if let Some(confirm) = prompt.confirm.clone() {
                        // the choices above toggle one by one; this sends them (OpenCode's questions)
                        button {
                            class: "button button-md button-primary press",
                            r#type: "button",
                            disabled: locked,
                            onclick: {
                                let (mut run, key) = (run.clone(), key.clone());
                                move |_| run("confirm".into(), Call::Answer { key: key.clone(), n: CONFIRM_OPTION as u64, text: None })
                            },
                            "{confirm}"
                            Glyph { icon: Icon::ArrowRight, size: 16 }
                        }
                    } else {
                        // raw keys only when the choices couldn't be read as buttons, or for multi-select steps
                        if prompt.options.is_empty() || prompt.multi {
                            for (k , icon) in [("Up", Icon::ArrowUp), ("Down", Icon::ArrowDown), ("Enter", Icon::CornerDownLeft)] {
                                button {
                                    key: "{k}",
                                    class: "icon-button icon-button-lg press",
                                    r#type: "button",
                                    title: k,
                                    "aria-label": k,
                                    disabled,
                                    onclick: {
                                        let mut run = run.clone();
                                        move |_| run(k.into(), Call::Keys(vec![k.into()]))
                                    },
                                    Glyph { icon, size: 16 }
                                }
                            }
                        }
                        if prompt.multi {
                            button {
                                class: "button button-md button-primary press",
                                r#type: "button",
                                disabled,
                                onclick: {
                                    let mut run = run.clone();
                                    move |_| run("Right".into(), Call::Keys(vec!["Right".into()]))
                                },
                                "Next"
                                Glyph { icon: Icon::ArrowRight, size: 16 }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// What a press on the card asks of the hub.
#[derive(Debug, Clone)]
enum Call {
    Answer { key: String, n: u64, text: Option<String> },
    Keys(Vec<String>),
    Input(String),
}

/// A command or a file's text, kept exactly as framed; an edit's diff in the diff colours.
#[component]
fn CodeLines(lines: Vec<String>) -> Element {
    let kinds = rules::diff_kinds(&lines);
    rsx! {
        pre { class: "prompt-code",
            if let Some(kinds) = kinds {
                for (i , l) in lines.iter().enumerate() {
                    span {
                        key: "{i}",
                        class: match kinds[i] {
                            DiffLine::Add => "diff-line diff-add",
                            DiffLine::Del => "diff-line diff-del",
                            DiffLine::Same => "diff-line",
                        },
                        if l.is_empty() { " " } else { "{l}" }
                    }
                }
            } else {
                {lines.join("\n")}
            }
        }
    }
}

/// A field the agent asked to have pasted in (a sign-in code).
#[component]
fn InputForm(label: String, locked: bool, on_submit: EventHandler<String>) -> Element {
    let mut value = use_signal(String::new);
    rsx! {
        form {
            class: "prompt-form",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                let v = value.peek().trim().to_string();
                if !v.is_empty() {
                    on_submit.call(v);
                }
            },
            label { class: "prompt-input",
                span { class: "field-label", "{label}" }
                input {
                    class: "field mono",
                    name: "answer",
                    autocomplete: "off",
                    autocapitalize: "off",
                    spellcheck: "false",
                    placeholder: "Paste here",
                    value: "{value}",
                    oninput: move |e| value.set(e.value()),
                }
            }
            button { class: "round-button press", r#type: "submit", disabled: locked, "aria-label": "Submit",
                Glyph { icon: Icon::Send, size: 16 }
            }
        }
    }
}
