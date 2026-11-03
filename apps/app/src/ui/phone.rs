//! "Open on your phone" (components/PhoneSheet.tsx): a code the Inyon app scans, or types, to pair
//! with this hub. The hub only hands one out to someone who signed in a moment ago, so an older
//! tab asks for the password again first. The code works once, for ten minutes.

use dioxus::prelude::*;

use super::icons::{Glyph, Icon};
use super::kit::{Busy, Field, Sheet};
use super::toast::{self, ToastTone};
use crate::machines as m;
use crate::{api, dom, platform};

#[component]
pub fn PhoneSheet(open: bool, on_close: EventHandler<()>) -> Element {
    let mut code = use_signal(|| None::<api::PhoneCode>);
    let mut need_password = use_signal(|| false);
    let mut password = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let mut fetch = move || {
        error.set(None);
        busy.set(true);
        spawn(async move {
            match api::phone_pair(&dom::origin()).await {
                Ok(c) => {
                    code.set(Some(c));
                    need_password.set(false);
                }
                Err(e) if e.status == 403 => need_password.set(true),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    use_effect(use_reactive!(|open| {
        if open {
            code.set(None);
            password.set(String::new());
            fetch();
        }
    }));

    let address = dom::origin().trim_start_matches("https://").trim_start_matches("http://").to_string();
    rsx! {
        Sheet { open, on_close: move |_| on_close.call(()), title: "Open on your phone", icon: rsx! { Glyph { icon: Icon::Smartphone, size: 16 } },
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
                                    fetch();
                                }
                                Err(e) => {
                                    error.set(Some(e.message));
                                    busy.set(false);
                                }
                            }
                        });
                    },
                    p { class: "sheet-text", "Pairing a phone needs a fresh sign-in. Enter the hub's password once more." }
                    Field { label: "Password",
                        input {
                            class: "field",
                            r#type: "password",
                            autocomplete: "current-password",
                            "data-autofocus": "true",
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                        }
                    }
                    if let Some(e) = error() {
                        p { class: "form-error", "{e}" }
                    }
                    button { class: "button button-md button-primary press button-block", r#type: "submit", disabled: password().is_empty() || busy(),
                        Busy { on: busy() }
                        "Continue"
                    }
                }
            } else if let Some(c) = code() {
                {
                    let link = m::phone_qr_link(&c.link);
                    let about = m::parse_hub_link(&c.link);
                    let left = m::minutes_left(&c, platform::clock().now);
                    let copy_link = c.link.clone();
                    rsx! {
                        div { class: "stack",
                            p { class: "sheet-text",
                                "In the Inyon app, tap "
                                strong { "Add" }
                                ", then "
                                strong { "Connect to a hub" }
                                ", and scan this. The phone then reaches this hub and its machines itself, with no browser in between."
                            }
                            if let Some((size, d)) = m::qr_path(&link) {
                                div { class: "qr", role: "img", "aria-label": "Pairing code",
                                    svg { view_box: "0 0 {size} {size}", "shape-rendering": "crispEdges",
                                        path { d: "{d}", fill: "#000" }
                                    }
                                }
                            }
                            div { class: "phone-typed",
                                p { class: "phone-typed-lede", "Can't scan it? In the app, type the hub's address and this code." }
                                dl { class: "phone-typed-rows mono",
                                    dt { "Address" }
                                    dd { class: "tabular", "{address}" }
                                    dt { "ID" }
                                    dd { class: "tabular wide-1", {m::show_id(&c.id)} }
                                    dt { "Code" }
                                    dd { class: "tabular wide-2", {m::show_code(&c.code)} }
                                    if let Some(h) = about.as_ref().and_then(|a| a.hub_id.clone()) {
                                        dt { "Hub ID" }
                                        dd { class: "tabular", {m::groups_of(&h)} }
                                    }
                                    if let Some(f) = about.as_ref().and_then(|a| a.tls_fingerprint.clone()) {
                                        dt { "Certificate" }
                                        dd { class: "tabular", {m::groups_of(&f)} }
                                    }
                                }
                                if about.as_ref().is_some_and(|a| a.hub_id.is_some()) {
                                    p { class: "phone-typed-note", "The phone shows the hub ID before it pairs. Check they match." }
                                }
                            }
                            div { class: "phone-foot",
                                p { class: "phone-left", {format!("Works once, for {left} more minute{}. Five wrong codes cancel it.", if left == 1 { "" } else { "s" })} }
                                button {
                                    class: "button button-sm button-ghost press",
                                    r#type: "button",
                                    onclick: move |_| {
                                        let l = copy_link.clone();
                                        spawn(async move {
                                            if dom::copy(&l).await {
                                                toast::toast("Link copied", None, ToastTone::Success);
                                            }
                                        });
                                    },
                                    "Copy link"
                                }
                            }
                        }
                    }
                }
            } else if let Some(e) = error() {
                div { class: "stack",
                    p { class: "form-error", "{e}" }
                    button { class: "button button-md button-primary press", r#type: "button", disabled: busy(), onclick: move |_| fetch(),
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
