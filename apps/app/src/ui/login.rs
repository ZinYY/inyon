//! Sign-in, and the first password on a new hub (apps/web/src/pages/Login.tsx).

use dioxus::prelude::*;

use super::icons::{Glyph, Icon, Logo};
use crate::api;

#[component]
pub fn Login(setup: bool, needs_code: bool, on_done: EventHandler<()>) -> Element {
    let mut password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut setup_code = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let submit = move |e: FormEvent| {
        e.prevent_default();
        if busy() {
            return;
        }
        spawn(async move {
            busy.set(true);
            error.set(String::new());
            let pw = password();
            let result = if setup {
                match api::check_new_password(&pw, &confirm()) {
                    Err(msg) => Err(msg),
                    Ok(()) => {
                        let c = setup_code();
                        api::create_password(&pw, needs_code.then_some(c.trim())).await.map_err(|e| e.message)
                    }
                }
            } else {
                api::login(&pw).await.map_err(|e| e.message)
            };
            busy.set(false);
            match result {
                Ok(()) => on_done.call(()),
                Err(msg) => error.set(msg),
            }
        });
    };

    let incomplete = password().is_empty() || (setup && confirm().is_empty()) || (needs_code && setup_code().trim().is_empty());
    rsx! {
        div { class: "login",
            form { class: "login-form slide-up", onsubmit: submit,
                Logo { class: "logo-lg", target: true }
                h1 { class: "login-title", if setup { "Create your password" } else { "Welcome back" } }
                p { class: "login-lede",
                    if setup {
                        "This hub is new. Pick the password you'll use to sign in from your browsers."
                    } else {
                        "Sign in to reach your machines and coding agents."
                    }
                }
                input {
                    class: "field field-lg login-first",
                    r#type: "password",
                    autofocus: true,
                    autocomplete: if setup { "new-password" } else { "current-password" },
                    placeholder: if setup { "New password (8+ characters)" } else { "Password" },
                    "aria-label": if setup { "New password" } else { "Password" },
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                }
                if setup {
                    input {
                        class: "field field-lg login-next",
                        r#type: "password",
                        autocomplete: "new-password",
                        placeholder: "Type it again",
                        "aria-label": "Type the password again",
                        value: "{confirm}",
                        oninput: move |e| confirm.set(e.value()),
                    }
                }
                if needs_code {
                    label { class: "login-code",
                        span { class: "field-label", "Setup code" }
                        input {
                            class: "field field-lg mono",
                            autocapitalize: "none",
                            autocomplete: "off",
                            spellcheck: "false",
                            value: "{setup_code}",
                            oninput: move |e| setup_code.set(e.value()),
                        }
                        span { class: "field-hint",
                            "The hub printed it when it started. On the hub machine, "
                            code { "inyon hub code" }
                            " shows it again."
                        }
                    }
                }
                if !error().is_empty() {
                    p { class: "login-error", role: "alert", "{error}" }
                }
                button { class: "button button-primary button-lg login-submit press", r#type: "submit", disabled: incomplete || busy(),
                    if busy() {
                        Glyph { icon: Icon::Spinner, size: 14, class: "spin" }
                    }
                    if setup { "Create password" } else { "Continue" }
                }
            }
        }
    }
}
