//! Adding and changing machines, and starting a session on one (components/Sheets.tsx): the Add
//! sheet with its two ways in (a pairing command run on the machine, or the hub's own SSH), the
//! Rename and Edit sheets, and New session. A menu can open the last two for a machine that isn't
//! on screen, through [`open_action`].

use dioxus::prelude::*;
use inyon_protocol::{AgentKind, HostConfig, HostKind, HostStatus};

use super::icons::{AgentMark, Glyph, Icon, Logo};
use super::kit::{Busy, Field, Opt, Segmented, Select, Sheet};
use super::toast::{self, ToastTone};
use crate::machines as m;
use crate::route::{self, Route};
use crate::state::Hub;
use crate::{api, dom};

/// A sheet a menu opens for a machine.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionSheet {
    NewSession(String),
    Edit(String),
}

static ACTION: GlobalSignal<Option<ActionSheet>> = Signal::global(|| None);
static ACTION_OPEN: GlobalSignal<bool> = Signal::global(|| false);

pub fn open_action(s: ActionSheet) {
    *ACTION.write() = Some(s);
    *ACTION_OPEN.write() = true;
}

/// The sheets menus open, mounted once in the shell. The sheet stays while it slides away.
#[component]
pub fn ActionSheets() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let open = ACTION_OPEN();
    let close = move |_| *ACTION_OPEN.write() = false;
    match ACTION() {
        Some(ActionSheet::NewSession(id)) => rsx! { NewSessionSheet { open, on_close: close, host: id } },
        Some(ActionSheet::Edit(id)) => {
            let edit = hub.read().hosts.iter().find(|h| h.config.id == id).map(|h| h.config.clone());
            rsx! { MachineSheet { open: open && edit.is_some(), on_close: close, edit } }
        }
        None => rsx! {},
    }
}

