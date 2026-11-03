//! Settings (pages/Settings.tsx), as the hub's own web app shows it: how the app looks, the agents
//! (with your own commands), the companion, the hub's live numbers, uploads, notifications, the
//! machines, other devices, paired phones and companion apps, the account, updates and about.
//! What the app keeps for itself is saved in the browser as the web app saves it
//! (ui/notices.rs); what the hub keeps goes back to it with `PUT /api/settings`.

use dioxus::prelude::*;
use inyon_chat_core::store::{ReplyFontSetting, Theme, ToolDetail};
use inyon_protocol::{AgentKind, HostState, PairedCompanion};

use super::icons::{AgentMark, Glyph, Icon, Logo};
use super::kit::{Busy, Field, Opt, Segmented, Select, Sheet, Toggle, WaitRows};
use super::menu::{self, Action, Item};
use super::notices::{self, set_prefs, HUB_BUILD, PREFS};
use super::toast::{self, confirm, Ask, Toast, ToastTone};
use super::Auth;
use crate::api::{self, CustomAgent, HubSettings, HubUpdate, PairedPhone};
use crate::route::Route;
use crate::settings::{self as st, Build, Permission, UpdateRow};
use crate::state::Hub;
use crate::{dom, platform};

/// What the Bun hub can't keep.
const OLD_HUB: &str = "This hub runs on Bun, which keeps no custom agents. Run the default Rust hub to add them.";

/// The sections the desktop's side list jumps to, in order.
const NAV: [(&str, &str); 12] = [
    ("appearance", "Appearance"),
    ("agents", "Agents"),
    ("companion", "Companion"),
    ("sessions", "Live numbers"),
    ("uploads", "Uploads and clipboard"),
    ("notifications", "Notifications"),
    ("machines", "Machines"),
    ("devices", "Other devices"),
    ("paired", "Paired devices"),
    ("account", "Account"),
    ("updates", "Updates"),
    ("about", "About"),
];

/// The hub's settings as last loaded or saved; kept here so an Undo still works after the page
/// has closed.
static HUB: GlobalSignal<Option<HubSettings>> = Signal::global(|| None);
/// The custom agent being edited: `Some(None)` for a new one.
static EDITING: GlobalSignal<Option<Option<CustomAgent>>> = Signal::global(|| None);
static PHONES: GlobalSignal<Option<Vec<PairedPhone>>> = Signal::global(|| None);
static COMPANIONS: GlobalSignal<Option<Vec<PairedCompanion>>> = Signal::global(|| None);
// why the last read of each failed; what was read before stays on screen
static PHONES_FAILED: GlobalSignal<Option<String>> = Signal::global(|| None);
static COMPANIONS_FAILED: GlobalSignal<Option<String>> = Signal::global(|| None);
static SETTINGS_FAILED: GlobalSignal<Option<String>> = Signal::global(|| None);
/// The companion app whose "What it can do" sheet is open.
static DETAILS: GlobalSignal<Option<PairedCompanion>> = Signal::global(|| None);

/// What a menu in Settings does.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsOp {
    EditAgent(String),
    RemoveAgent(String),
    /// puts a removed custom agent back where it was
    UndoRemove(CustomAgent, usize),
    CompanionDetails(String),
    RemoveCompanion(String),
    ForgetPhone(String),
}

pub fn run(op: SettingsOp) {
    match op {
        SettingsOp::EditAgent(id) => {
            let a = customs().into_iter().find(|a| a.id == id);
            if a.is_some() {
                *EDITING.write() = Some(a);
            }
        }
        SettingsOp::RemoveAgent(id) => spawn_forever_here(remove_agent(id)),
        SettingsOp::UndoRemove(agent, at) => spawn_forever_here(async move {
            if let Err(e) = save_agents(move |list| st::put_back(list, agent, at)).await {
                toast::toast("Could not undo", Some(e), ToastTone::Error);
            }
        }),
        SettingsOp::CompanionDetails(id) => {
            let c = COMPANIONS.peek().iter().flatten().find(|c| c.id == id).cloned();
            *DETAILS.write() = c;
        }
        SettingsOp::RemoveCompanion(id) => {
            if let Some(c) = COMPANIONS.peek().iter().flatten().find(|c| c.id == id).cloned() {
                spawn_forever_here(remove_companion(c));
            }
        }
        SettingsOp::ForgetPhone(id) => {
            if let Some(p) = PHONES.peek().iter().flatten().find(|p| p.id == id).cloned() {
                spawn_forever_here(forget_phone(p));
            }
        }
    }
}

/// A menu's work outlives the menu (and the row it came from).
fn spawn_forever_here(f: impl std::future::Future<Output = ()> + 'static) {
    dioxus::core::spawn_forever(f);
}

fn customs() -> Vec<CustomAgent> {
    HUB.peek().as_ref().and_then(|h| h.custom_agents.clone()).unwrap_or_default()
}

/// Saves the custom agents with the hub's other settings as they are now.
/// Changes the custom agents and saves them with the hub's other settings. The change runs once
/// it's this save's turn, on the list as it is then, so one save can't undo another.
async fn save_agents<R>(change: impl FnOnce(&mut Vec<CustomAgent>) -> R) -> Result<R, String> {
    take_turn().await;
    let out = put_agents(change).await;
    end_turn();
    out
}

async fn put_agents<R>(change: impl FnOnce(&mut Vec<CustomAgent>) -> R) -> Result<R, String> {
    let Some(mut base) = HUB.peek().clone() else { return Err("The hub's settings haven't loaded yet.".into()) };
    let mut list = base.custom_agents.clone().unwrap_or_default();
    let out = change(&mut list);
    base.custom_agents = Some(list);
    let saved = api::save_settings(&base).await.map_err(|e| e.message)?;
    // the Bun hub (INYON_HUB_IMPL=bun) doesn't keep them
    if saved.custom_agents.is_none() {
        return Err(OLD_HUB.into());
    }
    // only the agents from the answer: anything else changed meanwhile goes out on its own turn
    EDITS.with(|e| e.set(e.get() + 1));
    if let Some(h) = HUB.write().as_mut() {
        h.custom_agents = saved.custom_agents;
    }
    Ok(out)
}

async fn remove_agent(id: String) {
    let removed = save_agents(|list| {
        let at = list.iter().position(|a| a.id == id)?;
        Some((list.remove(at), at))
    })
    .await;
    match removed {
        Ok(Some((a, at))) => toast::show(Toast {
            id: 0,
            title: format!("Removed {}", a.name),
            body: None,
            tone: ToastTone::Info,
            href: None,
            action: Some(("Undo".into(), Action::Settings(SettingsOp::UndoRemove(a, at)))),
            agent: None,
        }),
        Ok(None) => {}
        Err(e) => toast::toast("Could not remove", Some(e), ToastTone::Error),
    }
}

