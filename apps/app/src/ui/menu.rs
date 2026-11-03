//! Right-click menus (ui/ContextMenu.tsx, components/menus.tsx): a right click, or a long press on
//! a touch screen, on anything listed (a machine, a session, a message, a code block, the terminal)
//! opens a short list of what can be done with it; its ⋯ button opens the same list. One menu at a
//! time, drawn over everything by [`MenuLayer`]; ↑/↓ move, Enter picks, Esc closes.

use dioxus::prelude::*;
use inyon_protocol::{HostKind, HostState, HostStatus};

use super::icons::{Glyph, Icon};
use crate::route::Route;
use crate::{dom, machines, platform};

/// What an item does. Kept as data rather than a callback, so the menu still works after the row
/// that opened it has scrolled out of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Copy(String),
    /// send a message that failed again
    Retry {
        host: String,
        pane: String,
        id: String,
        text: String,
    },
    /// drop a message that failed
    Dismiss {
        key: String,
        id: String,
    },
    /// copy, paste or select in the terminal on screen
    Term(crate::term::MenuOp),
    /// a screen inside the app
    Go(Route),
    /// an address in a new tab
    NewTab(String),
    /// the New session sheet for a machine
    NewSession(String),
    /// the Rename (or, for SSH, Edit) sheet for a machine
    EditMachine(String),
    UpdateMachine(String),
    /// asks, then removes a machine from the hub
    RemoveMachine(String),
    /// what can be done to a page (components/pages/shared.tsx's page menu)
    Page(super::pages::PageOp),
    /// a temp item or file (pages/Temp.tsx)
    Temp(super::temp::TempOp),
    /// a custom agent or a paired companion app in Settings
    Settings(super::settings::SettingsOp),
    /// asks, then ends a session: its agent exits and its pane closes (lib/end.ts)
    EndSession {
        host: String,
        pane: String,
    },
    /// the session header's ⋯: Remote Control, compact, rewind, fork, exit…
    Session(super::session_menu::SessionAct),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub label: String,
    pub icon: Icon,
    pub action: Action,
    pub danger: bool,
    /// a hairline above it, starting a new group
    pub separated: bool,
    /// a second line under the label
    pub hint: Option<String>,
    /// one of a few to pick from (a part of a page, a note), and whether it's the one picked
    pub checked: Option<bool>,
    pub disabled: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, icon: Icon, action: Action) -> Item {
        Item { label: label.into(), icon, action, danger: false, separated: false, hint: None, checked: None, disabled: false }
    }

    pub fn copy(label: &str, text: &str) -> Item {
        Item::new(label, Icon::Copy, Action::Copy(text.into()))
    }

    pub fn hint(mut self, h: impl Into<String>) -> Item {
        self.hint = Some(h.into());
        self
    }

    pub fn checked(mut self, on: bool) -> Item {
        self.checked = Some(on);
        self
    }

    pub fn disabled(mut self, off: bool) -> Item {
        self.disabled = off;
        self
    }

    pub fn danger(mut self) -> Item {
        self.danger = true;
        self
    }

    pub fn separated(mut self) -> Item {
        self.separated = true;
        self
    }
}

/// What can be done to a machine from wherever it's listed: its row's right-click menu and the ⋯
/// menu share it, in the web app's order. No new tabs in the phone app, which has none.
pub fn machine_menu(h: &HostState, tabs: bool) -> Vec<Item> {
    let id = h.config.id.clone();
    let page = Route::Machine { id: id.clone() };
    let paired = !(h.config.kind == HostKind::Node && h.config.paired_at.is_none());
    let mut v = vec![Item::new("Open", Icon::Server, Action::Go(page.clone()))];
    if paired && h.status == HostStatus::Online {
        v.push(Item::new("New session", Icon::Plus, Action::NewSession(id.clone())));
    }
    if tabs {
        v.push(Item::new("Open in new tab", Icon::ExternalLink, Action::NewTab(page.href())));
    }
    if paired {
        v.push(Item::new("Temp files", Icon::FolderOpen, Action::Go(Route::Temp { host: Some(id.clone()) })));
    }
    v.push(Item::new(if h.config.kind == HostKind::Ssh { "Edit…" } else { "Rename…" }, Icon::Pencil, Action::EditMachine(id.clone())).separated());
    if let Some(latest) = machines::can_update(h) {
        v.push(Item::new(format!("Update to inyon {latest}"), Icon::Download, Action::UpdateMachine(id.clone())));
    }
    if h.config.kind != HostKind::Local {
        v.push(Item::new("Remove…", Icon::Trash2, Action::RemoveMachine(id)).danger().separated());
    }
    v
}

