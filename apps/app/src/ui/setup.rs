//! A machine's Setup tab (components/SetupPanel.tsx), OpenCode's providers
//! (components/ProvidersSheet.tsx) and signing an agent in from here (components/SignInSheet.tsx):
//! the hub runs the agent's own sign-in out of sight, and the sheet shows its link and code, takes
//! the code the page hands back, and shows how it went.

use std::cell::Cell;
use std::rc::Rc;

use dioxus::prelude::*;
use inyon_protocol::{AgentKind, HostState, HostStatus, OpenCodeProvider, ProviderVia};

use super::icons::{AgentMark, Glyph, Icon};
use super::kit::{Busy, Notice, Sheet, Toggle, Tone, WaitRows};
use super::toast::{self, confirm, Ask, ToastTone};
use crate::machines as m;
use crate::route::{self, Route};
use crate::{api, dom, platform};

/// A step that failed, shown under the row it belongs to.
#[derive(Debug, Clone, PartialEq)]
struct Failure {
    action: String,
    hint: Option<String>,
    output: Option<String>,
    manual: Option<String>,
    docs: Option<String>,
}

#[component]
fn CopyLine(cmd: String) -> Element {
    rsx! {
        div { class: "copy-line",
            code { class: "copy-line-text truncate mono", "{cmd}" }
            button {
                class: "icon-button press",
                r#type: "button",
                "aria-label": "Copy command",
                title: "Copy command",
                onclick: move |_| {
                    let c = cmd.clone();
                    spawn(async move { toast::copy(&c, "Command").await });
                },
                Glyph { icon: Icon::Copy, size: 14 }
            }
        }
    }
}

#[component]
fn FailureBlock(f: Failure) -> Element {
    let tail = f.output.as_deref().map(|o| {
        let lines: Vec<&str> = o.lines().collect();
        lines[lines.len().saturating_sub(12)..].join("\n").trim().to_string()
    });
    rsx! {
        div { class: "failure slide-up",
            p { class: "danger", {f.hint.clone().unwrap_or_else(|| "It didn't finish.".into())} }
            if let Some(t) = tail.filter(|t| !t.is_empty()) {
                pre { class: "failure-output mono", "{t}" }
            }
            if let Some(cmd) = f.manual.clone() {
                div {
                    p { class: "fg-3", "Or run it on the machine yourself:" }
                    CopyLine { cmd }
                }
            }
            if let Some(d) = &f.docs {
                a { class: "quiet-link", href: "{d}", target: "_blank", rel: "noreferrer",
                    "Install guide"
                    Glyph { icon: Icon::ExternalLink, size: 12 }
                }
            }
        }
    }
}