async fn load_phones() {
    match api::phones().await {
        Ok(list) => {
            *PHONES.write() = Some(list);
            *PHONES_FAILED.write() = None;
        }
        // not "no phones paired": the list from before stays
        Err(e) => *PHONES_FAILED.write() = Some(e.message),
    }
}

async fn load_companions() {
    match api::companions().await {
        Ok(list) => {
            *COMPANIONS.write() = Some(list);
            *COMPANIONS_FAILED.write() = None;
        }
        Err(e) => *COMPANIONS_FAILED.write() = Some(e.message),
    }
}

async fn load_settings() {
    let at = edits();
    match api::settings().await {
        Ok(s) => {
            *SETTINGS_FAILED.write() = None;
            settle(at, s);
        }
        Err(e) => *SETTINGS_FAILED.write() = Some(e.message),
    }
}

/// A read that failed, with Try again.
#[component]
fn ReadFailed(text: String, on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { class: "stale-note", role: "status",
            Glyph { icon: Icon::CircleAlert, size: 14 }
            span { class: "stale-note-text", "{text}" }
            button { class: "button button-xs button-ghost press", r#type: "button", onclick: move |_| on_retry.call(()), "Try again" }
        }
    }
}

async fn forget_phone(p: PairedPhone) {
    match api::forget_phone(&p.id).await {
        Ok(()) => {
            toast::toast(format!("{} forgotten", p.name), Some("It can pair again with a new code.".into()), ToastTone::Success);
            load_phones().await;
        }
        Err(e) => toast::toast("Couldn't forget it", Some(e.message), ToastTone::Error),
    }
}

/// A companion's token can't come back, so this asks first.
async fn remove_companion(c: PairedCompanion) {
    let yes = confirm(Ask {
        title: format!("Remove {}?", c.name),
        body: Some("Its key stops working now and its connection closes. To use it again, pair it with a new code.".into()),
        yes: "Remove".into(),
        danger: true,
        ..Ask::default()
    })
    .await;
    if !yes {
        return;
    }
    match api::forget_companion(&c.id).await {
        Ok(()) => {
            *DETAILS.write() = None;
            toast::toast(format!("{} removed", c.name), None, ToastTone::Success);
            load_companions().await;
        }
        Err(e) => toast::toast("Couldn't remove it", Some(e.message), ToastTone::Error),
    }
}

// The hub's settings go out whole, so one save at a time: a change made while one is on its way
// goes out after it, as the settings are then. `EDITS` counts the changes made here, so an answer
// is only shown when nothing newer was changed meanwhile.
thread_local! {
    static SAVING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static AGAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static EDITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn edits() -> u64 {
    EDITS.with(|e| e.get())
}

/// Shows what the hub said, unless something was changed here since `at`.
fn settle(at: u64, s: HubSettings) {
    if edits() == at {
        *HUB.write() = Some(s);
    }
}

/// Waits for the save on its way, then holds the turn.
async fn take_turn() {
    while SAVING.with(|s| s.replace(true)) {
        platform::sleep(30).await;
    }
}

/// Gives the turn back, sending what changed while it was held.
fn end_turn() {
    SAVING.with(|s| s.set(false));
    if AGAIN.with(|a| a.replace(false)) {
        spawn_forever_here(push_hub());
    }
}

/// Saves the hub's settings with a change, showing it at once.
fn save_hub(change: impl FnOnce(&mut HubSettings)) {
    let Some(mut next) = HUB.peek().clone() else { return };
    change(&mut next);
    *HUB.write() = Some(next);
    EDITS.with(|e| e.set(e.get() + 1));
    if SAVING.with(|s| s.get()) {
        AGAIN.with(|a| a.set(true));
    } else {
        spawn_forever_here(push_hub());
    }
}

/// Sends the settings as they are now. A failure shows the hub's own again, unless something
/// newer was changed meanwhile (which goes out next).
async fn push_hub() {
    take_turn().await;
    let at = edits();
    // read before the answer writes HUB: a guard in the `if let` would last through it
    let next = HUB.peek().clone();
    if let Some(next) = next {
        match api::save_settings(&next).await {
            Ok(saved) => settle(at, saved),
            Err(e) => {
                toast::toast("Could not save", Some(e.message), ToastTone::Error);
                if let Ok(s) = api::settings().await {
                    settle(at, s);
                }
            }
        }
    }
    end_turn();
}

fn date(ms: f64) -> String {
    dom::local_date(ms)
}

fn since(at: f64) -> String {
    st::since(at, platform::clock().now, date)
}

// ---------- the page ----------

#[component]
pub fn SettingsPage() -> Element {
    let hub = use_context::<Signal<Hub>>();
    use_hook(|| {
        spawn(load_settings());
        // a link to one section (/settings#notifications) opens on it
        let hash = dom::hash();
        if !hash.is_empty() {
            spawn(async move {
                platform::sleep(50).await;
                dom::scroll_to(&hash);
            });
        }
    });
    let h = hub.read();
    rsx! {
        div { class: "scroll",
            header { class: "topbar",
                div { class: "topbar-row",
                    span { class: "phone-only", Logo { connecting: !h.connected, target: true } }
                    h1 { class: "topbar-title", "Settings" }
                }
            }
            div { class: "settings",
                nav { class: "settings-nav", "aria-label": "Sections",
                    for (id , label) in NAV {
                        a { key: "{id}", class: "settings-nav-item", href: "#{id}", "{label}" }
                    }
                }
                div { class: "settings-body",
                    if let (None, Some(why)) = (HUB.read().as_ref(), SETTINGS_FAILED()) {
                        ReadFailed {
                            text: format!("Couldn't read the hub's settings: {why}"),
                            on_retry: move |_| {
                                spawn(load_settings());
                            },
                        }
                    }
                    Appearance {}
                    Agents {}
                    Section { id: "companion", title: "Companion", desc: "An agent you talk to about your other sessions, here, on your phone or on Legato's island. It reads every session, and sends a message or answers a question when you ask it to.",
                        super::companion::CompanionRows {}
                    }
                    LiveNumbers {}
                    Uploads { hosts: h.hosts.clone() }
                    Notifications {}
                    Machines { hosts: h.hosts.clone() }
                    OtherDevices {}
                    PairedDevices {}
                    Account {}
                    Section { id: "updates", title: "Updates", desc: "Nothing updates by itself. Update here, or run inyon update on the hub.",
                        HubUpdateRow {}
                    }
                    About {}
                    p { class: "settings-foot", "Agent marks belong to their owners" }
                }
            }
        }
        CustomAgentSheet {}
        CompanionDetails {}
    }
}

/// A heading, a line about it, and its rows in one card.
#[component]
fn Section(id: String, title: String, #[props(default)] desc: Option<String>, children: Element) -> Element {
    rsx! {
        section { id: "{id}", class: "set-section",
            h2 { class: "set-title", "{title}" }
            if let Some(d) = desc {
                p { class: "set-desc", "{d}" }
            }
            div { class: "set-card", {children} }
        }
    }
}

/// A setting: what it is, a hint under it, and its control on the right (under it on a phone).
#[component]
fn Row(label: String, #[props(default)] icon: Option<Icon>, #[props(default)] hint: Option<String>, #[props(default)] children: Element) -> Element {
    rsx! {
        div { class: "set-row",
            div { class: "set-words",
                div { class: "set-label",
                    if let Some(i) = icon {
                        Glyph { icon: i, size: 16, class: "fg-3" }
                    }
                    "{label}"
                }
                if let Some(h) = hint {
                    div { class: "set-hint", "{h}" }
                }
            }
            {children}
        }
    }
}

/// A row that is one button: Copy version details, Sign out.
#[component]
fn ActionRow(label: String, icon: Icon, on_click: EventHandler<()>, #[props(default)] danger: bool, #[props(default)] external: bool) -> Element {
    rsx! {
        button { class: if danger { "set-action set-action-danger" } else { "set-action" }, r#type: "button", onclick: move |_| on_click.call(()),
            Glyph { icon, size: 16, class: if danger { "" } else { "fg-3" } }
            "{label}"
            if external {
                span { class: "fg-3", "↗" }
            }
        }
    }
}

fn theme_value(t: Theme) -> &'static str {
    match t {
        Theme::Light => "light",
        Theme::Dark => "dark",
        Theme::System => "system",
    }
}

