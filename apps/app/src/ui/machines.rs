//! The Machines list and a machine's page (pages/Machines.tsx): every machine the hub reaches, how
//! it's doing, and on its page its sessions, setup, agent config files, uploads and the Claude
//! integration. The rules behind what they say are in crate::machines.

use dioxus::prelude::*;
use inyon_protocol::{HostKind, HostState, HostStatus, IntegrationState};

use super::add::{open_action, ActionSheet, PairMachine};
use super::icons::{AgentMark, Glyph, Icon, Logo, MachineGlyph};
use super::kit::{Busy, Notice, Segmented, Tone, WaitRows};
use super::menu;
use super::overview::{badges, host_dot};
use super::toast::{self, confirm, Ask, ToastTone};
use crate::machines::{self as m, UpdateLook};
use crate::model::{self, AgentPane, Shown};
use crate::route::{self, Route};
use crate::state::Hub;
use crate::{api, format, platform};

/// Asks, then removes a machine from the hub.
pub async fn remove(h: &HostState) -> bool {
    let ask = Ask {
        title: format!("Remove {}?", h.config.name),
        body: Some("Its tmux sessions keep running. If it can be reached, Inyon first takes its hooks, uploads and prompt log off it.".into()),
        yes: "Remove".into(),
        danger: true,
        ..Ask::default()
    };
    if !confirm(ask).await {
        return false;
    }
    match api::remove_host(&h.config.id).await {
        Ok(r) => {
            if let Some(name) = r.untold {
                toast::toast(
                    format!("{name} still trusts this phone"),
                    Some("Inyon tells it to forget this phone once it is back online. To end it now, run inyon forget on that computer.".into()),
                    ToastTone::Info,
                );
            }
            true
        }
        Err(e) => {
            toast::toast("Could not remove", Some(e.message), ToastTone::Error);
            false
        }
    }
}

/// The page's own header: a back link or the mark, the title, and its buttons.
#[component]
fn PageHead(title: Element, #[props(default)] back: Option<String>, #[props(default)] actions: Option<Element>) -> Element {
    rsx! {
        header { class: "topbar",
            div { class: "topbar-row page-head",
                if let Some(b) = back {
                    a { class: "icon-button icon-button-lg press", href: "{b}", "aria-label": "Back", title: "Back",
                        Glyph { icon: Icon::ChevronLeft, size: 20 }
                    }
                } else {
                    span { class: "phone-only", Logo {} }
                }
                h1 { class: "page-title truncate", {title} }
                {actions}
            }
        }
    }
}

#[component]
pub fn MachinesPage(add: bool) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut q = use_signal(String::new);
    let mut adding = use_signal(|| add);
    let mut phone = use_signal(|| false);
    // "Pair again" from Can't connect? lands here with ?add=1
    use_effect(use_reactive!(|add| {
        if add {
            adding.set(true);
        }
    }));
    let close = move |_| {
        adding.set(false);
        if matches!(*here.peek(), Route::Machines { add: true }) {
            route::replace(&mut here, Route::Machines { add: false });
        }
    };
    let h = hub.read();
    let hosts = &h.hosts;
    let loaded = h.loaded || !hosts.is_empty();
    let query = q();
    let shown = m::shown(hosts, &query);
    let now = platform::clock().now;
    rsx! {
        div { class: "scroll",
            PageHead {
                title: rsx! { "Machines" },
                actions: rsx! {
                    span { class: "page-actions",
                        button { class: "button button-sm button-ghost press", r#type: "button", onclick: move |_| phone.set(true),
                            Glyph { icon: Icon::Smartphone, size: 14 }
                            "Phone"
                        }
                        button { class: "button button-sm button-primary press", r#type: "button", onclick: move |_| adding.set(true),
                            Glyph { icon: Icon::Plus, size: 14 }
                            "Add machine"
                        }
                    }
                },
            }
            div { class: "narrow",
                if hosts.len() > m::SEARCH_FROM {
                    label { class: "search",
                        Glyph { icon: Icon::Search, size: 16, class: "fg-3" }
                        input {
                            class: "search-input",
                            r#type: "search",
                            value: "{query}",
                            placeholder: "Search {hosts.len()} machines",
                            "aria-label": "Search {hosts.len()} machines",
                            oninput: move |e| q.set(e.value()),
                        }
                    }
                }
                if !loaded {
                    WaitRows { label: "Loading machines" }
                } else if hosts.is_empty() {
                    div { class: "empty",
                        span { class: "empty-icon", Logo { class: "mark-drawn" } }
                        p { class: "empty-title", "No machines yet" }
                        p { class: "empty-text", "Pair a machine or add one over SSH to see the coding agents running in tmux on it." }
                        button { class: "button button-primary button-md press empty-action", r#type: "button", onclick: move |_| adding.set(true),
                            Glyph { icon: Icon::Plus, size: 14 }
                            "Add a machine"
                        }
                    }
                } else if !shown.is_empty() {
                    div { class: "panel rows stagger",
                        for (i , x) in shown.iter().enumerate() {
                            MachineRow { key: "{x.config.id}", h: (*x).clone(), line: m::row_line(x, hosts, now), i }
                        }
                    }
                }
                if !query.is_empty() && shown.is_empty() {
                    p { class: "no-match", "No machine matches “{query}”" }
                }
            }
        }
        super::add::MachineSheet { open: adding(), on_close: close, edit: None }
        super::phone::PhoneSheet { open: phone(), on_close: move |_| phone.set(false) }
    }
}