/// Everything a machine needs for Inyon, with one-tap fixes where it's safe to run them.
#[component]
pub fn SetupPanel(host: HostState) -> Element {
    let mut here = use_context::<Signal<Route>>();
    let mut report = use_signal(|| None::<api::SetupReport>);
    let mut busy = use_signal(|| None::<String>);
    let mut failure = use_signal(|| None::<Failure>);
    let mut sign_in = use_signal(|| None::<AgentKind>);
    let mut providers = use_signal(|| false);
    let id = host.config.id.clone();
    let online = host.status == HostStatus::Online;

    let load = {
        let id = id.clone();
        move || {
            let id = id.clone();
            spawn(async move {
                match api::setup(&id).await {
                    Ok(r) => report.set(Some(r)),
                    Err(e) => toast::toast("Could not check the machine", Some(e.message), ToastTone::Error),
                }
            });
        }
    };
    {
        let load = load.clone();
        use_effect(use_reactive!(|online| {
            if online {
                load();
            }
        }));
    }
    // runs a step; `done` is the toast when it worked, given the version the hub reported
    let run = {
        let id = id.clone();
        let load = load.clone();
        move |action: String, done: Rc<dyn Fn(Option<String>) -> String>, manual: Option<String>, docs: Option<String>| {
            let id = id.clone();
            let load = load.clone();
            busy.set(Some(action.clone()));
            failure.set(None);
            spawn(async move {
                match api::run_setup(&id, &action).await {
                    Ok(r) => {
                        if let Some(pane) = r.pane {
                            busy.set(None);
                            route::go(&mut here, Route::Session { host: id, pane, view: Some(inyon_chat_core::overview::PaneView::Terminal) });
                            return;
                        }
                        if r.ok {
                            toast::toast(done(r.version), None, ToastTone::Success);
                        } else {
                            failure.set(Some(Failure { action, hint: r.hint, output: r.output, manual: r.manual.or(manual), docs }));
                        }
                        load();
                    }
                    Err(e) => failure.set(Some(Failure { action, hint: Some(e.message), output: None, manual, docs })),
                }
                busy.set(None);
            });
        }
    };
    // an agent that stays in its terminal signs in there too: its own sign-in, in a new session
    let sign_in_terminal = {
        let id = id.clone();
        move |kind: AgentKind| {
            let id = id.clone();
            spawn(async move {
                let b = api::Launch { agent: kind.as_str().into(), sign_in: true, name: Some(format!("{}-sign-in", kind.as_str())), ..Default::default() };
                match api::launch(&id, &b).await {
                    Ok(s) => route::go(&mut here, Route::Session { host: id, pane: s.pane, view: Some(inyon_chat_core::overview::PaneView::Terminal) }),
                    Err(e) => toast::toast("Could not start the sign-in", Some(e.message), ToastTone::Error),
                }
            });
        }
    };

    if !online {
        return rsx! { p { class: "quiet-line", "Setup checks run once the machine is connected." } };
    }
    let Some(r) = report() else {
        return rsx! { WaitRows { label: "Checking {host.config.name}", class: "setup" } };
    };
    let reload = load.clone();
    let keep_id = id.clone();
    let keep_load = load.clone();
    let after_sign_in = load.clone();
    let after_providers = load;
    let about = format!("{}{}{}", r.os, r.pkg.as_ref().map(|p| format!(" · {p}")).unwrap_or_default(), if r.sudo { " · sudo available" } else { " · no passwordless sudo" });
    rsx! {
        div { class: "setup",
            div { class: "setup-about",
                span { "{about}" }
                button { class: "quiet-button press", r#type: "button", onclick: move |_| reload(),
                    Glyph { icon: Icon::RefreshCw, size: 14 }
                    "Re-check"
                }
            }
            section { class: "panel rows",
                for c in r.checks.iter().cloned() {
                    div { key: "{c.id}", class: "setup-row",
                        div { class: "setup-line",
                            span { class: if c.ok { "check-mark" } else { "check-mark check-mark-warn" },
                                if c.ok {
                                    Glyph { icon: Icon::Check, size: 12 }
                                } else {
                                    Glyph { icon: Icon::TriangleAlert, size: 12 }
                                }
                            }
                            div { class: "setup-words",
                                div { class: "setup-label", "{c.label}" }
                                if let Some(d) = &c.detail {
                                    div { class: "setup-detail truncate", "{d}" }
                                }
                            }
                            if let Some(fix) = c.fix.clone() {
                                {
                                    let mut run = run.clone();
                                    let manual = c.manual.clone();
                                    let on = busy().as_deref() == Some(fix.action.as_str());
                                    rsx! {
                                        button {
                                            class: "button button-sm button-primary press",
                                            r#type: "button",
                                            disabled: busy().is_some(),
                                            onclick: move |_| {
                                                let label = fix.label.clone();
                                                run(fix.action.clone(), Rc::new(move |_| format!("{label}: done")), manual.clone(), None)
                                            },
                                            Busy { on }
                                            "{fix.label}"
                                        }
                                    }
                                }
                            }
                        }
                        if let (Some(f), Some(fix)) = (failure(), c.fix.as_ref()) {
                            if f.action == fix.action {
                                div { class: "setup-indent", FailureBlock { f } }
                            }
                        }
                        if !c.ok && c.fix.is_none() {
                            if let Some(cmd) = c.manual.clone() {
                                div { class: "setup-indent",
                                    p { class: "setup-hint", "Run this on the machine (it needs your password):" }
                                    CopyLine { cmd }
                                }
                            }
                        }
                    }
                }
                div { class: "setup-row",
                    div { class: "setup-line",
                        div { class: "setup-words setup-indent-words",
                            div { class: "setup-label", "Keep a tmux session running" }
                            div { class: "setup-detail", "If every session closes, Inyon starts a fresh one called “main”." }
                        }
                        Toggle {
                            on: r.keep_session,
                            label: "Keep a tmux session running",
                            on_change: move |v: bool| {
                                let id = keep_id.clone();
                                let load = keep_load.clone();
                                spawn(async move {
                                    if let Err(e) = api::keep_session(&id, v).await {
                                        toast::toast("Could not change it", Some(e.message), ToastTone::Error);
                                    }
                                    load();
                                });
                            },
                        }
                    }
                }
            }
            section {
                h3 { class: "section-title", "Agents" }
                div { class: "panel rows",
                    for a in r.agents.iter().cloned() {
                        AgentRow {
                            key: "{a.kind.as_str()}",
                            a: a.clone(),
                            busy: busy(),
                            failure: failure(),
                            on_run: {
                                let run = run.clone();
                                move |(action, update): (String, bool)| {
                                    let mut run = run.clone();
                                    let def = a.kind.def();
                                    let name = def.name.clone();
                                    let done: Rc<dyn Fn(Option<String>) -> String> = if update {
                                        Rc::new(move |v: Option<String>| format!("{name} updated to {}", m::short_version(v.as_deref())))
                                    } else {
                                        Rc::new(move |v: Option<String>| format!("{name} {} installed", m::short_version(v.as_deref())))
                                    };
                                    run(action, done, None, Some(def.install.docs.clone()));
                                }
                            },
                            on_sign_in: {
                                let sign_in_terminal = sign_in_terminal.clone();
                                move |k: AgentKind| {
                                    if k.def().chat {
                                        sign_in.set(Some(k));
                                    } else {
                                        let f = sign_in_terminal.clone();
                                        f(k);
                                    }
                                }
                            },
                            on_providers: move |_| providers.set(true),
                        }
                    }
                }
                p { class: "fine-print",
                    "Install uses each agent's official installer; Update runs its own updater, or npm when it came from npm. Sign in runs the agent's sign-in on that machine and walks you through it here; for an agent that stays in its terminal, it opens the sign-in in a terminal session. Providers connects and disconnects OpenCode's model providers the same way."
                }
            }
            SignInSheet {
                host: id.clone(),
                host_name: host.config.name.clone(),
                agent: sign_in(),
                on_close: move |_| {
                    sign_in.set(None);
                    after_sign_in();
                },
            }
            ProvidersSheet {
                host: id.clone(),
                host_name: host.config.name.clone(),
                open: providers(),
                on_close: move |_| {
                    providers.set(false);
                    after_providers();
                },
            }
        }
    }
}