fn opts(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect()
}

#[component]
fn Appearance() -> Element {
    let p = PREFS.read().clone();
    let font = match p.reply_font {
        ReplyFontSetting::Auto => "auto",
        ReplyFontSetting::Serif => "serif",
        ReplyFontSetting::Sans => "sans",
        ReplyFontSetting::Mono => "mono",
    };
    rsx! {
        Section { id: "appearance", title: "Appearance",
            Row { label: "Theme",
                Segmented {
                    value: theme_value(p.theme),
                    options: opts(&[("light", "Light"), ("dark", "Dark"), ("system", "Auto")]),
                    icons: vec![Icon::Sun, Icon::Moon, Icon::Monitor],
                    label: "Theme",
                    on_change: move |v: String| {
                        let theme = match v.as_str() {
                            "light" => Theme::Light,
                            "dark" => Theme::Dark,
                            _ => Theme::System,
                        };
                        set_prefs(|s| s.theme = theme);
                    },
                }
            }
            Row { label: "Match the agent inside sessions", hint: "Each agent's colours, logo and reply font. Off keeps the neutral look everywhere.",
                div { class: "set-skins",
                    for k in [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode, AgentKind::Copilot] {
                        AgentMark { key: "{k.as_str()}", kind: k, size: 20 }
                    }
                    span { class: "set-skins-toggle",
                        Toggle { on: p.agent_skins, label: "Agent skins", on_change: move |v| set_prefs(|s| s.agent_skins = v) }
                    }
                }
            }
            Row { label: "Reply font", hint: "Auto uses each agent's own: serif for Claude, sans for Codex, mono for OpenCode.",
                Segmented {
                    value: font,
                    options: opts(&[("auto", "Auto"), ("serif", "Serif"), ("sans", "Sans"), ("mono", "Mono")]),
                    label: "Reply font",
                    on_change: move |v: String| {
                        let f = match v.as_str() {
                            "serif" => ReplyFontSetting::Serif,
                            "sans" => ReplyFontSetting::Sans,
                            "mono" => ReplyFontSetting::Mono,
                            _ => ReplyFontSetting::Auto,
                        };
                        set_prefs(|s| s.reply_font = f);
                    },
                }
            }
            Row { label: "Tool calls in chat", hint: "Everything shows each command, file and search with its timing, and background jobs with their live state. Compact folds long runs into a few lines.",
                Segmented {
                    value: if p.tool_detail == ToolDetail::Compact { "compact" } else { "all" },
                    options: opts(&[("all", "Everything"), ("compact", "Compact")]),
                    label: "Tool calls in chat",
                    on_change: move |v: String| set_prefs(|s| s.tool_detail = if v == "compact" { ToolDetail::Compact } else { ToolDetail::All }),
                }
            }
            Row { label: "Haptic feedback", icon: Icon::Vibrate, hint: "Taps, sends and answers buzz on phones (Android, and iPhone with iOS 17.4 or later).",
                Toggle { on: p.haptics, label: "Haptics", on_change: move |v| set_prefs(|s| s.haptics = v) }
            }
        }
    }
}

// ---------- Agents, with your own ----------

#[component]
fn Agents() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let h = hub.read();
    let settings = HUB.read().clone();
    rsx! {
        Section { id: "agents", title: "Agents", desc: "Found on your machines. Inyon picks the right one for each pane automatically. Add your own command to run it as one of these.",
            for d in inyon_protocol::agents().values() {
                {
                    let on: Vec<String> = h.hosts.iter().filter(|x| x.agents.contains(&d.kind)).map(|x| x.config.name.clone()).collect();
                    let line = if on.is_empty() { "Not installed on any machine".to_string() } else { format!("On {}", on.join(", ")) };
                    let cmd = d.launch.split(' ').next().unwrap_or_default().to_string();
                    rsx! {
                        div { key: "{d.kind.as_str()}", class: "set-row",
                            div { class: "set-words set-agent",
                                AgentMark { kind: d.kind, size: 28 }
                                span { class: "set-agent-words",
                                    span { class: "set-agent-name", "{d.name}" }
                                    span { class: "set-agent-line", "{line}" }
                                }
                            }
                            div { class: "set-agent-chips",
                                code { class: "code-chip mono", "{cmd}" }
                                span { class: if d.chat { "set-kind set-kind-chat" } else { "set-kind" }, if d.chat { "Chat view" } else { "Terminal" } }
                            }
                        }
                    }
                }
            }
            if let Some(s) = settings {
                for a in s.custom_agents.clone().unwrap_or_default() {
                    CustomAgentRow { key: "{a.id}", a }
                }
                button { class: "set-action", r#type: "button", onclick: move |_| *EDITING.write() = Some(None),
                    span { class: "set-add", Glyph { icon: Icon::Plus, size: 16 } }
                    "Add an agent"
                }
            }
        }
    }
}

fn agent_menu(a: &CustomAgent) -> Vec<Item> {
    vec![
        Item::new("Edit", Icon::Pencil, Action::Settings(SettingsOp::EditAgent(a.id.clone()))),
        Item::new("Remove", Icon::Trash2, Action::Settings(SettingsOp::RemoveAgent(a.id.clone()))).danger().separated(),
    ]
}

