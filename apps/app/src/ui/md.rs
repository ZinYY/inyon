//! A reply drawn from its Markdown tree (Markdown.tsx): prose in the reply font, code blocks with
//! their language and a copy button, links that open in a new tab, and only this site's images.

use dioxus::prelude::*;

use super::icons::{Glyph, Icon};
use super::menu::{self, Item};
use crate::dom;
use crate::markdown::{self, El, Md, Picture};

/// `font`: the reply font's class (`reply-serif`, `reply-mono`, or none for sans).
#[component]
pub fn Markdown(text: String, #[props(default)] class: String, #[props(default)] font: String) -> Element {
    let nodes = markdown::parse(&text);
    rsx! {
        div { class: "prose-reply {font} {class}", {nodes.iter().map(node)} }
    }
}

fn nodes(list: &[Md]) -> Element {
    rsx! { {list.iter().map(node)} }
}

fn node(n: &Md) -> Element {
    match n {
        Md::Text(t) => rsx! { "{t}" },
        Md::Code(t) => rsx! { code { "{t}" } },
        Md::Break => rsx! { br {} },
        Md::Rule => rsx! { hr {} },
        Md::CodeBlock { lang, code } => rsx! { CodeBlock { lang: lang.clone(), code: code.clone() } },
        Md::Image { src, alt } => match markdown::picture(src) {
            Picture::Show => rsx! { img { src: "{src}", alt: "{alt}", loading: "lazy" } },
            Picture::Link => rsx! { a { href: "{src}", target: "_blank", rel: "noreferrer", if alt.is_empty() { "{src}" } else { "{alt}" } } },
            Picture::Alt => rsx! { "{alt}" },
        },
        Md::Link { href, children } => match markdown::safe_href(href) {
            Some(h) => rsx! { a { href: "{h}", target: "_blank", rel: "noreferrer", {nodes(children)} } },
            None => nodes(children),
        },
        Md::El { tag, children } => {
            let c = nodes(children);
            match tag {
                El::P => rsx! { p { {c} } },
                El::H(1) => rsx! { h1 { {c} } },
                El::H(2) => rsx! { h2 { {c} } },
                El::H(3) => rsx! { h3 { {c} } },
                El::H(_) => rsx! { h4 { {c} } },
                El::Quote => rsx! { blockquote { {c} } },
                El::Ul => rsx! { ul { {c} } },
                El::Ol(start) => rsx! { ol { start: "{start}", {c} } },
                El::Li => rsx! { li { {c} } },
                El::Check(on) => rsx! { input { r#type: "checkbox", checked: *on, disabled: true, class: "task-box" } },
                El::Em => rsx! { em { {c} } },
                El::Strong => rsx! { strong { {c} } },
                El::Del => rsx! { del { {c} } },
                El::Table => rsx! { div { class: "table-wrap", table { {c} } } },
                El::Head => rsx! { thead { {c} } },
                El::Body => rsx! { tbody { {c} } },
                El::Tr => rsx! { tr { {c} } },
                El::Th => rsx! { th { {c} } },
                El::Td => rsx! { td { {c} } },
            }
        }
    }
}

/// A fenced block: the language over it, a copy button, and the text exactly as written (leading
/// spaces and tabs included).
#[component]
pub fn CodeBlock(lang: Option<String>, code: String) -> Element {
    let text = code.strip_suffix('\n').unwrap_or(&code).to_string();
    let copy = text.clone();
    let items = vec![Item::copy("Copy code", &text)];
    rsx! {
        div { class: "code-block", oncontextmenu: menu::open(items),
            div { class: "code-head",
                span { class: "code-lang mono", {lang.clone().unwrap_or_else(|| "text".into())} }
                CopyButton { text: copy, label: "Copy code" }
            }
            pre { class: "code-body", code { "{text}" } }
        }
    }
}

/// Copies `text`, and shows a tick for a moment.
#[component]
pub fn CopyButton(text: String, #[props(default = "Copy".to_string())] label: String, #[props(default)] class: String) -> Element {
    let mut done = use_signal(|| false);
    rsx! {
        button {
            class: "icon-button press {class}",
            r#type: "button",
            title: "{label}",
            "aria-label": "{label}",
            onclick: move |_| {
                let t = text.clone();
                spawn(async move {
                    if dom::copy(&t).await {
                        done.set(true);
                        crate::platform::sleep(1500).await;
                        done.set(false);
                    }
                });
            },
            if done() {
                Glyph { icon: Icon::Check, size: 14, class: "fade-in" }
            } else {
                Glyph { icon: Icon::Copy, size: 14 }
            }
        }
    }
}