/// One machine in the list: what it is, whether it answers, and what runs there.
#[component]
fn MachineRow(h: HostState, line: String, i: usize) -> Element {
    let items = menu::machine_menu(&h, true);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let online = h.status == HostStatus::Online;
    let href = Route::Machine { id: h.config.id.clone() }.href();
    rsx! {
        a {
            class: "row machine-item",
            style: "--i:{i}",
            href: "{href}",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            onclick: move |e| {
                p4.click(&e);
            },
            span { class: if online { "machine-tile" } else { "machine-tile fg-3" },
                MachineGlyph { kind: model::machine_kind(&h), size: 18 }
                span { class: "machine-tile-dot",
                    if matches!(h.status, HostStatus::Online | HostStatus::Error) {
                        {host_dot(h.status)}
                    } else {
                        span { class: "host-dot-ring" }
                    }
                }
            }
            div { class: "machine-text",
                div { class: "machine-name-row",
                    span { class: if online { "machine-name" } else { "machine-name fg-2" }, "{h.config.name}" }
                    if let Some(l) = m::node_update_label(&h) {
                        span { class: "chip", "{l}" }
                    }
                }
                div { class: "machine-meta mono", "{line}" }
            }
            span { class: "wide-badges", {badges(&h, 18)} }
            Glyph { icon: Icon::ChevronRight, size: 16, class: "fg-3" }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Sessions,
    Setup,
    Config,
    Uploads,
    Temp,
    Integration,
}

impl Tab {
    fn key(self) -> &'static str {
        match self {
            Tab::Sessions => "sessions",
            Tab::Setup => "setup",
            Tab::Config => "config",
            Tab::Uploads => "uploads",
            Tab::Temp => "temp",
            Tab::Integration => "integration",
        }
    }

    fn of(k: &str) -> Tab {
        match k {
            "setup" => Tab::Setup,
            "config" => Tab::Config,
            "uploads" => Tab::Uploads,
            "temp" => Tab::Temp,
            "integration" => Tab::Integration,
            _ => Tab::Sessions,
        }
    }
}

#[component]
pub fn MachinePage(id: String) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut tab = use_signal(|| None::<Tab>);
    let mut edit = use_signal(|| false);
    let mut repair = use_signal(|| false);
    let h = hub.read();
    let Some(host) = h.hosts.iter().find(|x| x.config.id == id).cloned() else {
        return rsx! {
            div { class: "scroll",
                div { class: "wide-page", WaitRows { label: "Loading this machine" } }
            }
        };
    };
    let hosts = h.hosts.clone();
    drop(h);
    let needs = m::needs_setup(&host);
    // a page that needs setup opens on it, until a tab is picked
    let current = tab().unwrap_or(if needs { Tab::Setup } else { Tab::Sessions });
    let waiting = m::waiting_to_pair(&host);
    let online = host.status == HostStatus::Online;
    let checkable = !waiting;
    let tabs = vec![
        ("sessions".to_string(), "Sessions".to_string()),
        ("setup".into(), if needs { "Setup ·".into() } else { "Setup".into() }),
        ("config".into(), "Config".into()),
        ("uploads".into(), "Uploads".into()),
        ("temp".into(), "Temp files".into()),
        ("integration".into(), "Integration".into()),
    ];
    let removable = host.config.kind != HostKind::Local;
    let gone = host.clone();
    let hid = host.config.id.clone();
    let hid2 = host.config.id.clone();
    let unpaired = host.config.kind == HostKind::Link && host.error.as_deref().is_some_and(|e| e.to_lowercase().contains("paired"));
    rsx! {
        div { class: "scroll",
            PageHead {
                back: Route::Machines { add: false }.href(),
                title: rsx! {
                    span { class: "page-title-row",
                        MachineGlyph { kind: model::machine_kind(&host), size: 17, class: "fg-2" }
                        span { class: "truncate", "{host.config.name}" }
                        {host_dot(host.status)}
                    }
                },
                actions: rsx! {
                    span { class: "page-actions",
                        if !waiting {
                            button { class: "button button-sm button-quiet press", r#type: "button", "aria-label": "New session", title: "New session",
                                onclick: move |_| open_action(ActionSheet::NewSession(hid.clone())),
                                Glyph { icon: Icon::Plus, size: 14 }
                                span { class: "wide-label", "New session" }
                            }
                        }
                        button { class: "button button-sm button-ghost press", r#type: "button", "aria-label": "Rename or edit", onclick: move |_| edit.set(true),
                            Glyph { icon: Icon::Pencil, size: 14 }
                            span { class: "wide-label", if host.config.kind == HostKind::Ssh { "Edit" } else { "Rename" } }
                        }
                        if removable {
                            button { class: "button button-sm button-ghost press", r#type: "button", "aria-label": "Remove", title: "Remove",
                                onclick: move |_| {
                                    let gone = gone.clone();
                                    spawn(async move {
                                        if remove(&gone).await {
                                            route::go(&mut here, Route::Machines { add: false });
                                        }
                                    });
                                },
                                Glyph { icon: Icon::Trash2, size: 14 }
                            }
                        }
                    }
                },
            }
            div { class: "wide-page",
                p { class: "page-line", {m::page_line(&host, &hosts)} }
                UpdateCard { host: host.clone() }
                if let Some(w) = &host.node_warning {
                    Notice { tone: Tone::Warn, "{w}" }
                }
                if !online && (host.error.is_some() || checkable) {
                    div { class: if host.status == HostStatus::Error { "banner banner-error slide-up" } else { "banner banner-quiet slide-up" },
                        if host.status == HostStatus::Error {
                            Glyph { icon: Icon::CircleAlert, size: 16, class: "banner-icon" }
                        } else {
                            span { class: "banner-icon banner-pulse", span { class: "host-dot host-dot-connecting" } }
                        }
                        span { class: "banner-text", {host.error.clone().unwrap_or_else(|| "Not connected right now.".into())} }
                        span { class: "banner-actions",
                            if unpaired {
                                button { class: "button button-xs button-quiet press", r#type: "button", onclick: move |_| repair.set(true), "Pair again" }
                            }
                            if checkable {
                                button { class: "button button-xs button-quiet press", r#type: "button",
                                    onclick: move |_| super::connect::open_connect_help(Some(hid2.clone())),
                                    "Can't connect?"
                                }
                            }
                        }
                    }
                }
                if waiting {
                    div { class: "panel pair-panel",
                        PairMachine { existing: Some(host.config.id.clone()) }
                    }
                } else {
                    div { class: "tabs-row",
                        Segmented { value: current.key().to_string(), options: tabs, label: "Machine", on_change: move |k: String| tab.set(Some(Tab::of(&k))) }
                    }
                    {
                        rsx! {
                            div { key: "{current.key()}", class: "fade-in",
                        match current {
                            Tab::Sessions => rsx! { SessionsTab { host: host.clone() } },
                            Tab::Setup => rsx! { super::setup::SetupPanel { host: host.clone() } },
                            Tab::Config => rsx! { ConfigTab { key: "{host.config.id}", id: host.config.id.clone(), name: host.config.name.clone() } },
                            Tab::Uploads => rsx! { UploadsCard { key: "{host.config.id}", host: host.clone() } },
                            Tab::Temp => rsx! { super::temp::TempFiles { key: "{host.config.id}", host: host.clone() } },
                            Tab::Integration => rsx! { IntegrationCard { host: host.clone() } },
                        }
                            }
                        }
                    }
                    super::pages::MachinePages { host: host.config.id.clone() }
                }
            }
        }
        super::add::MachineSheet { open: edit(), on_close: move |_| edit.set(false), edit: Some(host.config.clone()) }
        super::add::MachineSheet { open: repair(), on_close: move |_| repair.set(false), edit: None }
    }
}

/// A machine whose inyon is behind: what's available and an Update button (only ever on a tap).
#[component]
fn UpdateCard(host: HostState) -> Element {
    let mut asking = use_signal(|| false);
    let mut refused = use_signal(|| None::<String>);
    // what the hub says now replaces an earlier refusal
    let state = host.node_update.as_ref().and_then(|u| u.state);
    use_effect(use_reactive!(|state| {
        if state.is_some() {
            refused.set(None);
        }
    }));
    let Some(card) = m::update_card(&host, refused().as_deref()) else { return rsx! {} };
    let online = host.status == HostStatus::Online;
    let id = host.config.id.clone();
    let failed = card.state == UpdateLook::Failed;
    rsx! {
        div { class: if failed { "update-card update-failed slide-up" } else { "update-card slide-up" }, role: "status",
            span { class: "shrink-0",
                match card.state {
                    UpdateLook::Updating => rsx! { span { class: "host-dot host-dot-connecting" } },
                    UpdateLook::Updated => rsx! { Glyph { icon: Icon::CircleCheck, size: 16 } },
                    UpdateLook::Failed => rsx! { Glyph { icon: Icon::CircleAlert, size: 16, class: "danger" } },
                    UpdateLook::Available => rsx! { Glyph { icon: Icon::CircleArrowUp, size: 16, class: "fg-2" } },
                }
            }
            div { class: "update-words",
                div { class: "update-title", "{card.title}" }
                if let Some(d) = &card.detail {
                    div { class: "update-detail", "{d}" }
                }
            }
            if let Some(label) = card.button {
                button {
                    class: "button button-sm button-primary press",
                    r#type: "button",
                    disabled: !online || asking(),
                    onclick: move |_| {
                        let id = id.clone();
                        asking.set(true);
                        refused.set(None);
                        spawn(async move {
                            if let Err(e) = api::node_update(&id).await {
                                refused.set(Some(e.message));
                            }
                            asking.set(false);
                        });
                    },
                    Busy { on: asking() }
                    "{label}"
                }
            }
        }
    }
}

#[component]
fn SessionsTab(host: HostState) -> Element {
    if host.status != HostStatus::Online {
        return rsx! {
            div { class: "empty",
                p { class: "empty-title", "Not connected" }
                p { class: "empty-text", "Sessions appear once the machine is reachable." }
            }
        };
    }
    let id = host.config.id.clone();
    if host.sessions.is_empty() {
        return rsx! {
            div { class: "empty",
                span { class: "empty-icon", Glyph { icon: Icon::SquareTerminal, size: 20 } }
                p { class: "empty-title", "No tmux sessions" }
                p { class: "empty-text", "Start one from here, or in tmux on the machine." }
                button { class: "button button-md button-quiet press empty-action", r#type: "button", onclick: move |_| open_action(ActionSheet::NewSession(id.clone())),
                    Glyph { icon: Icon::Plus, size: 14 }
                    "New session"
                }
            }
        };
    }
    let all = model::all_panes(std::slice::from_ref(&host));
    rsx! {
        div { class: "session-groups",
            for s in host.sessions.iter() {
                section { key: "{s.id}", class: "panel session-group",
                    div { class: "session-group-head",
                        span { class: "session-group-name", "{s.name}" }
                        if s.attached.0 > 0.0 {
                            span { class: "chip chip-quiet", "attached" }
                        }
                        span { class: "session-group-count", {format::plural(s.windows.len(), "window")} }
                    }
                    for r in all.iter().filter(|r| r.session.id == s.id) {
                        PaneRow {
                            key: "{r.pane.id}",
                            host: host.config.id.clone(),
                            pane: r.pane.id.clone(),
                            place: format!("{}.{}", r.window.index, r.pane.index),
                            title: if r.pane.agent.is_some() { model::pane_title(r) } else { r.pane.command.clone() },
                            cwd: format::short_path(&r.pane.cwd, 50),
                            agent: r.pane.agent.as_ref().map(|a| a.kind),
                            shown: r.pane.agent.as_ref().map(|_| Shown::of(r.agent(), &host)),
                            session_id: r.pane.agent.as_ref().and_then(|a| a.session_id.clone()),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PaneRow(
    host: String,
    pane: String,
    place: String,
    title: String,
    cwd: String,
    agent: Option<inyon_protocol::AgentKind>,
    shown: Option<Shown>,
    session_id: Option<String>,
) -> Element {
    let items = menu::pane_menu(&host, &pane, agent.is_some(), session_id.as_deref(), true);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let href = Route::Session { host: host.clone(), pane: pane.clone(), view: None }.href();
    rsx! {
        a {
            class: "pane-row",
            href: "{href}",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            onclick: move |e| {
                p4.click(&e);
            },
            if let Some(k) = agent {
                AgentMark { kind: k, size: 20 }
            } else {
                span { class: "pane-row-icon", Glyph { icon: Icon::SquareTerminal, size: 16, class: "fg-3" } }
            }
            div { class: "pane-row-text",
                div { class: "pane-row-title truncate",
                    span { class: "pane-row-place mono", "{place}" }
                    "{title}"
                }
                div { class: "pane-row-cwd truncate", "{cwd}" }
            }
            if let Some(s) = shown {
                span { class: "dot dot-{s.class()}", title: "{s.label()}" }
            }
            Glyph { icon: Icon::ChevronRight, size: 16, class: "fg-3" }
        }
    }
}

/// Agent config files on the machine: pick one, edit it with JSON checked as you type, save with
/// a backup kept next to it.
#[component]
fn ConfigTab(id: String, name: String) -> Element {
    let mut files = use_signal(|| None::<Vec<api::ConfigFile>>);
    let mut sel = use_signal(|| None::<String>);
    let mut orig = use_signal(String::new);
    let mut text = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let mut saving = use_signal(|| false);
    {
        let id = id.clone();
        use_hook(move || {
            spawn(async move {
                match api::configs(&id).await {
                    Ok(f) => {
                        if sel.peek().is_none() {
                            sel.set(m::first_config(&f));
                        }
                        files.set(Some(f));
                    }
                    Err(e) => toast::toast("Could not list config files", Some(e.message), ToastTone::Error),
                }
            });
        });
    }
    {
        let id = id.clone();
        let picked = sel();
        use_effect(use_reactive!(|picked| {
            let Some(path) = picked else { return };
            let id = id.clone();
            loading.set(true);
            spawn(async move {
                match api::read_config(&id, &path).await {
                    Ok(c) => {
                        orig.set(c.clone());
                        text.set(c);
                    }
                    Err(e) => toast::toast("Could not read", Some(e.message), ToastTone::Error),
                }
                loading.set(false);
            });
        }));
    }
    let Some(list) = files() else {
        return rsx! { WaitRows { label: "Looking for config files on {name}" } };
    };
    if list.is_empty() {
        return rsx! {
            div { class: "empty",
                span { class: "empty-icon", Glyph { icon: Icon::FileCode2, size: 20 } }
                p { class: "empty-title", "No agent config files" }
                p { class: "empty-text", "Install an agent on this machine and its config files show up here." }
            }
        };
    }
    let cur = list.iter().find(|f| Some(&f.path) == sel().as_ref()).cloned();
    let body = text();
    let json = cur.as_ref().is_some_and(|c| c.format == "json");
    let bad = if json { m::json_error(&body) } else { None };
    let dirty = body != orig();
    let (state, tone) = match (&bad, dirty) {
        (Some(e), _) => (format!("Invalid JSON: {e}"), "danger"),
        (None, true) => ("Unsaved changes".to_string(), "warn"),
        (None, false) if json => ("Valid JSON".to_string(), "fg-3"),
        _ => ("Saved".to_string(), "fg-3"),
    };
    let clock = platform::clock();
    let save_id = id.clone();
    let reading = cur.as_ref().map_or("the file".to_string(), |c| c.label.clone());
    rsx! {
        div { class: "config",
            nav { class: "config-files",
                for g in m::config_groups(&list) {
                    div { key: "{g.as_str()}",
                        div { class: "config-group",
                            AgentMark { kind: g, size: 16 }
                            "{g.def().name}"
                        }
                        for f in list.iter().filter(|f| f.agent == g).cloned() {
                            button {
                                key: "{f.path}",
                                class: if Some(&f.path) == sel().as_ref() { "config-file config-file-on" } else { "config-file" },
                                r#type: "button",
                                onclick: move |_| {
                                    let path = f.path.clone();
                                    spawn(async move {
                                        if dirty && !confirm(Ask { title: "Discard unsaved changes?".into(), yes: "Discard".into(), danger: true, ..Ask::default() }).await {
                                            return;
                                        }
                                        sel.set(Some(path));
                                    });
                                },
                                span { class: if f.exists { "truncate" } else { "truncate fg-3" }, "{f.label}" }
                                span { class: "config-size", if f.exists { {inyon_chat_core::format::bytes(f.size)} } else { "new" } }
                            }
                        }
                    }
                }
            }
            div { class: "config-main",
                div { class: "panel config-editor",
                    div { class: "config-head",
                        span { class: "truncate mono", {sel().map(|p| m::tilde(&p)).unwrap_or_default()} }
                        if let Some(t) = cur.as_ref().and_then(|c| c.mtime) {
                            span { class: "config-changed", "changed {format::time_ago(t * 1000.0, clock)}" }
                        }
                    }
                    if loading() {
                        div { class: "config-wait", role: "status", "aria-label": "Reading {reading}",
                            for i in 0..8 {
                                div { key: "{i}", class: "skeleton skeleton-text", style: "width:{40 + (i * 37) % 50}%" }
                            }
                        }
                    } else {
                        textarea {
                            class: "config-text mono",
                            spellcheck: "false",
                            value: "{body}",
                            placeholder: if cur.as_ref().is_some_and(|c| !c.exists) { "This file does not exist yet. Saving creates it." } else { "" },
                            oninput: move |e| text.set(e.value()),
                        }
                    }
                    div { class: "config-foot",
                        span { class: "config-state truncate {tone}", "{state}" }
                        button { class: "button button-sm button-ghost press", r#type: "button", disabled: !dirty, onclick: move |_| text.set(orig()),
                            Glyph { icon: Icon::RotateCcw, size: 14 }
                            "Discard"
                        }
                        button {
                            class: "button button-sm button-primary press",
                            r#type: "button",
                            disabled: !dirty || bad.is_some() || saving(),
                            onclick: move |_| {
                                let Some(path) = sel() else { return };
                                let id = save_id.clone();
                                let content = text();
                                saving.set(true);
                                spawn(async move {
                                    match api::write_config(&id, &path, &content).await {
                                        Ok(()) => {
                                            orig.set(content);
                                            toast::toast(
                                                "Saved",
                                                Some("A backup of the previous version was kept next to the file. Running sessions pick changes up after a restart.".into()),
                                                ToastTone::Success,
                                            );
                                            if let Ok(f) = api::configs(&id).await {
                                                files.set(Some(f));
                                            }
                                        }
                                        Err(e) => toast::toast("Could not save", Some(e.message), ToastTone::Error),
                                    }
                                    saving.set(false);
                                });
                            },
                            Busy { on: saving() }
                            if !saving() {
                                Glyph { icon: Icon::Save, size: 14 }
                            }
                            "Save"
                        }
                    }
                }
                p { class: "config-note", "Every save keeps the previous version as a .inyon-bak file next to it (the last five)." }
            }
        }
    }
}

/// What the app has sent to the machine, and buttons to clean it up.
#[component]
pub fn UploadsCard(host: HostState) -> Element {
    let mut usage = use_signal(|| None::<api::UploadUsage>);
    let mut busy = use_signal(|| None::<String>);
    let id = host.config.id.clone();
    let online = host.status == HostStatus::Online;
    {
        let id = id.clone();
        use_effect(use_reactive!(|online| {
            if online {
                let id = id.clone();
                spawn(async move {
                    if let Ok(u) = api::upload_usage(&id).await {
                        usage.set(Some(u));
                    }
                });
            }
        }));
    }
    let name = host.config.name.clone();
    let clean = move |days: f64| {
        let id = id.clone();
        let name = name.clone();
        spawn(async move {
            if days == 0.0
                && !confirm(Ask {
                    title: "Delete every uploaded file?".into(),
                    body: Some(format!("All files sent from the app to {name} are removed.")),
                    yes: "Delete all".into(),
                    danger: true,
                    ..Ask::default()
                })
                .await
            {
                return;
            }
            busy.set(Some(days.to_string()));
            match api::clean_uploads(&id, days).await {
                Ok(u) => {
                    usage.set(Some(u));
                    toast::toast("Uploads cleaned", None, ToastTone::Success);
                }
                Err(e) => toast::toast("Could not clean", Some(e.message), ToastTone::Error),
            }
            busy.set(None);
        });
    };
    let u = usage();
    let total = match &u {
        Some(u) => format!("{} · {} files", inyon_chat_core::format::bytes(u.bytes), u.files),
        None if online => "…".into(),
        None => "offline".into(),
    };
    let clean_old = clean.clone();
    let clean_all = clean;
    rsx! {
        div { class: "panel uploads",
            div { class: "uploads-head",
                Glyph { icon: Icon::HardDrive, size: 16, class: "fg-3" }
                span { class: "uploads-name", "{host.config.name}" }
                span { class: "uploads-total tabular", "{total}" }
            }
            if let Some(u) = u {
                div { class: "uploads-dir truncate mono", {m::tilde(&u.dir)} }
                div { class: "uploads-actions",
                    {
                        let days = m::clean_days(&u);
                        let label = m::clean_label(&u);
                        let on = busy() == Some(days.to_string());
                        rsx! {
                            button { class: "button button-sm button-quiet press", r#type: "button", disabled: u.files == 0 || busy().is_some(), onclick: move |_| clean_old(days),
                                Busy { on }
                                "{label}"
                            }
                        }
                    }
                    button { class: "button button-sm button-danger press", r#type: "button", disabled: u.files == 0 || busy().is_some(), onclick: move |_| clean_all(0.0),
                        Busy { on: busy().as_deref() == Some("0") }
                        "Delete all"
                    }
                }
            }
        }
    }
}

#[component]
fn IntegrationCard(host: HostState) -> Element {
    let mut busy = use_signal(|| false);
    let mut sign_in = use_signal(|| None::<inyon_protocol::AgentKind>);
    let on = host.integration == IntegrationState::Installed;
    let online = host.status == HostStatus::Online;
    let id = host.config.id.clone();
    let button = if on {
        "Turn off"
    } else if host.integration == IntegrationState::Outdated {
        "Update"
    } else {
        "Turn on"
    };
    rsx! {
        div { class: "integration",
            div { class: "panel integration-card",
                div { class: "integration-head",
                    Glyph { icon: Icon::Plug, size: 20, class: if on { "" } else { "fg-3" } }
                    div { class: "integration-words",
                        div { class: "integration-title", "Claude Code integration" }
                        div { class: "integration-state", {m::integration_text(host.integration)} }
                    }
                    button {
                        class: if on { "button button-md button-danger press" } else { "button button-md button-primary press" },
                        r#type: "button",
                        disabled: !online || busy(),
                        onclick: move |_| {
                            let id = id.clone();
                            busy.set(true);
                            spawn(async move {
                                match api::integration(&id, !on).await {
                                    Ok(()) => toast::toast(
                                        if on { "Integration turned off" } else { "Integration on" },
                                        (!on).then(|| "Restart running Claude sessions to pick it up.".to_string()),
                                        ToastTone::Success,
                                    ),
                                    Err(e) => toast::toast("Failed", Some(e.message), ToastTone::Error),
                                }
                                busy.set(false);
                            });
                        },
                        Busy { on: busy() }
                        "{button}"
                    }
                }
                ul { class: "integration-list",
                    li { "Adds a tiny hook that records which tmux pane runs which session, for exact matching and instant updates." }
                    li { "Wraps your status line so Inyon sees model, effort, context window, cost and 5-hour/weekly limits. Your status line still shows exactly as before." }
                    li {
                        "Backs up "
                        span { class: "mono", "~/.claude/settings.json" }
                        " first. Turning it off restores your original status line."
                    }
                }
            }
            h3 { class: "section-title", "Agents installed on this machine" }
            div { class: "agent-pills",
                if host.agents.is_empty() {
                    span { class: "fg-3", "None on PATH" }
                }
                for k in host.agents.iter().copied() {
                    span { key: "{k.as_str()}", class: "agent-pill mono",
                        AgentMark { kind: k, size: 18 }
                        "{k.def().name}"
                        if let Some(signed) = &host.signed_in {
                            if signed.contains(&k) {
                                span { class: "agent-pill-note", "· signed in" }
                            } else {
                                button { class: "button button-xs button-primary press", r#type: "button", disabled: !online, onclick: move |_| sign_in.set(Some(k)), "Sign in" }
                            }
                        }
                    }
                }
            }
            super::setup::SignInSheet { host: host.config.id.clone(), host_name: host.config.name.clone(), agent: sign_in(), on_close: move |_| sign_in.set(None) }
        }
    }
}