/// One of your own agents, with the same menu on right-click, on a long press and on ⋯.
#[component]
fn CustomAgentRow(a: CustomAgent) -> Element {
    let items = agent_menu(&a);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3) = (press.clone(), press.clone(), press);
    let mut menu_at = use_context::<Signal<Option<menu::Menu>>>();
    let anchor = format!("agent-more-{}", a.id);
    let (more, a2) = (items.clone(), anchor.clone());
    rsx! {
        div {
            class: "set-row set-custom",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            div { class: "set-words set-agent",
                AgentMark { kind: a.like, size: 28 }
                span { class: "set-agent-words",
                    span { class: "set-agent-name truncate", "{a.name}" }
                    span { class: "set-agent-line truncate", "Works like {a.like.def().name}" }
                    code { class: "code-chip mono truncate phone-only-inline", title: "{a.command}", "{a.command}" }
                }
            }
            code { class: "code-chip mono truncate set-custom-cmd", title: "{a.command}", "{a.command}" }
            button {
                id: "{anchor}",
                class: "icon-button press",
                r#type: "button",
                "aria-label": "Options for {a.name}",
                onclick: move |_| menu::drop_down(&mut menu_at, &a2, more.clone()),
                Glyph { icon: Icon::Ellipsis, size: 16 }
            }
        }
    }
}