/// Adding a machine (pair, or SSH), or renaming one (and for SSH, changing how it's reached).
#[component]
pub fn MachineSheet(open: bool, on_close: EventHandler<()>, edit: Option<HostConfig>) -> Element {
    let mut mode = use_signal(|| "pair".to_string());
    let mut name = use_signal(String::new);
    let mut target = use_signal(String::new);
    let mut port = use_signal(String::new);
    let mut key = use_signal(String::new);
    let mut busy = use_signal(|| false);
    // a fresh form every time it opens
    let mut opened = use_signal(|| 0u32);
    {
        let edit = edit.clone();
        use_effect(use_reactive!(|open| {
            if open {
                mode.set("pair".into());
                name.set(edit.as_ref().map(|e| e.name.clone()).unwrap_or_default());
                target.set(edit.as_ref().and_then(|e| e.target.clone()).unwrap_or_default());
                port.set(edit.as_ref().and_then(|e| e.port).map(|p| p.to_string()).unwrap_or_default());
                key.set(edit.as_ref().and_then(|e| e.identity_file.clone()).unwrap_or_default());
                let n = *opened.peek() + 1;
                opened.set(n);
            }
        }));
    }
    let local = edit.as_ref().is_some_and(|e| matches!(e.kind, HostKind::Local | HostKind::Node));
    let pairing = edit.is_none() && mode() == "pair";
    let title = edit.as_ref().map_or("Add a machine".to_string(), |e| format!("Edit {}", e.name));
    let edit_id = edit.as_ref().map(|e| e.id.clone());
    let submit = move || {
        let body = if local {
            api::HostBody { name: name(), ..Default::default() }
        } else {
            api::HostBody { name: name(), target: Some(target()), port: port().parse().ok(), identity_file: Some(key()).filter(|k| !k.is_empty()) }
        };
        let id = edit_id.clone();
        busy.set(true);
        spawn(async move {
            let r = match &id {
                Some(id) => api::update_host(id, &body).await,
                None => api::add_host(&body).await,
            };
            match r {
                Ok(()) => {
                    toast::toast(if id.is_some() { "Machine updated" } else { "Machine added" }, (!local).then(|| "Connecting over SSH…".to_string()), ToastTone::Success);
                    on_close.call(());
                }
                Err(e) => toast::toast("Could not save", Some(e.message), ToastTone::Error),
            }
            busy.set(false);
        });
    };
    let mut submit_form = submit.clone();
    let mut submit_button = submit;
    let ready = !name().is_empty() && (local || !target().is_empty());
    rsx! {
        Sheet {
            open,
            on_close: move |_| on_close.call(()),
            title,
            footer: if pairing { None } else {
                Some(rsx! {
                    div { class: "sheet-actions",
                        button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| on_close.call(()), "Cancel" }
                        button { class: "button button-md button-primary press", r#type: "button", disabled: !ready || busy(), onclick: move |_| submit_button(),
                            Busy { on: busy() }
                            if edit.is_some() { "Save" } else { "Add machine" }
                        }
                    }
                })
            },
            if edit.is_none() {
                div { class: "sheet-switch",
                    Segmented {
                        value: mode(),
                        full: true,
                        label: "How to add it",
                        options: vec![("pair".into(), "Run a command on it".into()), ("ssh".into(), "Reach it over SSH".into())],
                        on_change: move |v| mode.set(v),
                    }
                }
            }
            if pairing {
                PairMachine { key: "{opened}", on_close: move |_| on_close.call(()) }
            } else {
                form {
                    class: "stack",
                    onsubmit: move |e| {
                        e.prevent_default();
                        if ready && !busy() {
                            submit_form();
                        }
                    },
                    if edit.is_none() {
                        p { class: "sheet-text",
                            "The hub signs in with its own SSH keys and config. Nothing is installed on the machine. Use this when the hub can already reach it: same network, a VPN, or a jump host set up in the hub's "
                            code { class: "mono", "~/.ssh/config" }
                            "."
                        }
                    }
                    Field { label: "Name",
                        input { class: "field", value: "{name}", placeholder: "Work server", "data-autofocus": "true", oninput: move |e| name.set(e.value()) }
                    }
                    if !local {
                        Field { label: "SSH destination", hint: "user@host, or a Host alias from ~/.ssh/config on the hub machine. The hub uses its own SSH keys, agent and config.",
                            input {
                                class: "field mono-field",
                                value: "{target}",
                                placeholder: "deploy@work-server",
                                autocapitalize: "off",
                                autocorrect: "off",
                                spellcheck: "false",
                                oninput: move |e| target.set(e.value()),
                            }
                        }
                        div { class: "port-row",
                            Field { label: "Port",
                                input {
                                    class: "field",
                                    value: "{port}",
                                    placeholder: "22",
                                    inputmode: "numeric",
                                    oninput: move |e| port.set(e.value().chars().filter(|c| c.is_ascii_digit()).collect()),
                                }
                            }
                            Field { label: "Key file (optional)",
                                input {
                                    class: "field mono-field",
                                    value: "{key}",
                                    placeholder: "~/.ssh/id_ed25519",
                                    autocapitalize: "off",
                                    spellcheck: "false",
                                    oninput: move |e| key.set(e.value()),
                                }
                            }
                        }
                    }
                    // Enter in a field submits
                    button { r#type: "submit", hidden: true, tabindex: "-1" }
                }
            }
        }
    }
}