#[component]
fn AgentRow(
    a: api::SetupAgent,
    busy: Option<String>,
    failure: Option<Failure>,
    on_run: EventHandler<(String, bool)>,
    on_sign_in: EventHandler<AgentKind>,
    on_providers: EventHandler<()>,
) -> Element {
    let def = a.kind.def();
    let install = format!("agent:{}", a.kind.as_str());
    let update = format!("update:{}", a.kind.as_str());
    let working = if busy.as_deref() == Some(install.as_str()) {
        Some("Installing…")
    } else if busy.as_deref() == Some(update.as_str()) {
        Some("Updating…")
    } else {
        None
    };
    let state = working.map_or_else(|| m::setup_state(&a), str::to_string);
    let opencode = a.kind == AgentKind::Opencode;
    let connected = a.providers.as_ref().is_some_and(|p| !p.is_empty());
    let any = busy.is_some();
    let (i1, i2, u1, u2, u3) = (install.clone(), install.clone(), update.clone(), update.clone(), update.clone());
    rsx! {
        div { class: "setup-row",
            div { class: "setup-line setup-agent",
                AgentMark { kind: a.kind, size: 28 }
                div { class: "setup-words setup-agent-words",
                    div { class: "setup-label", "{def.name}" }
                    div { class: "setup-detail truncate", "{state}" }
                }
                div { class: "setup-buttons",
                    if !a.installed {
                        button { class: "button button-sm button-quiet press", r#type: "button", disabled: any, onclick: move |_| on_run.call((i1.clone(), false)),
                            Busy { on: busy.as_deref() == Some(i2.as_str()) }
                            Glyph { icon: Icon::Download, size: 14 }
                            "Install"
                        }
                    }
                    if a.installed && a.outdated {
                        button { class: "button button-sm button-quiet press", r#type: "button", disabled: any, onclick: move |_| on_run.call((u1.clone(), true)),
                            Busy { on: busy.as_deref() == Some(u2.as_str()) }
                            Glyph { icon: Icon::RefreshCw, size: 14 }
                            "Update to {m::short_version(a.latest.as_deref())}"
                        }
                    }
                    // no registry answer for this one: an update still runs its own updater
                    if a.installed && a.latest.is_none() {
                        button { class: "button button-sm button-ghost press", r#type: "button", disabled: any, onclick: move |_| on_run.call((u3.clone(), true)),
                            Glyph { icon: Icon::RefreshCw, size: 14 }
                            "Update"
                        }
                    }
                    if a.installed && opencode {
                        button {
                            class: if connected { "button button-sm button-quiet press" } else { "button button-sm button-primary press" },
                            r#type: "button",
                            onclick: move |_| on_providers.call(()),
                            Glyph { icon: Icon::Plug, size: 14 }
                            "Providers"
                        }
                    }
                    if a.installed && !opencode && !a.signed_in {
                        button { class: "button button-sm button-primary press", r#type: "button", onclick: move |_| on_sign_in.call(a.kind),
                            Glyph { icon: Icon::LogIn, size: 14 }
                            "Sign in"
                        }
                    }
                    a { class: "icon-button icon-button-lg press", href: "{def.install.docs}", target: "_blank", rel: "noreferrer", "aria-label": "{def.name} docs", title: "{def.name} docs",
                        Glyph { icon: Icon::ExternalLink, size: 14 }
                    }
                }
            }
            if let Some(f) = failure.filter(|f| f.action == install || f.action == update) {
                div { class: "setup-indent-agent", FailureBlock { f } }
            }
        }
    }
}