/// Add an agent, or edit one: a name, the command a new session types, and the agent it works like.
#[component]
fn CustomAgentSheet() -> Element {
    let editing = EDITING();
    let open = editing.is_some();
    let agent = editing.clone().flatten();
    let mut name = use_signal(String::new);
    let mut command = use_signal(String::new);
    let mut like = use_signal(|| AgentKind::Claude);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    // each opening, and each submit, so a save that ends after the sheet closed (or opened again)
    // leaves the sheet alone
    let mut opening = use_signal(|| 0u64);
    let mut submits = use_signal(|| 0u64);
    let key = editing.as_ref().map(|a| a.as_ref().map_or_else(|| "new".to_string(), |a| a.id.clone()));
    {
        let agent = agent.clone();
        use_effect(use_reactive!(|key| {
            *opening.write() += 1;
            if key.is_none() {
                return;
            }
            busy.set(false);
            name.set(agent.as_ref().map(|a| a.name.clone()).unwrap_or_default());
            command.set(agent.as_ref().map(|a| a.command.clone()).unwrap_or_default());
            like.set(agent.as_ref().map_or(AgentKind::Claude, |a| a.like));
            error.set(String::new());
        }));
    }
    let close = move |_: ()| *EDITING.write() = None;
    let old = agent.clone();
    let submit = move |e: FormEvent| {
        e.prevent_default();
        let (n, c) = (name().trim().to_string(), command().trim().to_string());
        if n.is_empty() || c.is_empty() {
            error.set("Give it a name and a command.".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let old = old.clone();
        let l = like();
        let this_opening = *opening.peek();
        *submits.write() += 1;
        let ticket = *submits.peek();
        spawn(async move {
            let next = CustomAgent { id: old.as_ref().map(|a| a.id.clone()).unwrap_or_default(), name: n.clone(), command: c, like: l };
            let id = old.as_ref().map(|a| a.id.clone());
            let saved = save_agents(move |list| match st::with_custom(list, id.as_deref(), next) {
                Some(edited) => {
                    *list = edited;
                    true
                }
                None => false,
            })
            .await;
            let still_open = *opening.peek() == this_opening;
            match saved {
                Ok(true) => {
                    let body = old.is_none().then(|| format!("New sessions can start it, and Inyon reads it as {}.", l.def().name));
                    toast::toast(if old.is_some() { format!("Saved {n}") } else { format!("Added {n}") }, body, ToastTone::Success);
                    if still_open {
                        *EDITING.write() = None;
                    }
                }
                Ok(false) if still_open => error.set("This agent was removed meanwhile, so the edit wasn't saved.".into()),
                Err(e) if still_open => error.set(e),
                _ => {}
            }
            if still_open && *submits.peek() == ticket {
                busy.set(false);
            }
        });
    };
    let title = agent.as_ref().map_or_else(|| "Add an agent".to_string(), |a| format!("Edit {}", a.name));
    let kinds: Vec<Opt> = st::chat_kinds().into_iter().map(|k| Opt { value: k.as_str().into(), label: k.def().name.clone(), hint: None, mark: Some(k) }).collect();
    rsx! {
        Sheet {
            open,
            on_close: close,
            title,
            footer: rsx! {
                div { class: "sheet-actions sheet-actions-end",
                    button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| *EDITING.write() = None, "Cancel" }
                    button { class: "button button-md button-primary press", r#type: "submit", form: "custom-agent-form", disabled: busy(),
                        Busy { on: busy() }
                        if agent.is_some() { "Save" } else { "Add" }
                    }
                }
            },
            form { id: "custom-agent-form", class: "stack", onsubmit: submit,
                Field { label: "Name",
                    input { class: "field", maxlength: 40, placeholder: "Claude YOLO", "data-autofocus": "true", value: "{name}", oninput: move |e| name.set(e.value()) }
                }
                Field { label: "Command", hint: "What a new session types. Inyon also spots it running in any pane.",
                    input {
                        class: "field mono",
                        maxlength: 300,
                        placeholder: "claude --dangerously-skip-permissions",
                        spellcheck: "false",
                        autocapitalize: "off",
                        autocorrect: "off",
                        value: "{command}",
                        oninput: move |e| command.set(e.value()),
                    }
                }
                Field { label: "Works like", hint: "Its logo, chat view, prompts and hooks come from this agent.", group: true,
                    Select {
                        value: like().as_str(),
                        options: kinds,
                        title: "Works like",
                        on_change: move |v: String| {
                            if let Some(k) = st::chat_kinds().into_iter().find(|k| k.as_str() == v) {
                                like.set(k);
                            }
                        },
                    }
                }
                if !error().is_empty() {
                    p { class: "form-error", role: "alert", "{error}" }
                }
            }
        }
    }
}

// ---------- Live numbers, uploads ----------

#[component]
fn LiveNumbers() -> Element {
    let s = HUB.read().clone();
    rsx! {
        Section { id: "sessions", title: "Live numbers", desc: "Plan limits come from each machine's signed-in Claude account, the same numbers as /usage, so they follow account switches by themselves.",
            if let Some(s) = s {
                Row { label: "Restart sessions that stop reporting", hint: "When a session's settings are swapped under it, its cost and effort stop updating. If on, Inyon resumes it once it's idle, with nothing typed and no background shells.",
                    Toggle { on: s.auto_resume.unwrap_or(false), label: "Restart sessions that stop reporting", on_change: move |v| save_hub(|h| h.auto_resume = Some(v)) }
                }
            } else {
                WaitRows { label: "Loading hub settings", count: 1, class: "set-wait" }
            }
            SleepRow {}
        }
    }
}

/// From 0.24: agents idle this long go to sleep (Exit to shell), so they stop using memory.
#[component]
fn SleepRow() -> Element {
    let mut rules = use_signal(|| None::<Vec<inyon_protocol::SleepRule>>);
    use_hook(|| {
        spawn(async move {
            // a hub from before organize v2 has no sleep, and no row
            if let Ok(o) = api::organize().await {
                if o.v >= 2 {
                    rules.set(Some(o.sleep));
                }
            }
        });
    });
    // the choice not yet on the hub; one request at a time, then the latest choice if it's another
    let mut want = use_signal(|| None::<u32>);
    let mut sending = use_signal(|| false);
    let Some(r) = rules() else { return rsx! {} };
    let current = want().unwrap_or_else(|| st::sleep_minutes(&r));
    rsx! {
        Row { label: "Put idle agents to sleep", hint: "An agent idle this long exits to its shell, keeping its pane, and Resume conversation brings it back when you need it. Pinned sessions, ones with a message waiting and ones without a conversation to resume stay awake.",
            Segmented {
                value: current.to_string(),
                options: st::sleep_choices(current),
                label: "Put idle agents to sleep",
                on_change: move |v: String| {
                    want.set(Some(v.parse::<u32>().unwrap_or(0)));
                    if *sending.peek() {
                        return;
                    }
                    sending.set(true);
                    spawn(async move {
                        loop {
                            // read first: a guard in the `while let` would last through the writes
                            let next = *want.peek();
                            let Some(m) = next else { break };
                            let answer = api::set_sleep((m > 0).then_some(m)).await;
                            let latest = *want.peek();
                            if latest != Some(m) {
                                // chosen again meanwhile: that goes next
                                continue;
                            }
                            want.set(None);
                            match answer {
                                Ok(next) => rules.set(Some(next)),
                                Err(e) => toast::toast("Could not change it", Some(e.message), ToastTone::Error),
                            }
                        }
                        sending.set(false);
                    });
                },
            }
        }
    }
}

fn num(v: f64) -> String {
    format!("{}", v as i64)
}

#[component]
fn Uploads(hosts: Vec<HostState>) -> Element {
    let s = HUB.read().clone();
    rsx! {
        section { id: "uploads", class: "set-section",
            h2 { class: "set-title", "Uploads and clipboard" }
            p { class: "set-desc", "Files you attach, paste or drop are copied to ~/.inyon/uploads on the session's machine, and the agent gets their paths." }
            div { class: "set-card",
                if let Some(s) = s {
                    Row { label: "Delete uploads after", hint: "Checked when a machine connects, every few hours, and after an upload. A file cut off half way is deleted after an hour.",
                        Segmented {
                            value: num(s.uploads.retention_days),
                            options: opts(&[("1", "1 day"), ("7", "7 days"), ("30", "30 days"), ("0", "Never")]),
                            label: "Delete uploads after",
                            on_change: move |v: String| save_hub(|h| h.uploads.retention_days = v.parse().unwrap_or(0.0)),
                        }
                    }
                    Row { label: "Keep at most", hint: "Per machine. Past this, the oldest uploads go first.",
                        Segmented {
                            value: num(s.uploads.max_total_mb.unwrap_or(1024.0)),
                            options: opts(&[("512", "500 MB"), ("1024", "1 GB"), ("5120", "5 GB"), ("0", "No limit")]),
                            label: "Keep at most",
                            on_change: move |v: String| save_hub(|h| h.uploads.max_total_mb = Some(v.parse().unwrap_or(0.0))),
                        }
                    }
                    Row { label: "Largest file",
                        Segmented {
                            value: num(s.uploads.max_mb),
                            options: opts(&[("25", "25 MB"), ("100", "100 MB"), ("500", "500 MB")]),
                            label: "Largest file",
                            on_change: move |v: String| save_hub(|h| h.uploads.max_mb = v.parse().unwrap_or(25.0)),
                        }
                    }
                } else {
                    WaitRows { label: "Loading upload settings", count: 3, class: "set-wait" }
                }
            }
            if !hosts.is_empty() {
                div { class: "set-uploads",
                    for h in hosts {
                        super::machines::UploadsCard { key: "{h.config.id}", host: h.clone() }
                    }
                }
            }
            a { class: "set-link", href: Route::Temp { host: None }.href(),
                span { class: "set-words",
                    span { class: "set-label", "Agent temp files" }
                    span { class: "set-hint", "Pasted images, screenshots and scratch files agents leave behind." }
                }
                Glyph { icon: Icon::ChevronRight, size: 16, class: "fg-3" }
            }
        }
    }
}

// ---------- notifications ----------

#[component]
fn Notifications() -> Element {
    let p = PREFS.read().clone();
    let mut perm = use_signal(dom::notice_permission);
    let secure = dom::secure();
    let push = crate::push::supported();
    let allow = move |_| {
        spawn(async move {
            let p = dom::ask_notices().await;
            perm.set(p);
            // with permission, the browser can be told while Inyon is closed too
            if p == Permission::Granted {
                let s = PREFS.peek().clone();
                let _ = crate::push::enable(s.notify_done, s.notify_pages).await;
            }
        });
    };
    let test = move |_| {
        spawn(async move {
            // through the browser's push service, the way they arrive while Inyon is closed
            let s = PREFS.peek().clone();
            if push && crate::push::test(s.notify_done, s.notify_pages).await > 0 {
                toast::toast("Test sent", Some("It went through your browser\u{2019}s push service.".into()), ToastTone::Success);
            } else if dom::show_notice("Inyon", "Notifications work. You\u{2019}ll get one when an agent finishes or needs you.", "test", Some(&Route::Settings.href())) {
                toast::toast("Test notification sent", None, ToastTone::Success);
            } else {
                toast::toast("Couldn't show a notification", Some("Check that notifications are allowed for Inyon in your settings.".into()), ToastTone::Error);
            }
        });
    };
    let pm = perm();
    rsx! {
        Section { id: "notifications", title: "Notifications",
            Row { label: "When an agent finishes", hint: "A notice when a session goes from working to idle. Needs-you alerts are always on.",
                Toggle { on: p.notify_done, label: "Finished notices", on_change: move |v| set_prefs(|s| s.notify_done = v) }
            }
            Row { label: "When a page moves on", hint: "A notice when a task on a page's progress board is done.",
                Toggle { on: p.notify_pages, label: "Page notices", on_change: move |v| set_prefs(|s| s.notify_pages = v) }
            }
            Row { label: "System notifications", icon: Icon::Bell, hint: st::notices_hint(secure, pm, push, dom::home_screen_hint()),
                if secure && pm == Permission::Default {
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: allow, "Allow" }
                }
                if pm == Permission::Granted {
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: test, "Send a test" }
                }
            }
        }
    }
}

// ---------- machines ----------