/// Pairing a machine that connects out to the hub: nothing is opened on that machine. With
/// `existing`, a new command for a machine that was added but never ran its own.
#[component]
pub fn PairMachine(#[props(default)] existing: Option<String>, #[props(default)] on_close: Option<EventHandler<()>>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut name = use_signal(String::new);
    let mut pair = use_signal(|| None::<api::Pairing>);
    // only a guess: the machine being added is often not the one with this browser
    let mut windows = use_signal(dom::windows);
    let mut busy = use_signal(|| false);
    let start = {
        let existing = existing.clone();
        move || {
            let existing = existing.clone();
            let n = name();
            busy.set(true);
            spawn(async move {
                let origin = dom::origin();
                let r = match &existing {
                    Some(id) => api::repair(id, &origin).await,
                    None => api::pair(if n.is_empty() { "Laptop" } else { &n }, &origin).await,
                };
                match r {
                    Ok(p) => pair.set(Some(p)),
                    Err(e) => toast::toast("Could not create a pairing", Some(e.message), ToastTone::Error),
                }
                busy.set(false);
            });
        }
    };
    let Some(p) = pair() else {
        let mut start = start;
        if existing.is_some() {
            return rsx! {
                div { class: "stack",
                    p { class: "sheet-text",
                        "This machine was added but hasn't connected yet. Run its pairing command on it. If you no longer have the command, make a new one; the old one stops working."
                    }
                    div {
                        button { class: "button button-md button-primary press", r#type: "button", disabled: busy(), onclick: move |_| start(),
                            Busy { on: busy() }
                            "New pairing command"
                        }
                    }
                }
            };
        }
        let mut start2 = start.clone();
        return rsx! {
            form {
                class: "stack",
                onsubmit: move |e| {
                    e.prevent_default();
                    if !busy() {
                        start2();
                    }
                },
                div { class: "aside",
                    Glyph { icon: Icon::ShieldCheck, size: 16, class: "aside-icon" }
                    span {
                        "You run one command on the machine you're adding. From then on it connects out to this hub by itself, so it works behind a router or firewall and you don't open SSH or any port on it. Best for laptops and anything the hub can't reach. Remove the machine to revoke it."
                    }
                }
                Field { label: "Name",
                    input { class: "field", value: "{name}", placeholder: "My laptop", "data-autofocus": "true", oninput: move |e| name.set(e.value()) }
                }
                div { class: "form-end",
                    button { class: "button button-md button-primary press", r#type: "submit", disabled: busy(),
                        Busy { on: busy() }
                        "Create pairing command"
                    }
                }
            }
        };
    };
    let command = m::pair_command(&p, windows()).to_string();
    let joined = hub.read().hosts.iter().find(|h| h.config.id == p.id).filter(|h| h.status == HostStatus::Online).map(|h| h.config.name.clone());
    let copy = command.clone();
    rsx! {
        div { class: "stack",
            if p.commands.is_some() {
                Segmented {
                    value: if windows() { "windows" } else { "unix" },
                    full: true,
                    label: "System",
                    options: vec![("unix".into(), "macOS or Linux".into()), ("windows".into(), "Windows".into())],
                    on_change: move |v: String| windows.set(v == "windows"),
                }
            }
            p { class: "sheet-text",
                if windows() {
                    "Paste this once into PowerShell or Command Prompt on that machine. It installs a small background app for your user that starts when you sign in, plus psmux (tmux for Windows) if it is missing. No admin rights, no open ports."
                } else {
                    "Run this once in a terminal on that machine. It downloads inyon from this hub and keeps it running in the background for your user. No sudo, no open ports."
                }
            }
            div { class: "command-box",
                code { class: "command-text mono", "{command}" }
                button {
                    class: "button button-sm button-quiet press",
                    r#type: "button",
                    onclick: move |_| {
                        let c = copy.clone();
                        spawn(async move {
                            if dom::copy(&c).await {
                                toast::toast("Command copied", None, ToastTone::Success);
                            } else {
                                toast::toast("Could not copy", Some("Select the command and copy it by hand.".into()), ToastTone::Error);
                            }
                        });
                    },
                    Glyph { icon: Icon::Copy, size: 14 }
                    "Copy command"
                }
            }
            div { class: "pair-wait", role: "status",
                if let Some(n) = &joined {
                    Glyph { icon: Icon::Check, size: 16 }
                    "{n} is connected."
                } else {
                    Logo { class: "logo-16", connecting: true }
                    "Waiting for the machine to connect…"
                }
            }
            p { class: "fine-print",
                "The command contains this machine's secret token: don't share it. Plain http is only accepted on private networks (home LAN, Tailscale, NetBird); otherwise put the hub behind https."
            }
            if let Some(close) = on_close {
                div { class: "form-end",
                    button { class: "button button-md button-primary press", r#type: "button", onclick: move |_| close.call(()), if joined.is_some() { "Done" } else { "Close" } }
                }
            }
        }
    }
}

/// What New session starts: an agent the machine has, one of your own agents, or a terminal.
#[derive(Debug, Clone, PartialEq)]
enum Pick {
    Agent(AgentKind),
    Custom(api::CustomAgent),
    Shell,
}

impl Pick {
    fn kind(&self) -> Option<AgentKind> {
        match self {
            Pick::Agent(k) => Some(*k),
            Pick::Custom(c) => Some(c.like),
            Pick::Shell => None,
        }
    }

    fn name(&self) -> String {
        match self {
            Pick::Agent(k) => k.def().name.clone(),
            Pick::Custom(c) => c.name.clone(),
            Pick::Shell => "terminal".into(),
        }
    }
}

fn first_pick(installed: &[AgentKind]) -> Pick {
    m::default_agent(installed).map_or(Pick::Shell, Pick::Agent)
}

#[component]
pub fn NewSessionSheet(open: bool, on_close: EventHandler<()>, host: String) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut on = use_signal(String::new);
    let mut pick = use_signal(|| Pick::Shell);
    let mut customs = use_signal(Vec::<api::CustomAgent>::new);
    let mut cwd = use_signal(|| "~".to_string());
    let mut session = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut model = use_signal(String::new);
    let mut mode = use_signal(String::new);
    let mut prompt = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let online: Vec<inyon_protocol::HostState> = hub.read().hosts.iter().filter(|h| h.status == HostStatus::Online).cloned().collect();
    let first = if online.iter().any(|h| h.config.id == host) { Some(host.clone()) } else { online.first().map(|h| h.config.id.clone()) };
    {
        let first = first.clone();
        let online = online.clone();
        use_effect(use_reactive!(|open| {
            if !open {
                return;
            }
            let f = first.clone().unwrap_or_default();
            let inst = online.iter().find(|h| h.config.id == f).map(|h| h.agents.clone()).unwrap_or_default();
            on.set(f);
            session.set(String::new());
            prompt.set(String::new());
            model.set(String::new());
            pick.set(first_pick(&inst));
            spawn(async move {
                customs.set(api::settings().await.ok().and_then(|s| s.custom_agents).unwrap_or_default());
            });
        }));
    }
    // opened before the machines arrived: pick one once they do
    {
        let online = online.clone();
        use_effect(use_reactive!(|first| {
            if open && on.peek().is_empty() {
                if let Some(f) = first {
                    let inst = online.iter().find(|h| h.config.id == f).map(|h| h.agents.clone()).unwrap_or_default();
                    on.set(f);
                    pick.set(first_pick(&inst));
                }
            }
        }));
    }
    let h = online.iter().find(|x| x.config.id == on()).cloned();
    let installed = h.as_ref().map(|h| h.agents.clone()).unwrap_or_default();
    let recent = m::recent_folders(&hub.read().hosts, &on());
    let chosen = pick();
    let kind = chosen.kind();
    let def = kind.map(|k| k.def());
    let has_mode = matches!(kind, Some(AgentKind::Claude | AgentKind::Antigravity));
    let start_label = format!("Start {}", chosen.name());
    let where_label = if session().is_empty() { "Opens in a new tmux session" } else { "Opens in a new tmux window" };
    let placeholder = match &chosen {
        Pick::Custom(c) => c.name.clone(),
        Pick::Shell => "shell".into(),
        Pick::Agent(k) => k.as_str().to_string(),
    };
    let submit = move || {
        let chosen = pick();
        let kind = chosen.kind();
        let chat = kind.is_some_and(|k| k.def().chat);
        let has_mode = matches!(kind, Some(AgentKind::Claude | AgentKind::Antigravity));
        let host = on();
        let body = api::Launch {
            agent: kind.map_or("shell".to_string(), |k| k.as_str().to_string()),
            custom: match &chosen {
                Pick::Custom(c) => Some(c.id.clone()),
                _ => None,
            },
            cwd: Some(cwd()),
            session: Some(session()).filter(|s| !s.is_empty()),
            name: Some(name()).filter(|s| !s.is_empty()),
            model: Some(model()).filter(|s| !s.is_empty()),
            mode: Some(mode()).filter(|s| has_mode && !s.is_empty()),
            prompt: Some(prompt()).filter(|s| chat && !s.is_empty()),
            sign_in: false,
        };
        busy.set(true);
        spawn(async move {
            match api::launch(&host, &body).await {
                Ok(s) => {
                    on_close.call(());
                    let view = kind.is_none().then_some(inyon_chat_core::overview::PaneView::Terminal);
                    route::go(&mut here, Route::Session { host, pane: s.pane, view });
                }
                Err(e) => toast::toast("Could not start", Some(e.message), ToastTone::Error),
            }
            busy.set(false);
        });
    };
    let mut submit_button = submit;
    let mut submit_form = submit;
    let machines: Vec<Opt> = online
        .iter()
        .map(|x| Opt::new(x.config.id.clone(), x.config.name.clone()).hint(if x.config.kind == HostKind::Link { Some("Paired".into()) } else { x.config.target.clone() }))
        .collect();
    let tmux: Vec<Opt> = std::iter::once(Opt::new("", "New session"))
        .chain(h.iter().flat_map(|h| h.sessions.iter()).map(|s| Opt::new(s.name.clone(), format!("New window in “{}”", s.name))))
        .collect();
    rsx! {
        Sheet {
            open,
            on_close: move |_| on_close.call(()),
            wide: true,
            title: "New session",
            footer: rsx! {
                div { class: "sheet-actions",
                    span { class: "sheet-foot-note truncate", "{where_label}" }
                    button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| on_close.call(()), "Cancel" }
                    button { class: "button button-md button-primary press", r#type: "button", disabled: on().is_empty() || busy(), onclick: move |_| submit_button(),
                        Busy { on: busy() }
                        "{start_label}"
                    }
                }
            },
            form {
                class: "stack stack-lg",
                onsubmit: move |e| {
                    e.prevent_default();
                    if !on().is_empty() && !busy() {
                        submit_form();
                    }
                },
                div {
                    div { class: "field-label", "Agent" }
                    div { class: "agent-grid", role: "radiogroup", "aria-label": "Agent",
                        for k in m::agent_choices(&installed) {
                            {
                                let p = k.map_or(Pick::Shell, Pick::Agent);
                                let selected = chosen == p;
                                rsx! {
                                    button {
                                        key: "{k.map_or(\"shell\", |k| k.as_str())}",
                                        class: if selected { "agent-choice agent-choice-on" } else { "agent-choice" },
                                        style: if k.is_none() { "order:2" } else { "" },
                                        r#type: "button",
                                        role: "radio",
                                        "aria-checked": "{selected}",
                                        onclick: move |_| pick.set(p.clone()),
                                        if let Some(k) = k {
                                            AgentMark { kind: k, size: 26 }
                                        } else {
                                            span { class: "shell-mark", Glyph { icon: Icon::SquareTerminal, size: 16 } }
                                        }
                                        span { class: "agent-choice-words",
                                            span { class: "agent-choice-name truncate", {k.map_or("Terminal".to_string(), |k| k.def().name.clone())} }
                                            span { class: "agent-choice-note truncate", {k.map_or("Plain shell".to_string(), |k| k.def().vendor.clone())} }
                                        }
                                    }
                                }
                            }
                        }
                        for c in customs() {
                            {
                                let selected = matches!(&chosen, Pick::Custom(x) if x.id == c.id);
                                let p = Pick::Custom(c.clone());
                                rsx! {
                                    button {
                                        key: "custom-{c.id}",
                                        class: if selected { "agent-choice agent-choice-on" } else { "agent-choice" },
                                        style: "order:1",
                                        r#type: "button",
                                        role: "radio",
                                        "aria-checked": "{selected}",
                                        title: "{c.command}",
                                        onclick: move |_| pick.set(p.clone()),
                                        AgentMark { kind: c.like, size: 26 }
                                        span { class: "agent-choice-words",
                                            span { class: "agent-choice-name truncate", "{c.name}" }
                                            span { class: "agent-choice-note truncate", "Like {c.like.def().name}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(h) = h.as_ref().filter(|h| h.agents.is_empty()) {
                        p { class: "field-hint", "No agent CLIs found on {h.config.name}'s PATH." }
                    }
                }
                div { class: "form-grid",
                    Field { label: "Machine", group: true,
                        Select {
                            title: "Machine",
                            value: on(),
                            options: machines,
                            on_change: move |v: String| {
                                on.set(v);
                                session.set(String::new());
                            },
                        }
                    }
                    Field { label: "Folder",
                        input {
                            class: "field mono-field",
                            value: "{cwd}",
                            autocapitalize: "none",
                            autocorrect: "off",
                            spellcheck: "false",
                            oninput: move |e| cwd.set(e.value()),
                        }
                    }
                }
                if !recent.is_empty() {
                    div { class: "folder-chips",
                        for r in recent {
                            button {
                                key: "{r}",
                                class: if cwd() == r { "folder-chip folder-chip-on mono" } else { "folder-chip mono" },
                                r#type: "button",
                                title: "{r}",
                                onclick: { let r = r.clone(); move |_| cwd.set(r.clone()) },
                                {inyon_chat_core::format::tail_path(&r, 32)}
                            }
                        }
                    }
                }
                if let Some(d) = def.filter(|d| !d.models.is_empty() || has_mode) {
                    div { class: "form-grid",
                        if !d.models.is_empty() {
                            Field { label: "Model", group: true,
                                Select {
                                    title: "Model",
                                    value: model(),
                                    options: std::iter::once(Opt::new("", "Default")).chain(d.models.iter().map(|x| Opt::new(x.id.clone(), x.label.clone()))).collect::<Vec<_>>(),
                                    on_change: move |v| model.set(v),
                                }
                            }
                        }
                        if has_mode {
                            Field { label: "Permissions", group: true,
                                Select {
                                    title: "Permissions",
                                    value: mode(),
                                    options: std::iter::once(Opt::new("", "Default")).chain(d.modes.iter().map(|x| Opt::new(x.id.clone(), x.label.clone()))).collect::<Vec<_>>(),
                                    on_change: move |v| mode.set(v),
                                }
                            }
                        }
                    }
                }
                if def.is_some_and(|d| d.chat) {
                    Field { label: "First message (optional)",
                        textarea {
                            class: "field field-area",
                            rows: "3",
                            value: "{prompt}",
                            placeholder: "What should it work on?",
                            oninput: move |e| prompt.set(e.value()),
                        }
                    }
                }
                div { class: "form-grid",
                    Field { label: "tmux", group: true,
                        Select { title: "tmux", value: session(), options: tmux, on_change: move |v| session.set(v) }
                    }
                    if session().is_empty() {
                        Field { label: "Session name",
                            input {
                                class: "field",
                                value: "{name}",
                                placeholder: "{placeholder}",
                                autocapitalize: "none",
                                autocorrect: "off",
                                spellcheck: "false",
                                oninput: move |e| name.set(e.value()),
                            }
                        }
                    }
                }
                button { r#type: "submit", hidden: true, tabindex: "-1" }
            }
        }
    }
}