/// How many providers to connect the list shows before it asks for a narrower search.
const SHOWN: usize = 12;

/// OpenCode's model providers on one machine: the connected ones with Disconnect, and the rest to
/// connect through OpenCode's own sign-in.
#[component]
pub fn ProvidersSheet(host: String, host_name: String, open: bool, on_close: EventHandler<()>) -> Element {
    rsx! {
        Sheet { open, on_close: move |_| on_close.call(()), title: "OpenCode providers", icon: rsx! { AgentMark { kind: AgentKind::Opencode, size: 24 } },
            if open {
                Providers { host, host_name }
            }
        }
    }
}

#[component]
fn Providers(host: String, host_name: String) -> Element {
    let mut report = use_signal(|| None::<inyon_protocol::ProvidersReport>);
    let mut failed = use_signal(|| None::<String>);
    let mut query = use_signal(String::new);
    let mut busy = use_signal(|| None::<String>);
    let mut connecting = use_signal(|| None::<OpenCodeProvider>);
    let load = {
        let host = host.clone();
        move || {
            let host = host.clone();
            failed.set(None);
            spawn(async move {
                match api::providers(&host, None).await {
                    Ok(r) => report.set(Some(r)),
                    Err(e) => failed.set(Some(e.message)),
                }
            });
        }
    };
    use_hook(load.clone());
    let error = failed().or_else(|| report().and_then(|r| r.error));
    if let Some(e) = error {
        let mut again = load.clone();
        return rsx! {
            div { class: "stack",
                Notice { tone: Tone::Error, "{e}" }
                div { class: "form-end",
                    button { class: "button button-md button-primary press", r#type: "button", onclick: move |_| again(), "Try again" }
                }
            }
        };
    }
    let Some(r) = report() else {
        return rsx! { WaitRows { label: "Reading providers on {host_name}" } };
    };
    let connected: Vec<OpenCodeProvider> = r.providers.iter().filter(|p| p.connected).cloned().collect();
    let q = query();
    let others: Vec<OpenCodeProvider> = m::providers_to_connect(&r.providers, &q).into_iter().cloned().collect();
    let credentials: Vec<OpenCodeProvider> = connected.iter().filter(|p| p.via == Some(ProviderVia::Credential)).cloned().collect();
    let first = others.first().cloned();
    let more = others.len().saturating_sub(SHOWN);
    let mut after_connect = load.clone();
    let out_load = load.clone();
    let name_all = host_name.clone();
    let host_all = host.clone();
    rsx! {
        div { class: "stack stack-lg",
            section {
                div { class: "field-label", "Connected" }
                if connected.is_empty() {
                    p { class: "quiet-line", "No provider connected on {host_name}." }
                } else {
                    div { class: "panel rows",
                        for p in connected.iter().cloned() {
                            {
                                let load = load.clone();
                                let host = host.clone();
                                let host_name = host_name.clone();
                                let credential = p.via == Some(ProviderVia::Credential);
                                let on = busy().as_deref() == Some(p.id.as_str());
                                let how = m::provider_how(&p);
                                rsx! {
                                    div { key: "{p.id}", class: "provider-row",
                                        span { class: "check-mark", Glyph { icon: Icon::Check, size: 12 } }
                                        div { class: "setup-words",
                                            div { class: "setup-label truncate", "{p.name}" }
                                            div { class: "setup-detail truncate mono", "{how}" }
                                        }
                                        if credential {
                                            button {
                                                class: "button button-sm button-ghost press",
                                                r#type: "button",
                                                disabled: busy().is_some(),
                                                onclick: move |_| {
                                                    let p = p.clone();
                                                    let host = host.clone();
                                                    let host_name = host_name.clone();
                                                    let mut load = load.clone();
                                                    spawn(async move {
                                                        let ask = Ask {
                                                            title: format!("Disconnect {}?", p.name),
                                                            body: Some(format!("OpenCode on {host_name} forgets its credential. Its models leave the list.")),
                                                            yes: "Disconnect".into(),
                                                            danger: true,
                                                            ..Ask::default()
                                                        };
                                                        if !confirm(ask).await {
                                                            return;
                                                        }
                                                        busy.set(Some(p.id.clone()));
                                                        match api::disconnect_provider(&host, &p.id).await {
                                                            Ok(()) => {
                                                                toast::toast(format!("{} disconnected", p.name), None, ToastTone::Success);
                                                                load();
                                                            }
                                                            Err(e) => toast::toast("Could not disconnect", Some(e.message), ToastTone::Error),
                                                        }
                                                        busy.set(None);
                                                    });
                                                },
                                                Busy { on }
                                                "Disconnect"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            section {
                div { class: "field-label", "Connect a provider" }
                label { class: "search",
                    Glyph { icon: Icon::Search, size: 16, class: "fg-3" }
                    input {
                        class: "search-input",
                        value: "{q}",
                        placeholder: "Search providers",
                        "aria-label": "Search providers",
                        autocapitalize: "off",
                        autocorrect: "off",
                        spellcheck: "false",
                        oninput: move |e| query.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                if let Some(p) = first.clone() {
                                    e.prevent_default();
                                    connecting.set(Some(p));
                                }
                            }
                        },
                    }
                }
                div { class: "panel rows provider-list", role: "listbox", "aria-label": "Providers to connect",
                    for p in others.iter().take(SHOWN).cloned() {
                        button {
                            key: "{p.id}",
                            class: "provider-row provider-pick",
                            r#type: "button",
                            role: "option",
                            "aria-selected": "false",
                            onclick: { let p = p.clone(); move |_| connecting.set(Some(p.clone())) },
                            Glyph { icon: Icon::Plug, size: 16, class: "fg-3" }
                            span { class: "setup-words",
                                span { class: "setup-label truncate", "{p.name}" }
                                if let Some(env) = p.env.as_ref().and_then(|e| e.first()) {
                                    span { class: "setup-detail truncate mono", "or set {env}" }
                                }
                            }
                        }
                    }
                    if others.is_empty() {
                        div { class: "provider-none", "No provider matches “{q}”." }
                    }
                }
                if more > 0 {
                    p { class: "fine-print", "{more} more. Keep typing to narrow the list." }
                }
            }
            p { class: "fine-print",
                "A key you type goes to "
                span { class: "mono", "opencode auth login" }
                " on {host_name}, nowhere else. Sessions Inyon started see a change at once; an "
                span { class: "mono", "opencode" }
                " already running on its own sees it after a restart."
            }
            if !credentials.is_empty() {
                div { class: "form-end",
                    button {
                        class: "button button-sm button-danger press",
                        r#type: "button",
                        disabled: busy().is_some(),
                        onclick: move |_| {
                            let list = credentials.clone();
                            let host = host_all.clone();
                            let name = name_all.clone();
                            let mut load = out_load.clone();
                            spawn(async move {
                                let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
                                let ask = Ask {
                                    title: "Sign out of every provider?".into(),
                                    body: Some(format!("{}: OpenCode on {name} forgets their credentials. Providers set through the environment stay.", names.join(", "))),
                                    yes: "Sign out".into(),
                                    danger: true,
                                    ..Ask::default()
                                };
                                if !confirm(ask).await {
                                    return;
                                }
                                busy.set(Some("all".into()));
                                let mut err = None;
                                for p in &list {
                                    if let Err(e) = api::disconnect_provider(&host, &p.id).await {
                                        err = Some(e.message);
                                        break;
                                    }
                                }
                                match err {
                                    None => toast::toast(
                                        "Signed out of OpenCode",
                                        Some(format!("{} disconnected", crate::format::plural(list.len(), "provider"))),
                                        ToastTone::Success,
                                    ),
                                    Some(e) => toast::toast("Could not sign out", Some(e), ToastTone::Error),
                                }
                                load();
                                busy.set(None);
                            });
                        },
                        Busy { on: busy().as_deref() == Some("all") }
                        "Sign out of OpenCode"
                    }
                }
            }
            SignInSheet {
                host: host.clone(),
                host_name: host_name.clone(),
                agent: connecting().map(|_| AgentKind::Opencode),
                provider: connecting().map(|p| (p.id, p.name)),
                on_close: move |_| {
                    connecting.set(None);
                    query.set(String::new());
                    after_connect();
                },
            }
        }
    }
}

/// Signs an agent in on a machine from here. With `provider` (id, name), connects that OpenCode
/// provider the same way (`opencode auth login -p`).
#[component]
pub fn SignInSheet(host: String, host_name: String, agent: Option<AgentKind>, #[props(default)] provider: Option<(String, String)>, on_close: EventHandler<()>) -> Element {
    let mut run = use_signal(|| 0u32);
    let title = match (&agent, &provider) {
        (Some(_), Some((_, name))) => format!("Connect {name}"),
        (Some(k), None) => format!("Sign in to {}", k.def().name),
        _ => String::new(),
    };
    let icon = agent.map(|k| rsx! { AgentMark { kind: k, size: 24 } });
    rsx! {
        Sheet { open: agent.is_some(), on_close: move |_| on_close.call(()), title, icon, wide: true,
            if let Some(k) = agent {
                SignInFlow {
                    key: "{k.as_str()}:{provider.as_ref().map_or(\"\", |p| p.0.as_str())}:{run}",
                    host: host.clone(),
                    host_name: host_name.clone(),
                    agent: k,
                    provider: provider.clone(),
                    on_close: move |_| on_close.call(()),
                    on_retry: move |_| run += 1,
                }
            }
        }
    }
}

const LINK_ID: &str = "signin-link";
const CODE_ID: &str = "signin-code";

#[component]
fn SignInFlow(host: String, host_name: String, agent: AgentKind, provider: Option<(String, String)>, on_close: EventHandler<()>, on_retry: EventHandler<()>) -> Element {
    let mut pane = use_signal(|| None::<String>);
    let mut screen = use_signal(|| None::<api::SignInScreen>);
    let mut error = use_signal(|| None::<String>);
    let mut answer = use_signal(String::new);
    // the code went in; waiting for the agent to take it
    let mut sent = use_signal(|| 0u32);
    let mut opened = use_signal(|| false);
    let name = provider.as_ref().map_or_else(|| agent.def().name.clone(), |p| p.1.clone());
    let alive = use_hook(|| Rc::new(Cell::new(true)));
    // start the sign-in and follow its screen; closing the sheet ends it
    {
        let host = host.clone();
        let provider = provider.clone();
        let name = name.clone();
        let host_name = host_name.clone();
        let alive = alive.clone();
        use_hook(move || {
            spawn(async move {
                let started = match &provider {
                    Some((id, _)) => api::connect_provider(&host, id).await,
                    None => api::sign_in(&host, agent).await,
                };
                let p = match started {
                    Ok(s) => s.pane,
                    Err(e) => return error.set(Some(e.message)),
                };
                if !alive.get() {
                    let _ = api::sign_in_stop(&host, &p).await;
                    return;
                }
                pane.set(Some(p.clone()));
                loop {
                    match api::sign_in_screen(&host, &p).await {
                        Ok(s) => {
                            let exit = s.exit;
                            screen.set(Some(s));
                            if let Some(code) = exit {
                                if code == 0 {
                                    let what = if provider.is_some() { "connected" } else { "signed in" };
                                    toast::toast(format!("{name} is {what}"), Some(host_name.clone()), ToastTone::Success);
                                }
                                break;
                            }
                        }
                        Err(e) => {
                            error.set(Some(e.message));
                            break;
                        }
                    }
                    platform::sleep(1000).await;
                }
            });
        });
    }
    {
        let host = host.clone();
        let alive = alive.clone();
        use_drop(move || {
            alive.set(false);
            let done = screen.peek().as_ref().is_some_and(|s| s.exit.is_some());
            if let (Some(p), false) = (pane.peek().clone(), done) {
                dioxus::core::spawn_forever(async move {
                    let _ = api::sign_in_stop(&host, &p).await;
                });
            }
        });
    }
    let send = {
        let host = host.clone();
        move |b: api::SignInSend| {
            let Some(p) = pane() else { return };
            let host = host.clone();
            spawn(async move {
                let text = b.text.is_some();
                match api::sign_in_send(&host, &p, &b).await {
                    Ok(()) => {
                        if text {
                            answer.set(String::new());
                            // a code that didn't take: the agent asks again, so let it be pasted again
                            let n = *sent.peek() + 1;
                            sent.set(n);
                            platform::sleep(20_000).await;
                            if *sent.peek() == n {
                                sent.set(0);
                            }
                        }
                    }
                    Err(e) => toast::toast("Could not send", Some(e.message), ToastTone::Error),
                }
            });
        }
    };
    // keyboard first: the link button when it appears
    let has_link = screen().is_some_and(|s| !s.links.is_empty() && s.menu.is_none());
    use_effect(use_reactive!(|has_link| {
        if has_link && !*opened.peek() {
            dom::focus(LINK_ID);
        }
    }));

    if let Some(e) = error() {
        return rsx! {
            div { class: "stack",
                Notice { tone: Tone::Error, "{e}" }
                div { class: "sheet-actions",
                    button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| on_close.call(()), "Close" }
                    button { class: "button button-md button-primary press", r#type: "button", onclick: move |_| on_retry.call(()), "Try again" }
                }
            }
        };
    }
    let on_text = if host_name.is_empty() { String::new() } else { format!(" on {host_name}") };
    let Some(s) = screen() else {
        return rsx! { WaitRows { label: "Starting {name}'s sign-in{on_text}", count: 2 } };
    };
    let done = s.exit.is_some();
    let link = s.links.first().cloned();
    let more: Vec<String> = s.links.iter().skip(1).cloned().collect();
    let menu = if done { None } else { s.menu.clone() };
    let ask_code = !done && menu.is_none() && s.code.is_none() && (link.is_some() || s.input.is_some());
    let waiting = sent() > 0;
    let connect = provider.is_some();
    let send_menu = send.clone();
    let send_up = send.clone();
    let send_down = send.clone();
    let send_code = send;
    let details = s.lines.join("\n");
    let menu_key = menu.as_ref().map(|x| x.options.iter().map(|o| o.label.clone()).collect::<Vec<_>>().join("|")).unwrap_or_default();
    rsx! {
        div { class: "stack",
            if done {
                if s.exit == Some(0) {
                    Notice { tone: Tone::Ok, {format!("{name} is {}{on_text}.", if connect { "connected" } else { "signed in" })} }
                } else {
                    Notice { tone: Tone::Error,
                        {s.error.clone().unwrap_or_else(|| if connect { "The connection ended without connecting. The details below say why.".to_string() } else { "The sign-in ended without signing in. The details below say why.".to_string() })}
                    }
                }
            } else if menu.is_some() {
            } else if waiting {
                div { class: "signin-wait",
                    span { class: "host-dot host-dot-connecting" }
                    "Checking the code with {name}…"
                }
            } else if s.input.is_some() && link.is_none() {
            } else {
                Steps { code: s.code.is_some(), link: link.is_some(), name: name.clone() }
            }
            if let Some(mn) = menu.clone() {
                MenuStep { key: "{menu_key}", menu: mn, on_pick: move |n| send_menu(api::SignInSend { option: Some(n), ..Default::default() }) }
            }
            if agent == AgentKind::Opencode && menu.as_ref().and_then(|x| x.question.as_deref()) == Some("Select provider") {
                div { class: "button-row",
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| send_up(api::SignInSend { keys: Some(vec!["Up".into(); 7]), ..Default::default() }), "Previous providers" }
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| send_down(api::SignInSend { keys: Some(vec!["Down".into(); 7]), ..Default::default() }), "More providers" }
                }
            }
            if let Some(code) = s.code.clone().filter(|_| !done && menu.is_none()) {
                div { class: "signin-code",
                    div {
                        div { class: "signin-code-label", "Your code" }
                        div { class: "signin-code-value mono tabular", "{code}" }
                    }
                    button {
                        class: "button button-sm button-quiet press",
                        r#type: "button",
                        onclick: move |_| {
                            let c = code.clone();
                            spawn(async move { toast::copy(&c, "Code").await });
                        },
                        Glyph { icon: Icon::Copy, size: 14 }
                        "Copy"
                    }
                }
            }
            if let Some(l) = link.clone().filter(|_| !done && menu.is_none() && !waiting) {
                div { class: "signin-link-row",
                    a {
                        id: LINK_ID,
                        class: "button button-lg button-primary press signin-open",
                        href: "{l}",
                        target: "_blank",
                        rel: "noreferrer",
                        onclick: move |_| {
                            opened.set(true);
                            // the code box is ready when you come back from the page
                            dom::focus(CODE_ID);
                        },
                        Glyph { icon: Icon::ExternalLink, size: 16 }
                        "Open the sign-in page"
                    }
                    button {
                        class: "icon-button icon-button-lg button-quiet press",
                        r#type: "button",
                        "aria-label": "Copy link",
                        title: "Copy link",
                        onclick: move |_| {
                            let l = l.clone();
                            spawn(async move { toast::copy(&l, "Link").await });
                        },
                        Glyph { icon: Icon::Copy, size: 16 }
                    }
                }
            }
            if !done && menu.is_none() && !more.is_empty() {
                div { class: "signin-more",
                    for l in more {
                        a { key: "{l}", class: "truncate", href: "{l}", target: "_blank", rel: "noreferrer", "{l}" }
                    }
                }
            }
            if ask_code && !waiting {
                form {
                    class: "signin-form",
                    onsubmit: move |e| {
                        e.prevent_default();
                        let a = answer();
                        if !a.trim().is_empty() {
                            send_code(api::SignInSend { text: Some(a), ..Default::default() });
                        }
                    },
                    label { r#for: CODE_ID, class: "field-label", {s.input.as_ref().map_or("Code from the sign-in page".to_string(), |i| i.label.clone())} }
                    div { class: "signin-form-row",
                        input {
                            id: CODE_ID,
                            class: "field mono-field",
                            r#type: if s.input.as_ref().is_some_and(|i| i.secret) { "password" } else { "text" },
                            value: "{answer}",
                            placeholder: s.input.as_ref().map_or("Paste the code".to_string(), |i| i.label.clone()),
                            autocapitalize: "off",
                            autocorrect: "off",
                            autocomplete: "off",
                            spellcheck: "false",
                            oninput: move |e| answer.set(e.value()),
                        }
                        button { class: "button button-md button-primary press", r#type: "submit", disabled: answer().trim().is_empty(),
                            if connect { "Connect" } else { "Sign in" }
                        }
                    }
                }
            }
            details { class: "signin-details",
                summary {
                    Glyph { icon: Icon::ChevronRight, size: 14, class: "signin-details-chevron" }
                    {format!("Details from {}", if host_name.is_empty() { "the machine" } else { host_name.as_str() })}
                }
                pre { class: "signin-output mono", if details.is_empty() { " " } else { "{details}" } }
            }
            if done {
                div { class: "sheet-actions",
                    if s.exit != Some(0) {
                        button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| on_retry.call(()), "Try again" }
                    }
                    button { class: "button button-md button-primary press", r#type: "button", "data-autofocus": "true", onclick: move |_| on_close.call(()), "Done" }
                }
            }
        }
    }
}

/// What to do next, as numbered steps.
#[component]
fn Steps(code: bool, link: bool, name: String) -> Element {
    let steps: Vec<String> = if code {
        vec!["Open the sign-in page".into(), "Sign in, and enter the code shown here".into(), format!("{name} picks it up by itself")]
    } else if link {
        vec!["Open the sign-in page and sign in".into(), "Copy the code the page shows you".into(), "Paste it below".into()]
    } else {
        vec![format!("Waiting for {name}…")]
    };
    if !link {
        return rsx! {
            div { class: "signin-wait",
                span { class: "host-dot host-dot-connecting" }
                "{steps[0]}"
            }
        };
    }
    rsx! {
        ol { class: "signin-steps",
            for (i , t) in steps.into_iter().enumerate() {
                li { key: "{t}",
                    span { class: "signin-step-n", "{i + 1}" }
                    "{t}"
                }
            }
        }
    }
}

/// A menu from the sign-in ("Select login method") as buttons: click one, use ↑/↓ and Enter, or
/// press its number.
#[component]
fn MenuStep(menu: api::SignInMenu, on_pick: EventHandler<u32>) -> Element {
    let mut picked = use_signal(|| None::<u32>);
    let n = menu.options.len();
    {
        let first = menu.options.iter().position(|o| o.selected).unwrap_or(0);
        use_hook(move || {
            spawn(async move {
                platform::sleep(20).await;
                dom::focus(&format!("signin-opt-{first}"));
            });
        });
    }
    let numbers: Vec<u32> = menu.options.iter().map(|o| o.n).collect();
    let mut pick = move |k: u32| {
        if picked.peek().is_some() {
            return;
        }
        picked.set(Some(k));
        on_pick.call(k);
        spawn(async move {
            platform::sleep(4000).await;
            picked.set(None);
        });
    };
    let question = menu.question.clone().map(|q| q.trim_end_matches(':').to_string());
    rsx! {
        div { class: "stack stack-sm",
            if let Some(q) = &question {
                div { class: "signin-question", "{q}" }
            }
            div {
                class: "signin-options",
                role: "listbox",
                "aria-label": question.clone().unwrap_or_else(|| "Choose one".into()),
                onkeydown: move |e: KeyboardEvent| {
                    let at = dom::active_id().and_then(|id| id.strip_prefix("signin-opt-").and_then(|i| i.parse::<usize>().ok()));
                    match e.key() {
                        Key::ArrowDown => dom::focus(&format!("signin-opt-{}", at.map_or(0, |i| (i + 1) % n))),
                        Key::ArrowUp => dom::focus(&format!("signin-opt-{}", at.map_or(0, |i| (i + n - 1) % n))),
                        Key::Character(c) if c.len() == 1 && c.chars().all(|ch| ch.is_ascii_digit()) && numbers.contains(&c.parse().unwrap_or(0)) => {
                            pick(c.parse().unwrap_or(0));
                        }
                        _ => return,
                    }
                    e.prevent_default();
                },
                for (i , o) in menu.options.iter().cloned().enumerate() {
                    button {
                        key: "{o.n}",
                        id: "signin-opt-{i}",
                        class: if picked() == Some(o.n) { "signin-option signin-option-on" } else { "signin-option" },
                        r#type: "button",
                        role: "option",
                        "aria-selected": "{o.selected}",
                        disabled: picked().is_some(),
                        onclick: move |_| pick(o.n),
                        span { class: "signin-option-n mono", "{o.n}" }
                        span { class: "setup-words",
                            span { class: "setup-label", "{o.label}" }
                            if let Some(d) = &o.detail {
                                span { class: "setup-detail", "{d}" }
                            }
                        }
                        if picked() == Some(o.n) {
                            Glyph { icon: Icon::Spinner, size: 16, class: "spin" }
                        }
                    }
                }
            }
            div { class: "fine-print", "Click one, or use ↑ ↓ and Enter, or press its number." }
        }
    }
}