#[component]
fn Machines(hosts: Vec<HostState>) -> Element {
    let s = HUB.read().clone();
    let drop = super::connect::drop_line(crate::connect::SELF);
    rsx! {
        Section { id: "machines", title: "Machines",
            if let Some(d) = drop {
                Row { label: "This page's connection", hint: d,
                    button { class: "button button-sm button-ghost press", r#type: "button", onclick: move |_| super::connect::open_connect_help(None), "Can't connect?" }
                }
            }
            for h in hosts.iter() {
                MachineLink { key: "{h.config.id}", line: st::machine_line(h, &hosts), h: h.clone() }
            }
            if let Some(s) = s {
                Row { label: "Machines can message agents", hint: "An agent on a joined machine can use inyon send to type into agents on your other machines, each once it's free. Reading sessions and pages needs no switch.",
                    Toggle { on: s.machines_send.unwrap_or(false), label: "Machines can message agents", on_change: move |v| save_hub(|h| h.machines_send = Some(v)) }
                }
            }
        }
    }
}

#[component]
fn MachineLink(h: HostState, line: String) -> Element {
    let items = menu::machine_menu(&h, true);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let now = platform::clock().now;
    let line = crate::machines::host_line(&h, line, now);
    let drop = super::connect::drop_line(&h.config.id);
    rsx! {
        a {
            class: "set-link-row",
            href: Route::Machine { id: h.config.id.clone() }.href(),
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            onclick: move |e| {
                p4.click(&e);
            },
            {super::overview::host_dot(h.status)}
            span { class: "set-words",
                span { class: "set-label truncate set-strong", "{h.config.name}" }
                span { class: "set-hint truncate", "{line}" }
                if let Some(d) = drop {
                    span { class: "set-hint", "{d}" }
                }
            }
            Glyph { icon: Icon::ChevronRight, size: 16, class: "fg-3" }
        }
    }
}

// ---------- other devices, paired devices ----------

#[component]
fn OtherDevices() -> Element {
    let address = dom::origin();
    let local = st::local_address(&address);
    let https = address.starts_with("https:");
    let copy = address.clone();
    rsx! {
        Section { id: "devices", title: "Your other devices", desc: "Everything runs on this hub. Open the same address anywhere you want Inyon.",
            // the Android build has no address of its own to show
            if !address.is_empty() {
                Row {
                label: "Hub address",
                hint: local.then(|| st::LOCAL_HINT.to_string()),
                button {
                    class: "set-address mono press",
                    r#type: "button",
                    onclick: move |_| {
                        let a = copy.clone();
                        spawn(async move {
                            if dom::copy(&a).await {
                                toast::toast("Address copied", None, ToastTone::Success);
                            }
                        });
                    },
                    span { class: "truncate", "{address}" }
                    Glyph { icon: Icon::Copy, size: 14, class: "fg-3" }
                }
                }
            }
            Row {
                label: "In a phone's browser",
                icon: Icon::Smartphone,
                hint: if https { "Open the address while the phone is on the same Tailscale or NetBird network.".to_string() } else { "Open the address while the phone is on the same Tailscale or NetBird network. Adding it to the home screen as an app, with notifications, needs the hub on HTTPS.".to_string() },
            }
        }
    }
}

/// The phones and companion apps paired with this hub, each one revocable, and the codes for the
/// next.
#[component]
fn PairedDevices() -> Element {
    let mut phone_code = use_signal(|| false);
    let mut companion_code = use_signal(|| false);
    use_hook(|| {
        spawn(async move {
            load_phones().await;
            load_companions().await;
        });
    });
    let phones = PHONES();
    let companions = COMPANIONS();
    rsx! {
        Section { id: "paired", title: "Paired devices", desc: "Phones and companion apps that reach this hub. Each has its own key, and removing one ends it at once.",
            Row { label: "Inyon app", icon: Icon::QrCode, hint: "A phone scans a code, then reaches your machines from any network.",
                button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| phone_code.set(true), "Show the code" }
            }
            Row { label: "Companion app", icon: Icon::Mic, hint: "Apps like Legato read your sessions, answer prompts and send text. They type a short code.",
                button { class: "button button-sm button-primary press", r#type: "button", onclick: move |_| companion_code.set(true), "Pair a companion app" }
            }
            GroupLabel { text: "Phones", n: phones.as_ref().map(Vec::len) }
            if let Some(why) = PHONES_FAILED() {
                ReadFailed {
                    text: format!("Couldn't read the paired phones: {why}"),
                    on_retry: move |_| {
                        spawn(load_phones());
                    },
                }
            }
            if phones.as_ref().is_some_and(Vec::is_empty) {
                p { class: "set-empty", "No phones paired." }
            }
            for p in phones.unwrap_or_default() {
                PhoneRow { key: "{p.id}", p }
            }
            GroupLabel { text: "Companion apps", n: companions.as_ref().map(Vec::len) }
            if let Some(why) = COMPANIONS_FAILED() {
                ReadFailed {
                    text: format!("Couldn't read the companion apps: {why}"),
                    on_retry: move |_| {
                        spawn(load_companions());
                    },
                }
            }
            if companions.as_ref().is_some_and(Vec::is_empty) {
                p { class: "set-empty", "No companion apps. Apps like Legato pair with a short code." }
            }
            for c in companions.unwrap_or_default() {
                CompanionRow { key: "{c.id}", c }
            }
        }
        super::phone::PhoneSheet {
            open: phone_code(),
            on_close: move |_| {
                phone_code.set(false);
                spawn(load_phones());
            },
        }
        super::companion::CompanionSheet {
            open: companion_code(),
            on_close: move |_| {
                companion_code.set(false);
                spawn(load_companions());
            },
            on_paired: move |_| {
                spawn(load_companions());
            },
        }
    }
}

#[component]
fn GroupLabel(text: String, n: Option<usize>) -> Element {
    rsx! {
        div { class: "set-group",
            "{text}"
            if let Some(n) = n {
                span { class: "set-group-n tabular", "{n}" }
            }
        }
    }
}

#[component]
fn PhoneRow(p: PairedPhone) -> Element {
    let items = vec![Item::new("Forget", Icon::Trash2, Action::Settings(SettingsOp::ForgetPhone(p.id.clone()))).danger()];
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let seen = p.last_seen.map(|s| format!(" · seen {}", since(s))).unwrap_or_default();
    let line = format!("Inyon app · paired {}{seen}", date(p.paired_at));
    let id = p.id.clone();
    rsx! {
        div {
            class: "set-device",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            Glyph { icon: Icon::Smartphone, size: 16, class: "fg-3 set-device-icon" }
            span { class: "set-words",
                span { class: "set-label truncate set-strong", "{p.name}" }
                span { class: "set-hint truncate mono", "{line}" }
            }
            button {
                class: "button button-sm button-ghost press",
                r#type: "button",
                onclick: move |e| {
                    if !p4.click(&e) {
                        run(SettingsOp::ForgetPhone(id.clone()));
                    }
                },
                "Forget"
            }
        }
    }
}

fn companion_menu(c: &PairedCompanion) -> Vec<Item> {
    vec![
        Item::new("What it can do", Icon::ShieldCheck, Action::Settings(SettingsOp::CompanionDetails(c.id.clone()))),
        Item::new("Remove", Icon::Trash2, Action::Settings(SettingsOp::RemoveCompanion(c.id.clone()))).danger().separated(),
    ]
}

fn companion_line(c: &PairedCompanion) -> String {
    let app = c.app.as_ref().map(|a| format!("{a} · ")).unwrap_or_default();
    format!("{app}paired {} · seen {}", date(c.paired_at as f64), since(c.last_seen as f64))
}

#[component]
fn CompanionRow(c: PairedCompanion) -> Element {
    let items = companion_menu(&c);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3) = (press.clone(), press.clone(), press);
    let mut menu_at = use_context::<Signal<Option<menu::Menu>>>();
    let anchor = format!("companion-more-{}", c.id);
    let (more, a2) = (items.clone(), anchor.clone());
    rsx! {
        div {
            class: "set-device set-device-top",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            Glyph { icon: Icon::Mic, size: 16, class: "fg-3 set-device-icon" }
            span { class: "set-words",
                span { class: "set-name-row",
                    span { class: "set-label truncate set-strong", "{c.name}" }
                    span { class: "chip mono", "contract v{c.contract_version}" }
                }
                span { class: "set-hint mono", {companion_line(&c)} }
                span { class: "set-scopes",
                    for s in c.scopes.iter() {
                        span { key: "{st::scope_label(*s).0}", class: "set-scope", {st::scope_label(*s).0} }
                    }
                }
            }
            button {
                id: "{anchor}",
                class: "icon-button press",
                r#type: "button",
                "aria-label": "Options for {c.name}",
                onclick: move |_| menu::drop_down(&mut menu_at, &a2, more.clone()),
                Glyph { icon: Icon::Ellipsis, size: 16 }
            }
        }
    }
}