/// A session listed on a machine's page.
pub fn pane_menu(host: &str, pane: &str, agent: bool, session_id: Option<&str>, tabs: bool) -> Vec<Item> {
    let chat = Route::Session { host: host.into(), pane: pane.into(), view: None };
    let mut v = vec![Item::new("Open", Icon::ArrowRight, Action::Go(chat.clone()))];
    if agent {
        v.push(Item::new(
            "Open terminal",
            Icon::SquareTerminal,
            Action::Go(Route::Session { host: host.into(), pane: pane.into(), view: Some(inyon_chat_core::overview::PaneView::Terminal) }),
        ));
    }
    if tabs {
        v.push(Item::new("Open in new tab", Icon::ExternalLink, Action::NewTab(chat.href())));
    }
    if let Some(id) = session_id {
        v.push(Item::copy("Copy session ID", id).separated());
    }
    v.push(Item::new("End session…", Icon::CircleX, Action::EndSession { host: host.into(), pane: pane.into() }).danger().separated());
    v
}

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    x: f64,
    y: f64,
    items: Vec<Item>,
}

/// An `oncontextmenu` handler that opens a menu with `items` where the pointer is.
pub fn open(items: Vec<Item>) -> impl FnMut(Event<MouseData>) + 'static {
    let mut menu = use_context::<Signal<Option<Menu>>>();
    move |e: Event<MouseData>| {
        if items.is_empty() {
            return;
        }
        e.prevent_default();
        e.stop_propagation();
        let p = e.client_coordinates();
        menu.set(Some(Menu { x: p.x, y: p.y, items: items.clone() }));
    }
}