/// What a paired companion app may do, and what the hub refuses it everywhere.
#[component]
fn CompanionDetails() -> Element {
    let d = DETAILS();
    // what it showed, kept while the sheet slides away
    let last = use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(None::<PairedCompanion>)));
    if d.is_some() {
        *last.borrow_mut() = d.clone();
    }
    let shown = d.clone().or_else(|| last.borrow().clone());
    let close = move |_: ()| *DETAILS.write() = None;
    let gone = shown.clone();
    rsx! {
        Sheet {
            open: d.is_some(),
            on_close: close,
            title: shown.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
            icon: rsx! { Glyph { icon: Icon::ShieldCheck, size: 16 } },
            footer: rsx! {
                div { class: "sheet-actions",
                    button {
                        class: "button button-md button-danger press",
                        r#type: "button",
                        onclick: move |_| {
                            if let Some(c) = gone.clone() {
                                run(SettingsOp::RemoveCompanion(c.id));
                            }
                        },
                        Glyph { icon: Icon::Trash2, size: 14 }
                        "Remove"
                    }
                    button { class: "button button-md button-primary press", r#type: "button", onclick: move |_| *DETAILS.write() = None, "Done" }
                }
            },
            if let Some(c) = shown {
                div { class: "stack",
                    p { class: "sheet-text fg-3",
                        {format!("{}contract v{} · paired {} · seen {}", c.app.as_ref().map(|a| format!("{a} · ")).unwrap_or_default(), c.contract_version, date(c.paired_at as f64), since(c.last_seen as f64))}
                    }
                    div { class: "set-scope-list",
                        for s in c.scopes.iter() {
                            div { key: "{st::scope_label(*s).0}", class: "set-scope-row",
                                span { class: "set-words",
                                    span { class: "set-label", {st::scope_label(*s).0} }
                                    span { class: "set-hint", {st::scope_label(*s).1} }
                                }
                                span { class: "ok", "aria-label": "Allowed", Glyph { icon: Icon::Check, size: 16 } }
                            }
                        }
                        div { class: "set-scope-row",
                            span { class: "set-words",
                                span { class: "set-label", "Interrupt, terminal, files, settings" }
                                span { class: "set-hint", "The hub refuses these on every route and WebSocket message." }
                            }
                            span { class: "fg-3", "aria-label": "Refused", Glyph { icon: Icon::Minus, size: 16 } }
                        }
                    }
                }
            }
        }
    }
}

// ---------- account ----------

#[component]
fn Account() -> Element {
    let mut auth = use_context::<Signal<Auth>>();
    rsx! {
        Section { id: "account", title: "Account",
            ChangePassword {}
            ActionRow {
                label: "Sign out",
                icon: Icon::LogOut,
                danger: true,
                on_click: move |_| {
                    // signed out only once the hub has ended the sign-in, or a reload would bring it back
                    spawn(async move {
                        match api::logout().await {
                            Ok(()) => auth.set(Auth::SignedOut { setup: false, code: false }),
                            Err(e) => toast::toast("Could not sign out", Some(e.message), ToastTone::Error),
                        }
                    });
                },
            }
        }
    }
}

/// A new password signs out every other browser and phone; this one stays signed in.
#[component]
fn ChangePassword() -> Element {
    let mut open = use_signal(|| false);
    let mut current = use_signal(String::new);
    let mut next = use_signal(String::new);
    let mut again = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut reset = move || {
        open.set(false);
        current.set(String::new());
        next.set(String::new());
        again.set(String::new());
        error.set(String::new());
    };
    if !open() {
        return rsx! {
            ActionRow { label: "Change password", icon: Icon::KeyRound, on_click: move |_| open.set(true) }
        };
    }
    let save = move |e: FormEvent| {
        e.prevent_default();
        error.set(String::new());
        if let Some(why) = st::password_error(&next(), &again()) {
            error.set(why.into());
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::change_password(&current(), &next()).await {
                Ok(()) => {
                    notices::haptic(inyon_chat_core::store::Haptic::Success);
                    toast::toast("Password changed", Some("Other devices need to sign in again.".into()), ToastTone::Success);
                    reset();
                }
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    };
    rsx! {
        form { class: "set-password", onsubmit: save,
            input { class: "field", r#type: "password", autocomplete: "current-password", placeholder: "Current password", autofocus: true, value: "{current}", oninput: move |e| current.set(e.value()) }
            input { class: "field", r#type: "password", autocomplete: "new-password", placeholder: "New password (8+ characters)", value: "{next}", oninput: move |e| next.set(e.value()) }
            input { class: "field", r#type: "password", autocomplete: "new-password", placeholder: "New password again", value: "{again}", oninput: move |e| again.set(e.value()) }
            if !error().is_empty() {
                p { class: "form-error", role: "alert", "{error}" }
            }
            p { class: "set-hint", "Every other browser and phone signed in to this hub will need the new password." }
            div { class: "sheet-actions sheet-actions-end",
                button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| reset(), "Cancel" }
                button { class: "button button-md button-primary press", r#type: "submit", disabled: busy() || current().is_empty() || next().is_empty() || again().is_empty(),
                    Busy { on: busy() }
                    "Change password"
                }
            }
        }
    }
}

// ---------- updates, about ----------

/// The hub: what it runs, what's newer, and the button that updates it.
#[component]
fn HubUpdateRow() -> Element {
    let mut info = use_signal(|| None::<HubUpdate>);
    let mut failed = use_signal(String::new);
    let mut checking = use_signal(|| false);
    // the hub's build when Update was tapped; a different one after a reconnect means it's done
    let mut from = use_signal(|| None::<Build>);
    let mut gen = use_signal(|| 0u64);
    let mut load = move |refresh: bool| {
        checking.set(refresh);
        spawn(async move {
            match api::hub_update(refresh).await {
                Ok(u) => {
                    info.set(Some(u));
                    failed.set(String::new());
                }
                Err(e) => failed.set(e.message),
            }
            checking.set(false);
        });
    };
    use_hook(move || {
        load(false);
        notices::check_build();
    });
    let updating = from().is_some() || info().is_some_and(|i| i.running);
    // while it runs: a failure from the log, or the end without a restart
    use_effect(use_reactive!(|updating| {
        let g = *gen.peek() + 1;
        gen.set(g);
        if !updating {
            return;
        }
        spawn(async move {
            loop {
                platform::sleep(4000).await;
                if *gen.peek() != g {
                    break;
                }
                if let Ok(u) = api::hub_update(false).await {
                    // done: failed, waiting for a restart, or finished with nothing to install
                    if !u.running {
                        from.set(None);
                    }
                    info.set(Some(u));
                }
            }
        });
    }));
    let hub = HUB_BUILD();
    use_effect(use_reactive!(|hub| {
        let (Some(h), Some(f)) = (hub, from.peek().clone()) else { return };
        if st::same_build(&h, &f) {
            return;
        }
        // the row turns to "Up to date", and the reload prompt says the rest
        from.set(None);
        notices::haptic(inyon_chat_core::store::Haptic::Success);
        load(false);
    }));
    let start = move |_| {
        spawn(async move {
            let was = HUB_BUILD
                .peek()
                .clone()
                .unwrap_or_else(|| Build { version: info.peek().as_ref().map_or_else(|| env!("CARGO_PKG_VERSION").to_string(), |i| i.current.clone()), commit: None });
            match api::start_hub_update().await {
                Ok(()) => {
                    from.set(Some(was));
                    if let Some(i) = info.write().as_mut() {
                        i.error = None;
                        i.running = true;
                    }
                }
                Err(e) => toast::toast("Couldn't start the update", Some(e.message), ToastTone::Error),
            }
        });
    };

    let Some(i) = info() else {
        if !failed().is_empty() {
            return rsx! {
                Row { label: "This hub", hint: format!("Couldn't check for updates: {}", failed()),
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| load(true), "Try again" }
                }
            };
        }
        return rsx! { WaitRows { label: "Checking the hub for updates", count: 1, class: "set-wait" } };
    };
    let row = st::update_row(&i, from().is_some());
    rsx! {
        if !updating {
            if let Some(e) = i.error.clone() {
                div { class: "set-error",
                    "The last update didn't finish: {e}"
                    if let Some(l) = i.log.clone() {
                        span { class: "fg-3", " The log is in {l}." }
                    }
                }
            }
        }
        match row {
            UpdateRow::Updating => rsx! {
                Row { label: "Updating the hub…", hint: "It restarts when it's done, and Inyon reconnects by itself. This takes a minute or two.",
                    span { class: "pulse-dot", role: "status", "aria-label": "Updating" }
                }
            },
            UpdateRow::RestartNeeded => rsx! {
                Row { label: "Updated", hint: "Restart the hub to run the new version. It has no systemd unit Inyon could restart." }
            },
            UpdateRow::Available { label, hint, button } => rsx! {
                Row { label, hint,
                    if let Some(b) = button {
                        button { class: "button button-sm button-primary press", r#type: "button", onclick: start, "{b}" }
                    }
                }
            },
            UpdateRow::UpToDate { hint } => rsx! {
                Row { label: "Up to date", hint,
                    button { class: "button button-sm button-ghost press", r#type: "button", disabled: checking(), onclick: move |_| load(true),
                        Busy { on: checking() }
                        "Check now"
                    }
                }
            },
        }
    }
}

/// Which Inyon this is, to check for updates or put in a bug report.
#[component]
fn About() -> Element {
    let build = notices::page_build();
    let version = build.version.clone();
    let commit = build.commit.clone();
    let summary = st::version_summary(&version, commit.as_deref());
    // the browser draws the terminal with xterm.js; the Android app doesn't have one yet
    #[cfg(target_arch = "wasm32")]
    let drawing = Some(crate::term::js::drawing());
    #[cfg(not(target_arch = "wasm32"))]
    let drawing: Option<&str> = None;
    rsx! {
        Section { id: "about", title: "About",
            Row { label: "Version", hint: commit.as_ref().map(|c| format!("Built from commit {c}")),
                span { class: "mono set-value", "{version}" }
            }
            if let Some(d) = drawing {
                Row { label: "Terminal drawing", hint: "WebGL draws on the graphics chip; a device without it uses the page.",
                    span { class: "mono set-value", "{d}" }
                }
            }
            ActionRow {
                label: "Copy version details",
                icon: Icon::Copy,
                on_click: move |_| {
                    let s = summary.clone();
                    spawn(async move {
                        if dom::copy(&s).await {
                            toast::toast("Version copied", None, ToastTone::Success);
                        }
                    });
                },
            }
            ActionRow { label: "How Inyon works", icon: Icon::BookOpen, external: true, on_click: move |_| dom::open_tab(&st::guide_url()) }
            ActionRow { label: "Support Inyon", icon: Icon::Heart, external: true, on_click: move |_| dom::open_tab(st::SPONSOR) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_the_side_list_names_is_on_the_page() {
        let ids: Vec<&str> = NAV.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["appearance", "agents", "companion", "sessions", "uploads", "notifications", "machines", "devices", "paired", "account", "updates", "about"]);
    }

    #[test]
    fn a_custom_agent_and_a_companion_have_the_web_apps_menus() {
        let a = CustomAgent { id: "a1".into(), name: "Claude YOLO".into(), command: "claude --yolo".into(), like: AgentKind::Claude };
        let m = agent_menu(&a);
        assert_eq!(m.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Edit", "Remove"]);
        assert!(m[1].danger && m[1].separated);
        let c = PairedCompanion { id: "c1".into(), name: "Legato".into(), app: None, scopes: vec![], contract_version: 1, paired_at: 0, last_seen: 0 };
        let m = companion_menu(&c);
        assert_eq!(m.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["What it can do", "Remove"]);
        assert_eq!(m[1].action, Action::Settings(SettingsOp::RemoveCompanion("c1".into())));
    }
}