/// An `oncontextmenu` handler for a menu whose items depend on the moment it opens (the
/// terminal's Copy, there only while text is selected).
pub fn open_with(items: impl Fn() -> Vec<Item> + 'static) -> impl FnMut(Event<MouseData>) + 'static {
    let mut menu = use_context::<Signal<Option<Menu>>>();
    move |e: Event<MouseData>| {
        let items = items();
        if items.is_empty() {
            return;
        }
        e.prevent_default();
        e.stop_propagation();
        let p = e.client_coordinates();
        menu.set(Some(Menu { x: p.x, y: p.y, items }));
    }
}

/// A ⋯ button's menu, under the button with their right edges lined up (or over it near the
/// bottom of the screen). `anchor` is the button's id.
pub fn drop_down(menu: &mut Signal<Option<Menu>>, anchor: &str, items: Vec<Item>) {
    if items.is_empty() {
        return;
    }
    let (x, y) = dom::rect(anchor).map_or((8.0, 8.0), |(_, _, right, bottom)| ((right - 220.0).max(8.0), bottom + 4.0));
    menu.set(Some(Menu { x, y, items }));
}

// ---------- a long press on a touch screen ----------

/// How long a finger rests before the menu opens, and how far it may move meanwhile.
const HOLD_MS: u32 = 500;
const SLOP: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
struct Press {
    n: u64,
    x: f64,
    y: f64,
    /// the menu opened, so the click that follows the lift must not also open the row
    fired: bool,
    /// when the finger came up after the menu opened
    lifted: Option<f64>,
}

/// How long after the lift the click it brings still belongs to the press.
const LIFT_MS: f64 = 600.0;

impl Press {
    /// An event at `now` that this press brings after it opened the menu: the browser's own
    /// long-press context menu (while the finger is down, or as it lifts, depending on the
    /// platform) and the click the lift sends.
    fn owns(&self, now: f64) -> bool {
        self.fired && self.lifted.is_none_or(|t| now - t < LIFT_MS)
    }
}

static PRESS: GlobalSignal<Option<Press>> = Signal::global(|| None);
static PRESSES: GlobalSignal<u64> = Signal::global(|| 0);

/// The handlers that give a row its menu on a long press, for touch screens whose browser doesn't
/// send a right click for one. Put each on the row's matching event.
#[derive(Clone)]
pub struct LongPress {
    menu: Signal<Option<Menu>>,
    items: Vec<Item>,
}

pub fn long_press(items: Vec<Item>) -> LongPress {
    LongPress { menu: use_context::<Signal<Option<Menu>>>(), items }
}

impl LongPress {
    pub fn down(&self, e: &PointerEvent) {
        if e.pointer_type() != "touch" || self.items.is_empty() {
            return;
        }
        let p = e.client_coordinates();
        let n = *PRESSES.peek() + 1;
        *PRESSES.write() = n;
        *PRESS.write() = Some(Press { n, x: p.x, y: p.y, fired: false, lifted: None });
        let mut menu = self.menu;
        let items = self.items.clone();
        spawn(async move {
            platform::sleep(HOLD_MS).await;
            let at = PRESS.peek().clone();
            if let Some(p) = at.filter(|p| p.n == n && !p.fired) {
                if let Some(pr) = PRESS.write().as_mut() {
                    pr.fired = true;
                }
                menu.set(Some(Menu { x: p.x, y: p.y, items }));
            }
        });
    }

    pub fn moved(&self, e: &PointerEvent) {
        let p = e.client_coordinates();
        let far = PRESS.peek().as_ref().is_some_and(|s| !s.fired && ((p.x - s.x).abs() > SLOP || (p.y - s.y).abs() > SLOP));
        if far {
            *PRESS.write() = None;
        }
    }

    /// The finger lifted.
    pub fn up(&self) {
        lifted();
    }

    /// The click after a lift: swallowed when the press opened the menu.
    pub fn click(&self, e: &MouseEvent) -> bool {
        if lift_click() {
            e.prevent_default();
            e.stop_propagation();
            return true;
        }
        false
    }
}

/// The finger came up: a press that didn't open the menu is forgotten, and one that did notes when,
/// so the click the lift brings is told apart from a later tap.
fn lifted() {
    let fired = PRESS.peek().as_ref().map(|p| p.fired);
    match fired {
        Some(false) => *PRESS.write() = None,
        Some(true) => {
            if let Some(p) = PRESS.write().as_mut() {
                p.lifted.get_or_insert(platform::clock().now);
            }
        }
        None => {}
    }
}

/// Whether a click is the one a long press brings when the finger lifts (it lands on whatever the
/// menu now covers); it's used up either way.
fn lift_click() -> bool {
    let owned = PRESS.peek().as_ref().is_some_and(|p| p.owns(platform::clock().now));
    if PRESS.peek().as_ref().is_some_and(|p| p.fired) {
        *PRESS.write() = None;
    }
    owned
}

/// Where a menu of `n` items opens so it stays on a `w`×`h` screen: at the pointer, moved left or
/// up when it would run off the edge.
pub fn place(x: f64, y: f64, n: usize, w: f64, h: f64) -> (f64, f64) {
    const WIDTH: f64 = 220.0;
    let height = 12.0 + 40.0 * n as f64;
    let left = if x + WIDTH + 8.0 > w { (x - WIDTH).max(8.0) } else { x };
    let top = if y + height + 8.0 > h { (y - height).max(8.0) } else { y };
    (left, top)
}

const FIRST: &str = "menu-item-0";

#[component]
pub fn MenuLayer() -> Element {
    let mut menu = use_context::<Signal<Option<Menu>>>();
    let chats = use_context::<Signal<crate::state::Chats>>();
    let here = use_context::<Signal<Route>>();
    let hub = use_context::<Signal<crate::state::Hub>>();
    // the first item takes focus as the menu opens, so the keys work at once
    use_effect(move || {
        if menu.read().is_some() {
            dom::focus(FIRST);
        }
    });
    let Some(m) = menu() else { return rsx! {} };
    let (w, h) = dom::viewport();
    // a hint makes its row a little taller
    let rows = m.items.len() + m.items.iter().filter(|i| i.hint.is_some()).count().div_ceil(2);
    let (left, top) = place(m.x, m.y, rows, w, h);
    let n = m.items.len();
    rsx! {
        div {
            class: "menu-backdrop",
            // a long press opens the menu under the finger, so the lift's click and the browser's
            // own long-press context menu land here; neither closes it
            onpointerup: move |_| lifted(),
            onclick: move |_| {
                if !lift_click() {
                    menu.set(None);
                }
            },
            oncontextmenu: move |e| {
                e.prevent_default();
                let now = platform::clock().now;
                if !PRESS.peek().as_ref().is_some_and(|p| p.owns(now)) {
                    menu.set(None);
                }
            },
        }
        div {
            class: "menu",
            role: "menu",
            style: "left:{left}px;top:{top}px",
            onkeydown: move |e: KeyboardEvent| {
                let at = dom::active_id().and_then(|id| id.strip_prefix("menu-item-").and_then(|i| i.parse::<usize>().ok()));
                match e.key() {
                    Key::Escape => menu.set(None),
                    Key::ArrowDown => dom::focus(&format!("menu-item-{}", at.map_or(0, |i| (i + 1) % n))),
                    Key::ArrowUp => dom::focus(&format!("menu-item-{}", at.map_or(n - 1, |i| (i + n - 1) % n))),
                    _ => return,
                }
                e.prevent_default();
            },
            for (i , item) in m.items.into_iter().enumerate() {
                button {
                    key: "{i}",
                    id: "menu-item-{i}",
                    class: match (item.danger, item.separated && i > 0) {
                        (true, true) => "menu-item menu-danger menu-group",
                        (true, false) => "menu-item menu-danger",
                        (false, true) => "menu-item menu-group",
                        (false, false) => "menu-item",
                    },
                    role: if item.checked.is_some() { "menuitemradio" } else { "menuitem" },
                    "aria-checked": item.checked.map(|on| if on { "true" } else { "false" }),
                    r#type: "button",
                    disabled: item.disabled,
                    onclick: move |_| {
                        menu.set(None);
                        run(item.action.clone(), chats, here, hub);
                    },
                    Glyph { icon: item.icon, size: 16 }
                    if let Some(hint) = &item.hint {
                        span { class: "menu-words",
                            span { class: "truncate", "{item.label}" }
                            span { class: "menu-hint truncate", "{hint}" }
                        }
                    } else {
                        span { class: "menu-words truncate", "{item.label}" }
                    }
                    if item.checked == Some(true) {
                        Glyph { icon: Icon::Check, size: 16, class: "menu-check" }
                    }
                }
            }
        }
    }
}

pub fn run(action: Action, chats: Signal<crate::state::Chats>, mut here: Signal<Route>, hub: Signal<crate::state::Hub>) {
    match action {
        Action::Copy(text) => {
            spawn(async move {
                dom::copy(&text).await;
            });
        }
        Action::Retry { host, pane, id, text } => super::composer::retry(chats, host, pane, id, text),
        Action::Dismiss { key, id } => super::composer::dismiss(chats, &key, &id),
        Action::Term(op) => crate::term::menu(op),
        Action::Go(r) => crate::route::go(&mut here, r),
        Action::NewTab(url) => dom::open_tab(&url),
        Action::NewSession(host) => super::add::open_action(super::add::ActionSheet::NewSession(host)),
        Action::EditMachine(id) => super::add::open_action(super::add::ActionSheet::Edit(id)),
        Action::UpdateMachine(id) => {
            spawn(async move {
                if let Err(e) = crate::api::node_update(&id).await {
                    super::toast::toast("Could not update", Some(e.message), super::toast::ToastTone::Error);
                }
            });
        }
        Action::RemoveMachine(id) => {
            let Some(h) = hub.peek().hosts.iter().find(|h| h.config.id == id).cloned() else { return };
            spawn(async move {
                super::machines::remove(&h).await;
            });
        }
        Action::Page(op) => super::pages::run(op, here, hub),
        Action::Temp(op) => super::temp::run(op),
        Action::Settings(op) => super::settings::run(op),
        Action::Session(act) => super::session_menu::run(act, here),
        Action::EndSession { host, pane } => {
            // from the session's own screen, the Overview after
            let own = matches!(&*here.peek(), Route::Session { host: h, pane: p, .. } if *h == host && *p == pane);
            spawn(async move {
                super::session::end_session(hub, &host, &pane, own.then_some(here)).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_lift_after_a_long_press_does_not_close_its_menu() {
        let held = Press { n: 1, x: 0.0, y: 0.0, fired: true, lifted: None };
        // the browser's own long-press menu and a click while the finger is down belong to the press
        assert!(held.owns(1_000.0));
        let up = Press { lifted: Some(1_000.0), ..held.clone() };
        assert!(up.owns(1_300.0), "the context menu or click the lift brings");
        assert!(!up.owns(1_000.0 + LIFT_MS), "a later tap or right click closes the menu");
        let short = Press { fired: false, ..held };
        assert!(!short.owns(1_000.0), "a press that opened nothing owns nothing");
    }

    #[test]
    fn a_menu_stays_on_the_screen() {
        assert_eq!(place(100.0, 100.0, 2, 390.0, 844.0), (100.0, 100.0));
        // near the right edge it opens to the left of the pointer, near the bottom above it
        assert_eq!(place(300.0, 820.0, 2, 390.0, 844.0), (80.0, 728.0));
        assert_eq!(place(10.0, 10.0, 3, 200.0, 100.0), (8.0, 8.0));
    }

    fn host(v: serde_json::Value) -> HostState {
        let mut base = json!({ "config": { "id": "box", "name": "studio", "kind": "node", "pairedAt": 1 }, "status": "online",
            "integration": "installed", "tmux": "ok", "agents": ["claude"], "sessions": [], "updatedAt": 1 });
        if let (Some(b), Some(o)) = (base.as_object_mut(), v.as_object()) {
            for (k, x) in o {
                b.insert(k.clone(), x.clone());
            }
        }
        serde_json::from_value(base).unwrap()
    }

    fn labels(v: &[Item]) -> Vec<&str> {
        v.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn a_machines_menu_offers_what_can_be_done_with_it_now() {
        let h = host(json!({ "nodeVersion": "0.22.0", "nodeUpdate": { "latest": "0.23.1", "supported": true } }));
        let m = machine_menu(&h, true);
        assert_eq!(labels(&m), ["Open", "New session", "Open in new tab", "Temp files", "Rename…", "Update to inyon 0.23.1", "Remove…"]);
        assert!(m[4].separated && m[6].separated && m[6].danger);
        assert_eq!(m[0].action, Action::Go(Route::Machine { id: "box".into() }));
        assert_eq!(m[2].action, Action::NewTab("/next/machines/box".into()));
        // offline, an SSH server, in the phone app: no new session, Edit, no tab
        let ssh = host(json!({ "config": { "id": "s", "name": "srv", "kind": "ssh", "target": "me@srv" }, "status": "error" }));
        assert_eq!(labels(&machine_menu(&ssh, false)), ["Open", "Temp files", "Edit…", "Remove…"]);
        // this hub can't be removed; a machine that never paired has no sessions or files yet
        let local = host(json!({ "config": { "id": "local", "name": "hub", "kind": "local" } }));
        assert_eq!(labels(&machine_menu(&local, true)), ["Open", "New session", "Open in new tab", "Temp files", "Rename…"]);
        let unpaired = host(json!({ "config": { "id": "n", "name": "new", "kind": "node" }, "status": "connecting" }));
        assert_eq!(labels(&machine_menu(&unpaired, true)), ["Open", "Open in new tab", "Rename…", "Remove…"]);
    }

    #[test]
    fn a_sessions_menu_opens_it_its_terminal_or_a_tab() {
        let m = pane_menu("box", "%3", true, Some("abc-123"), true);
        assert_eq!(labels(&m), ["Open", "Open terminal", "Open in new tab", "Copy session ID", "End session…"]);
        assert_eq!(m[2].action, Action::NewTab("/next/m/box/p/3".into()));
        assert!(m[4].danger && m[4].separated);
        assert_eq!(m[4].action, Action::EndSession { host: "box".into(), pane: "%3".into() });
        assert_eq!(labels(&pane_menu("box", "%4", false, None, false)), ["Open", "End session…"]);
        assert_eq!(machine_menu(&host(json!({})), true)[3].action, Action::Go(Route::Temp { host: Some("box".into()) }));
    }
}
