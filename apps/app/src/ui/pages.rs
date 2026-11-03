//! Pages (pages/Pages.tsx and components/pages/shared.tsx): sites an agent made, listed by machine
//! and project with their live progress, and opened in a sandboxed frame. On a phone the list and a
//! full-screen viewer; from tablet width up the list and the viewer side by side, with device
//! sizes. The list comes from the hub's `pages` stream; links, revoking and adding stay REST. The
//! rows other screens show (a machine's page, a session's pages, a reply's links) are here too.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use inyon_protocol::{MachineKind, PageEntry};

use super::icons::{Glyph, Icon, Logo, MachineGlyph};
use super::kit::{Busy, Field, Segmented, Sheet, WaitRows};
use super::menu::{self, Action, Item};
use super::toast::{self, confirm, Ask, Toast, ToastTone};
use crate::api::{self, ApiError, PageBody};
use crate::pages::{self as pg, Device, PageLink, PageMachine, Part};
use crate::route::{self, Route};
use crate::state::{Hub, PageList};
use crate::{dom, platform};

/// The docs page about Pages, for "How pages work".
pub const PAGES_DOCS: &str = "https://inyon.tech/docs/pages/";
/// The one command an agent runs to add a page, as the empty states show it.
pub const ADD_COMMAND: &str = "inyon pages add ./private/plans/my-plan --name \"My plan\"";
/// What every viewer says about the page in it: a sandbox, and never a place to sign in.
pub const SANDBOX_NOTE: &str = "Sandboxed page: never sign in here";
/// What Open in browser says under it: the link's life, and that the tab is no place to sign in.
pub const BROWSER_HINT: &str = "A 12 h link. Never sign in there";
/// The flags of every page frame, which match the hub's own CSP sandbox: scripts run, on an opaque
/// origin, with no way at the app's cookie, storage or page.
const SANDBOX: &str = "allow-scripts allow-forms allow-popups allow-downloads";
/// Two panes from this width up; below it, the list and a full-screen viewer.
const WIDE_FROM: f64 = 768.0;
const FILTER_ID: &str = "pages-filter";

/// Every page, from the `pages` stream, and what was looked at here.
pub static PAGES: GlobalSignal<PageList> = Signal::global(|| PageList::new(platform::load(pg::SEEN_KEY).as_deref()));
/// Bumped by Revoke (and by opening the page that's open already), so an open viewer mints a new
/// link at once.
static LINK_REV: GlobalSignal<HashMap<String, u64>> = Signal::global(HashMap::new);
/// The app came back into view (window focus, the tab shown again): links near their end renew.
static WAKES: GlobalSignal<u64> = Signal::global(|| 0);
static WIDE: GlobalSignal<bool> = Signal::global(|| dom::viewport().0 >= WIDE_FROM);
/// A menu item for the viewer on screen (Reload, a part, a note), numbered so the same one twice
/// still counts.
static VIEWER_OP: GlobalSignal<Option<(u64, ViewerOp)>> = Signal::global(|| None);
static SHEET: GlobalSignal<Option<PageSheet>> = Signal::global(|| None);
static SHEET_OPEN: GlobalSignal<bool> = Signal::global(|| false);

thread_local! {
    static LISTENING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The page-wide events Pages follows, hooked up the first time a page screen shows.
fn listen() {
    if LISTENING.with(|l| l.replace(true)) {
        return;
    }
    dom::answer_frames();
    dom::on_wake(|| *WAKES.write() += 1);
    dom::on_resize(|| {
        let wide = dom::viewport().0 >= WIDE_FROM;
        if *WIDE.peek() != wide {
            *WIDE.write() = wide;
        }
    });
}

/// What a page menu item does.
#[derive(Debug, Clone, PartialEq)]
pub enum PageOp {
    OpenInBrowser(PageEntry),
    Revoke(PageEntry),
    ShowFolder(PageEntry),
    CopyFolder(String),
    /// takes it off the list; `back` goes back to the list after (the viewer's own Remove)
    Remove {
        page: PageEntry,
        back: bool,
    },
    /// Undo after Remove
    Restore(PageEntry),
    Viewer(ViewerOp),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewerOp {
    Reload,
    Part(Part),
    Note(String),
}

#[derive(Debug, Clone, PartialEq)]
enum PageSheet {
    Folder(Box<PageEntry>),
    Add,
}

fn open_sheet(s: PageSheet) {
    *SHEET.write() = Some(s);
    *SHEET_OPEN.write() = true;
}

/// Mints a new link for a page an open viewer shows, as Reload does.
pub fn renew_link(id: &str) {
    *LINK_REV.write().entry(id.to_string()).or_default() += 1;
}

fn machine(hub: Signal<Hub>, host: &str) -> PageMachine {
    pg::machine_of(&hub.peek().hosts, host)
}

fn page_route(p: &PageEntry) -> Route {
    Route::Pages { page: Some((p.host.clone(), p.slug.clone())) }
}

/// Runs a page menu item. Its work outlives what started it (a toast's Undo goes as it's tapped).
pub fn run(op: PageOp, mut here: Signal<Route>, hub: Signal<Hub>) {
    match op {
        PageOp::OpenInBrowser(p) => open_in_browser(p),
        PageOp::Revoke(p) => {
            dioxus::core::spawn_forever(revoke(p));
        }
        PageOp::ShowFolder(p) => open_sheet(PageSheet::Folder(Box::new(p))),
        PageOp::CopyFolder(path) => {
            dioxus::core::spawn_forever(async move {
                if dom::copy(&path).await {
                    toast::toast("Copied the folder path", None, ToastTone::Success);
                }
            });
        }
        PageOp::Remove { page, back } => {
            let name = machine(hub, &page.host).name;
            dioxus::core::spawn_forever(async move {
                if remove(page, name).await && back {
                    route::replace(&mut here, Route::Pages { page: None });
                }
            });
        }
        PageOp::Restore(p) => {
            dioxus::core::spawn_forever(async move {
                let body = PageBody { path: p.folder.clone(), name: Some(p.name.clone()), slug: Some(p.slug.clone()) };
                if let Err(e) = api::add_page(&p.host, &body).await {
                    toast::toast(format!("Couldn't put “{}” back", p.name), Some(e.message), ToastTone::Error);
                }
            });
        }
        PageOp::Viewer(v) => {
            let n = VIEWER_OP.peek().as_ref().map_or(0, |o| o.0) + 1;
            *VIEWER_OP.write() = Some((n, v));
        }
    }
}

/// Opens a page outside the app, with a new link so it lasts the full 12 hours. A browser blocks a
/// window opened after a wait, so one is opened first and sent to the link once it is minted.
fn open_in_browser(p: PageEntry) {
    let tab = dom::PendingTab::open();
    dioxus::core::spawn_forever(async move {
        match api::open_page(&p.host, &p.slug).await {
            Ok(l) => tab.go(&format!("{}{}", dom::origin(), l.url)),
            Err(e) => {
                tab.close();
                toast::toast(format!("Couldn't open “{}”", p.name), Some(e.message), ToastTone::Error);
            }
        }
    });
}

/// Asks, then ends every link to a page; an open viewer gets a new one right away.
async fn revoke(p: PageEntry) {
    let ask = Ask {
        title: format!("Revoke links to “{}”?", p.name),
        body: Some("Every link to this page stops working: tabs on other devices, links you copied, and the one in an open viewer, which gets a new one right away.".into()),
        yes: "Revoke links".into(),
        danger: true,
        ..Ask::default()
    };
    if !confirm(ask).await {
        return;
    }
    match api::revoke_page(&p.host, &p.slug).await {
        Ok(()) => {
            renew_link(&pg::id_of(&p));
            toast::toast("Links revoked", Some(format!("Old links to “{}” stop working.", p.name)), ToastTone::Success);
        }
        Err(e) => toast::toast("Couldn't revoke the links", Some(e.message), ToastTone::Error),
    }
}

/// Takes a page off the list, with Undo; the folder stays where it is.
async fn remove(p: PageEntry, machine: String) -> bool {
    match api::remove_page(&p.host, &p.slug).await {
        Ok(()) => {
            PAGES.write().drop_page(&p.host, &p.slug);
            toast::show(Toast {
                id: 0,
                title: format!("Removed “{}”", p.name),
                body: Some(format!("The folder stays on {machine}.")),
                tone: ToastTone::Success,
                href: None,
                action: Some(("Undo".into(), Action::Page(PageOp::Restore(p)))),
                agent: None,
            });
            true
        }
        Err(e) => {
            toast::toast(format!("Couldn't remove “{}”", p.name), Some(e.message), ToastTone::Error);
            false
        }
    }
}

/// One menu for a page wherever it's listed: long-press, right-click and ⋯ share it.
pub fn page_menu(p: &PageEntry, m: &PageMachine, open: bool) -> Vec<Item> {
    let mut v = Vec::new();
    if open {
        v.push(Item::new("Open", Icon::PanelsTopLeft, Action::Go(page_route(p))));
    }
    let off = !m.online;
    v.push(
        Item::new("Open in browser", Icon::ExternalLink, Action::Page(PageOp::OpenInBrowser(p.clone())))
            .hint(if off { format!("{} is offline", m.name) } else { BROWSER_HINT.into() })
            .disabled(off),
    );
    v.push(Item::new("Revoke links", Icon::Link2Off, Action::Page(PageOp::Revoke(p.clone()))).separated());
    v.push(Item::new("Show folder", Icon::FolderOpen, Action::Page(PageOp::ShowFolder(p.clone()))));
    v.push(Item::new("Remove", Icon::Trash2, Action::Page(PageOp::Remove { page: p.clone(), back: false })).danger().separated());
    v
}

// ---------- the page ----------

/// `/pages` and `/pages/<machine>/<slug>`.
#[component]
pub fn PagesPage(page: Option<(String, String)>) -> Element {
    use_hook(listen);
    let wide = WIDE();
    dom::set_title("Pages · Inyon");
    if wide {
        return rsx! { DesktopPages { selected: page } };
    }
    match page {
        // keyed by page: picking another page from the viewer starts it afresh
        Some((host, slug)) => rsx! { PhoneViewer { key: "{host}/{slug}", host, slug } },
        None => rsx! { PhoneList {} },
    }
}

/// Whether the screen is a phone viewer, which has the screen to itself (no tab bar).
pub fn full_screen(page: &Option<(String, String)>) -> bool {
    page.is_some() && !WIDE()
}

// ---------- shared by the phone and desktop viewers ----------

#[derive(Debug, Clone, PartialEq)]
struct LinkState {
    id: String,
    link: Option<PageLink>,
    error: Option<ApiError>,
}

/// Asks for a new link; true when one came. One that answers after another page was picked is
/// dropped, and so is one that a newer request overtook (true then, since that one brings the link):
/// an open sent before a revoke may answer after it, with a link the revoke already ended.
async fn mint(host: String, slug: String, mut state: Signal<Option<LinkState>>, shown: Signal<Option<String>>, mut mints: Signal<pg::Latest>) -> bool {
    let id = pg::page_id(&host, &slug);
    let at = platform::clock().now;
    let ticket = mints.write().ask();
    let got = api::open_page(&host, &slug).await;
    if shown.peek().as_deref() != Some(id.as_str()) {
        return false;
    }
    if !mints.peek().is_newest(ticket) {
        return true;
    }
    match got {
        Ok(l) => {
            state.set(Some(LinkState { id, link: Some(pg::local_deadline(&l, at)), error: None }));
            true
        }
        Err(e) => {
            state.with_mut(|s| {
                let link = s.as_ref().filter(|s| s.id == id).and_then(|s| s.link.clone());
                *s = Some(LinkState { id, link, error: Some(e) });
            });
            false
        }
    }
}

/// What a viewer holds: the page's link (minted when it opens, and again an hour before it runs out,
/// by a timer while it's on screen and on coming back into view), the site's parts, and which part
/// or note is showing.
#[derive(Clone, Copy)]
struct Viewer {
    state: Signal<Option<LinkState>>,
    shown: Signal<Option<String>>,
    parts: Signal<Option<(String, Vec<Part>)>>,
    part: Signal<Part>,
    note: Signal<Option<String>>,
    n: Signal<u64>,
    mints: Signal<pg::Latest>,
}

impl Viewer {
    fn link(&self, id: &str) -> Option<PageLink> {
        self.state.read().as_ref().filter(|s| s.id == id).and_then(|s| s.link.clone())
    }

    fn error(&self, id: &str) -> Option<ApiError> {
        self.state.read().as_ref().filter(|s| s.id == id).and_then(|s| s.error.clone())
    }

    /// The parts this site has (the plan always), as found with the current link.
    fn parts(&self, id: &str) -> Vec<Part> {
        self.parts.read().as_ref().filter(|(i, _)| i == id).map_or_else(|| vec![Part::Plan], |(_, v)| v.clone())
    }

    fn part(&self, id: &str) -> Part {
        let want = (self.part)();
        if self.parts(id).contains(&want) {
            want
        } else {
            Part::Plan
        }
    }

    /// A note from the site's research folder shown instead of a part, while it's still listed.
    fn note(&self, p: &PageEntry) -> Option<String> {
        (self.note)().filter(|n| p.notes.as_ref().is_some_and(|v| v.contains(n)))
    }

    fn retry(&self, p: &PageEntry) {
        let (host, slug, state, shown, mints) = (p.host.clone(), p.slug.clone(), self.state, self.shown, self.mints);
        spawn(async move {
            mint(host, slug, state, shown, mints).await;
        });
    }

    /// A new link every time: the one on screen may have been revoked from another device. The new
    /// address reloads the frame; when no link comes the old one is tried again.
    fn reload(&self, p: &PageEntry) {
        let (state, shown, mut n, mints) = (self.state, self.shown, self.n, self.mints);
        let (host, slug) = (p.host.clone(), p.slug.clone());
        spawn(async move {
            if !mint(host, slug, state, shown, mints).await {
                n += 1;
            }
        });
    }

    /// Picking a part always loads it, also the one showing when the page moved on by its own links.
    fn pick_part(&mut self, part: Part) {
        self.part.set(part);
        self.note.set(None);
        self.n += 1;
    }

    fn pick_note(&mut self, name: String) {
        self.note.set(Some(name));
        self.n += 1;
    }
}

fn use_viewer(page: Option<PageEntry>) -> Viewer {
    let state = use_signal(|| None::<LinkState>);
    let mut shown = use_signal(|| None::<String>);
    let mut parts = use_signal(|| None::<(String, Vec<Part>)>);
    let mints = use_signal(pg::Latest::default);
    let mut checks = use_signal(pg::Latest::default);
    let mut v = Viewer { state, shown, parts, part: use_signal(|| Part::Plan), note: use_signal(|| None), n: use_signal(|| 0), mints };
    let key = page.as_ref().map(|p| (p.host.clone(), p.slug.clone()));
    let id = page.as_ref().map(pg::id_of);
    let rev = id.as_ref().map_or(0, |i| LINK_REV.read().get(i).copied().unwrap_or(0));
    // a link when the page opens, and a new one when its links were revoked
    use_effect(use_reactive!(|key, rev| {
        let _ = rev;
        let Some((host, slug)) = key else {
            shown.set(None);
            return;
        };
        let id = pg::page_id(&host, &slug);
        shown.set(Some(id.clone()));
        let mut st = state;
        st.with_mut(|s| match s {
            Some(s) if s.id == id => s.error = None,
            _ => *s = Some(LinkState { id: id.clone(), link: None, error: None }),
        });
        spawn(async move {
            mint(host, slug, state, shown, mints).await;
        });
    }));
    // renewed on time by a timer, since a page left on screen (a second monitor, a tablet on a
    // stand) gets no focus event; a failed try is tried again with a growing wait
    let until = id.as_ref().and_then(|i| v.link(i)).map(|l| l.until);
    let mut gen = use_signal(|| 0u64);
    let timer_key = key.clone();
    use_effect(use_reactive!(|until, timer_key| {
        let g = *gen.peek() + 1;
        gen.set(g);
        let (Some(until), Some((host, slug))) = (until, timer_key) else { return };
        spawn(async move {
            let mut wait = pg::renew_in(until, platform::clock().now);
            let mut retry = pg::RENEW_RETRY_MS;
            loop {
                platform::sleep(wait.min(u32::MAX as f64) as u32).await;
                // coming back into view renews it
                if *gen.peek() != g || !dom::visible() {
                    return;
                }
                if mint(host.clone(), slug.clone(), state, shown, mints).await || *gen.peek() != g {
                    return;
                }
                wait = retry;
                retry = pg::next_retry(retry);
            }
        });
    }));
    let wakes = WAKES();
    let wake_key = key.clone();
    use_effect(use_reactive!(|wakes, until, wake_key| {
        let _ = wakes;
        if let (Some(until), Some((host, slug))) = (until, wake_key) {
            if pg::should_renew(until, platform::clock().now) {
                spawn(async move {
                    mint(host, slug, state, shown, mints).await;
                });
            }
        }
    }));
    // which parts of a Blueprint site exist, asked of the hub with each new link
    let url = id.as_ref().and_then(|i| v.link(i)).map(|l| l.url);
    let parts_id = id.clone();
    use_effect(use_reactive!(|parts_id, url| {
        let (Some(id), Some(url)) = (parts_id, url) else { return };
        // a check for an older link that finishes last mustn't replace this one's answer
        let ticket = checks.write().ask();
        spawn(async move {
            let mut found = vec![Part::Plan];
            for part in [Part::Mockups, Part::Matrix] {
                if api::page_file_exists(&format!("{url}{}", part.path())).await {
                    found.push(part);
                }
            }
            if shown.peek().as_deref() == Some(id.as_str()) && checks.peek().is_newest(ticket) {
                parts.set(Some((id, found)));
            }
        });
    }));
    // looking at a page clears its changed mark, also when it changes while it's open
    let changed = page.as_ref().map_or(0.0, pg::changed_at);
    let seen_page = page.clone();
    use_effect(use_reactive!(|id, changed| {
        let _ = (id, changed);
        if let Some(p) = &seen_page {
            let saved = PAGES.write().mark_seen(p, platform::clock().now);
            if let Some(json) = saved {
                platform::save(pg::SEEN_KEY, &json);
            }
        }
    }));
    // a menu item for this viewer (Reload, a part, a note); one from before it opened is old news
    let start = use_hook(|| VIEWER_OP.peek().as_ref().map_or(0, |o| o.0));
    let op = VIEWER_OP();
    let op_page = page.clone();
    use_effect(use_reactive!(|op| {
        let (Some((n, op)), Some(p)) = (op, op_page.as_ref()) else { return };
        if n <= start {
            return;
        }
        match op {
            ViewerOp::Reload => v.reload(p),
            ViewerOp::Part(part) => v.pick_part(part),
            ViewerOp::Note(name) => v.pick_note(name),
        }
    }));
    v
}

/// The frame's address and key: the theme isn't part of its identity (a theme change is a
/// message, not a reload), so it's read when the address is made.
fn use_frame_src(v: Viewer, page: Option<&PageEntry>) -> Option<(String, String)> {
    let id = page.map(pg::id_of);
    let url = id.as_ref().and_then(|i| v.link(i)).map(|l| l.url);
    let part = id.as_ref().map_or(Part::Plan, |i| v.part(i));
    let note = page.and_then(|p| v.note(p));
    let src = use_memo(use_reactive!(|url, part, note| url.map(|u| match &note {
        Some(n) => pg::note_src("", &u, n, dom::theme()),
        None => pg::frame_src("", &u, part, dom::theme()),
    })));
    let n = (v.n)();
    src().map(|s| (s.clone(), format!("{s}#{n}")))
}

/// The page in its sandboxed frame. The theme goes in with ?theme= in the address, and as a message
/// when the app's theme changes, the frame loads a new document, or the page asks.
#[component]
fn PageFrame(src: String, title: String, #[props(default)] class: String) -> Element {
    let id = use_hook(|| format!("frame-{}", dom::random_id()));
    let frame = id.clone();
    rsx! {
        iframe {
            id: "{id}",
            class: "page-frame {class}",
            src: "{src}",
            title: "{title}",
            "sandbox": SANDBOX,
            "referrerpolicy": "no-referrer",
            "allow": "",
            onload: move |_| dom::post_theme(&frame),
        }
    }
}

/// The site's research notes, and the way back to the page while one is open.
fn notes_menu(p: &PageEntry, part: Part, note: Option<&str>, linked: bool) -> Vec<Item> {
    let mut v = Vec::new();
    if note.is_some() {
        v.push(Item::new(format!("Back to {}", part.label()), part_icon(part), Action::Page(PageOp::Viewer(ViewerOp::Part(part)))));
    }
    for (i, name) in p.notes.iter().flatten().enumerate() {
        let mut item =
            Item::new(pg::note_title(name), Icon::FileText, Action::Page(PageOp::Viewer(ViewerOp::Note(name.clone())))).checked(note == Some(name.as_str())).disabled(!linked);
        if note.is_some() && i == 0 {
            item = item.separated();
        }
        v.push(item);
    }
    v
}

fn part_icon(p: Part) -> Icon {
    match p {
        Part::Plan => Icon::FileText,
        Part::Mockups => Icon::LayoutTemplate,
        Part::Matrix => Icon::Table2,
    }
}

#[component]
fn StateCard(icon: Icon, #[props(default)] danger: bool, title: String, children: Element) -> Element {
    rsx! {
        div { class: "state-card fade-in", role: "status",
            span { class: if danger { "state-icon danger" } else { "state-icon" }, Glyph { icon, size: 20 } }
            h2 { class: "state-title", "{title}" }
            div { class: "state-text", {children} }
        }
    }
}

/// What the stage shows while there's no frame: opening, gone, or out of reach.
#[component]
fn FrameState(page: PageEntry, m: PageMachine, error: Option<ApiError>, on_retry: EventHandler<()>, on_back: EventHandler<()>) -> Element {
    if !m.online {
        let folder = page.folder.clone();
        return rsx! {
            StateCard { icon: Icon::CloudOff, danger: true, title: "{m.name} is offline",
                "“{page.name}” opens again as soon as it's back."
                div { class: "state-actions",
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| on_retry.call(()),
                        Glyph { icon: Icon::RotateCw, size: 14 }
                        "Try again"
                    }
                    button { class: "button button-sm button-ghost press", r#type: "button",
                        onclick: move |_| run_copy(folder.clone()),
                        Glyph { icon: Icon::Copy, size: 14 }
                        "Copy folder path"
                    }
                }
            }
        };
    }
    let Some(e) = error else {
        return rsx! { WaitRows { label: "Opening “{page.name}”", count: 2, class: "state-wait" } };
    };
    let gone = e.status == 404;
    rsx! {
        StateCard { icon: if gone { Icon::FolderOpen } else { Icon::CloudOff }, title: if gone { "This page is gone".to_string() } else { format!("Couldn't open “{}”", page.name) },
            if gone {
                "It was taken off the list on {m.name}."
            } else {
                "{e.message}"
            }
            div { class: "state-actions",
                if gone {
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| on_back.call(()),
                        Glyph { icon: Icon::ChevronLeft, size: 14 }
                        "Back to Pages"
                    }
                } else {
                    button { class: "button button-sm button-quiet press", r#type: "button", onclick: move |_| on_retry.call(()),
                        Glyph { icon: Icon::RotateCw, size: 14 }
                        "Try again"
                    }
                }
            }
        }
    }
}

fn run_copy(path: String) {
    spawn(async move {
        if dom::copy(&path).await {
            toast::toast("Copied the folder path", None, ToastTone::Success);
        }
    });
}

/// The command an agent runs, with a copy button.
#[component]
fn AddCommand() -> Element {
    rsx! {
        div { class: "add-command",
            code { class: "mono add-command-text", "{ADD_COMMAND}" }
            button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Copy command", title: "Copy command",
                onclick: move |_| {
                    spawn(async move {
                        if dom::copy(ADD_COMMAND).await {
                            toast::toast("Copied the command", None, ToastTone::Success);
                        }
                    });
                },
                Glyph { icon: Icon::Copy, size: 16 }
            }
        }
    }
}

#[component]
fn NoPages(#[props(default)] wide: bool) -> Element {
    rsx! {
        div { class: if wide { "no-pages no-pages-wide fade-in" } else { "no-pages panel fade-in" },
            if wide {
                span { class: "state-icon", Glyph { icon: Icon::PanelsTopLeft, size: 18 } }
            }
            h2 { class: "no-pages-title", "No pages yet" }
            p { class: "no-pages-text",
                "When an agent builds a plan or mockups, it runs one command and the page shows up here"
                if wide { ", under its machine and project" }
                "."
            }
            AddCommand {}
            div { class: "no-pages-actions",
                button {
                    class: if wide { "button button-md button-primary press" } else { "button button-lg button-quiet button-block press" },
                    r#type: "button",
                    onclick: move |_| open_sheet(PageSheet::Add),
                    Glyph { icon: Icon::FolderPlus, size: 16 }
                    "Add a folder"
                }
                if wide {
                    a { class: "button button-md button-ghost press", href: PAGES_DOCS, target: "_blank", rel: "noreferrer",
                        Glyph { icon: Icon::BookOpen, size: 16 }
                        "How pages work ↗"
                    }
                }
            }
        }
    }
}

#[component]
fn MachineHeading(m: PageMachine, #[props(default)] small: bool) -> Element {
    let kind = m.known.as_ref().map_or(MachineKind::Server, |k| k.1);
    rsx! {
        div { class: if small { "machine-heading machine-heading-sm" } else { "machine-heading" },
            MachineGlyph { kind, size: if small { 14 } else { 16 }, class: "fg-2" }
            span { class: "truncate", "{m.name}" }
            if m.hub {
                span { class: "chip chip-quiet mono", "hub" }
            }
            if !m.online {
                span { class: "machine-heading-off mono",
                    Glyph { icon: Icon::CloudOff, size: 14 }
                    "offline"
                }
            }
        }
    }
}

/// A thin done/total bar; it moves with transform only.
#[component]
pub fn ProgressBar(done: u32, total: u32, #[props(default)] class: String) -> Element {
    let f = if total > 0 { (done as f64 / total as f64).min(1.0) } else { 0.0 };
    rsx! {
        div {
            class: "progress {class}",
            role: "progressbar",
            "aria-label": "{done} of {total} done",
            "aria-valuemin": "0",
            "aria-valuemax": "{total}",
            "aria-valuenow": "{done}",
            div { class: "progress-fill", style: "transform:scaleX({f})" }
        }
    }
}

/// The changed-since-you-opened-it dot.
#[component]
fn ChangedDot() -> Element {
    rsx! { span { class: "changed-dot", role: "img", "aria-label": "changed", title: "Changed since you last opened it" } }
}

/// The pages that match the filter, grouped.
fn grouped(list: &[PageEntry], hub: &Hub, q: &str, running: &HashMap<String, String>) -> Vec<pg::PageGroup> {
    let name = |h: &str| pg::machine_of(&hub.hosts, h).name;
    let shown: Vec<&PageEntry> = list.iter().filter(|p| pg::matches_page(p, &name(&p.host), q, running)).collect();
    pg::group_pages(&shown, name, running)
}

#[component]
fn FilterInput(q: Signal<String>, placeholder: String, #[props(default)] class: String) -> Element {
    let value = q();
    rsx! {
        label { class: "search filter {class}",
            Glyph { icon: Icon::Search, size: 16, class: "fg-3" }
            input {
                id: FILTER_ID,
                class: "search-input",
                r#type: "search",
                value: "{value}",
                placeholder: "{placeholder}",
                "aria-label": "{placeholder}",
                autocapitalize: "off",
                spellcheck: "false",
                oninput: move |e| q.set(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Escape {
                        q.set(String::new());
                    }
                },
            }
        }
    }
}

// ---------- phone: the list ----------

#[component]
fn PhoneList() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut filtering = use_signal(|| false);
    let mut q = use_signal(String::new);
    let list = PAGES.read();
    let h = hub.read();
    let running = pg::running_names(&h.hosts);
    let query = q();
    let groups = list.pages.as_deref().map(|v| grouped(v, &h, &query, &running)).unwrap_or_default();
    let any = list.pages.as_ref().is_some_and(|v| !v.is_empty());
    rsx! {
        div { class: "scroll",
            header { class: "topbar",
                div { class: "topbar-row page-head",
                    span { class: "phone-only", Logo {} }
                    h1 { class: "page-title truncate", "Pages" }
                    if any {
                        span { class: "page-actions",
                            button {
                                class: if filtering() { "icon-button icon-button-lg press icon-button-on" } else { "icon-button icon-button-lg press" },
                                r#type: "button",
                                "aria-label": "Filter pages",
                                "aria-pressed": "{filtering()}",
                                onclick: move |_| {
                                    let on = !filtering();
                                    filtering.set(on);
                                    q.set(String::new());
                                    if on {
                                        spawn(async move {
                                            platform::sleep(20).await;
                                            dom::focus(FILTER_ID);
                                        });
                                    }
                                },
                                Glyph { icon: Icon::Search, size: 18 }
                            }
                            button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Add a folder", onclick: move |_| open_sheet(PageSheet::Add),
                                Glyph { icon: Icon::FolderPlus, size: 18 }
                            }
                        }
                    }
                }
            }
            div { class: "pages-phone",
                if filtering() {
                    FilterInput { q, placeholder: "Filter pages" }
                }
                match list.pages.as_deref() {
                    None => rsx! { WaitRows { label: "Loading pages", count: 2 } },
                    Some([]) => rsx! { NoPages {} },
                    Some(_) if groups.is_empty() => rsx! {
                        div { class: "no-match",
                            "No page matches “{query}”."
                            div {
                                button { class: "button button-sm button-quiet press no-match-clear", r#type: "button", onclick: move |_| q.set(String::new()), "Clear filter" }
                            }
                        }
                    },
                    Some(_) => rsx! {
                        for g in groups {
                            {
                                let m = pg::machine_of(&h.hosts, &g.host);
                                rsx! {
                                    section { key: "{g.host}", class: "pages-machine",
                                        MachineHeading { m: m.clone() }
                                        for pr in g.projects {
                                            div { key: "{pr.key}",
                                                div { class: "pages-project mono", "{pr.name}" }
                                                div { class: "page-cards",
                                                    for p in pr.pages {
                                                        PageCard { key: "{p.host}/{p.slug}", changed: pg::is_changed(&p, &list.seen), p, m: m.clone() }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn PageCard(p: PageEntry, m: PageMachine, changed: bool) -> Element {
    let mut here = use_context::<Signal<Route>>();
    let mut menu_at = use_context::<Signal<Option<menu::Menu>>>();
    let items = page_menu(&p, &m, true);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let more_id = use_hook(|| format!("more-{}", dom::random_id()));
    let label = pg::page_label(&p, changed, m.online, platform::clock());
    let (route_to, name, online) = (page_route(&p), m.name.clone(), m.online);
    let open = move || {
        if online {
            route::go(&mut here, route_to.clone());
        } else {
            toast::toast(format!("{name} is offline"), Some("This page opens when it’s back.".into()), ToastTone::Info);
        }
    };
    let mut open_k = open.clone();
    let mut open_c = open;
    let more_items = items.clone();
    let anchor = more_id.clone();
    let dim = if m.online { "" } else { " dim" };
    rsx! {
        div {
            class: "page-card press",
            role: "link",
            tabindex: "0",
            "aria-label": "{label}",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            onclick: move |e| {
                if !p4.click(&e) {
                    open_c();
                }
            },
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Enter {
                    open_k();
                }
            },
            div { class: "page-card-head{dim}",
                b { class: "page-card-name truncate", "{p.name}" }
                if changed {
                    ChangedDot {}
                }
                button {
                    id: "{more_id}",
                    class: "icon-button icon-button-lg press page-card-more",
                    r#type: "button",
                    "aria-label": "More for {p.name}",
                    // Enter here opens the menu, not the card's page
                    onkeydown: move |e: KeyboardEvent| e.stop_propagation(),
                    onclick: move |e| {
                        e.stop_propagation();
                        menu::drop_down(&mut menu_at, &anchor, more_items.clone());
                    },
                    Glyph { icon: Icon::Ellipsis, size: 18 }
                }
            }
            if let Some(g) = &p.progress {
                div { class: "page-card-progress{dim}",
                    div { class: "page-card-bar",
                        ProgressBar { done: g.done, total: g.total, class: "progress-fill-row" }
                        span { class: "mono tabular fg-2", "{g.done}/{g.total}" }
                    }
                    if let Some(now) = g.now.as_deref().filter(|n| !n.is_empty()) {
                        p { class: "page-card-now", "{now}" }
                    }
                }
            }
            div { class: if m.online { "page-card-foot mono fg-3" } else { "page-card-foot mono fg-2" },
                if m.online {
                    {pg::updated_line(&p, platform::clock())}
                } else {
                    Glyph { icon: Icon::CloudOff, size: 14 }
                    "Machine offline"
                }
            }
        }
    }
}

// ---------- phone: the viewer ----------

#[component]
fn PhoneViewer(host: String, slug: String) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut menu_at = use_context::<Signal<Option<menu::Menu>>>();
    let list = PAGES.read();
    let page = list.find(&host, &slug).cloned();
    let loaded = list.pages.is_some();
    let siblings: Vec<PageEntry> = list.pages.iter().flatten().filter(|p| p.host == host).cloned().collect();
    drop(list);
    let v = use_viewer(page.clone());
    let src = use_frame_src(v, page.as_ref());
    // back closes the viewer; it never walks back through the pages opened inside the frame
    let back = move |_| route::replace(&mut here, Route::Pages { page: None });
    let m = pg::machine_of(&hub.read().hosts, &host);
    let id = pg::page_id(&host, &slug);
    let link = v.link(&id);
    let parts = v.parts(&id);
    let part = v.part(&id);
    let note = page.as_ref().and_then(|p| v.note(p));
    let (machine_id, more_id, notes_id) = use_hook(|| (format!("mach-{}", dom::random_id()), format!("more-{}", dom::random_id()), format!("notes-{}", dom::random_id())));
    let Some(p) = page else {
        return rsx! {
            div { class: "viewer-phone",
                header { class: "viewer-head",
                    div { class: "viewer-head-row",
                        button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Back to Pages", onclick: back,
                            Glyph { icon: Icon::ChevronLeft, size: 20 }
                        }
                        h1 { class: "viewer-title truncate" }
                    }
                }
                div { class: "viewer-stage",
                    if loaded {
                        StateCard { icon: Icon::FolderOpen, title: "This page is gone",
                            "It isn't on the list any more."
                            div { class: "state-actions",
                                button { class: "button button-sm button-quiet press", r#type: "button", onclick: back,
                                    Glyph { icon: Icon::ChevronLeft, size: 14 }
                                    "Back to Pages"
                                }
                            }
                        }
                    } else {
                        WaitRows { label: "Opening", count: 2, class: "state-wait" }
                    }
                }
            }
        };
    };
    let linked = link.is_some();
    let mut more: Vec<Item> = vec![Item::new("Reload", Icon::RotateCw, Action::Page(PageOp::Viewer(ViewerOp::Reload))).disabled(!linked)];
    if parts.len() > 1 {
        for (i, pt) in parts.iter().enumerate() {
            let item = Item::new(pt.label(), part_icon(*pt), Action::Page(PageOp::Viewer(ViewerOp::Part(*pt)))).checked(note.is_none() && part == *pt).disabled(!linked);
            more.push(if i == 0 { item.separated() } else { item });
        }
    }
    more.push(Item::new("Open in browser", Icon::ExternalLink, Action::Page(PageOp::OpenInBrowser(p.clone()))).hint(BROWSER_HINT).separated().disabled(!linked));
    let mut machine_items: Vec<Item> = siblings.iter().map(|s| Item::new(s.name.clone(), Icon::FileText, Action::Go(page_route(s))).checked(s.slug == p.slug)).collect();
    if let Some((mid, _)) = &m.known {
        machine_items.push(Item::new(format!("Open {}", m.name), Icon::Server, Action::Go(Route::Machine { id: mid.clone() })).separated());
    }
    let notes = notes_menu(&p, part, note.as_deref(), linked);
    let has_notes = p.notes.as_ref().is_some_and(|n| !n.is_empty());
    let kind = m.known.as_ref().map_or(MachineKind::Server, |k| k.1);
    let (a1, a2, a3) = (machine_id.clone(), more_id.clone(), notes_id.clone());
    let pr = p.clone();
    let mut v2 = v;
    let shows_frame = m.online;
    rsx! {
        div { class: "viewer-phone",
            header { class: "viewer-head",
                div { class: "viewer-head-row",
                    button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Back to Pages", onclick: back,
                        Glyph { icon: Icon::ChevronLeft, size: 20 }
                    }
                    h1 { class: "viewer-title truncate", "{p.name}" }
                    button {
                        id: "{machine_id}",
                        class: "machine-chip mono press",
                        r#type: "button",
                        "aria-label": "Machine: {m.name}",
                        onclick: move |_| menu::drop_down(&mut menu_at, &a1, machine_items.clone()),
                        MachineGlyph { kind, size: 14 }
                        span { class: "truncate", "{m.name}" }
                    }
                    if has_notes {
                        button {
                            id: "{notes_id}",
                            class: if note.is_some() { "icon-button icon-button-lg press icon-button-on" } else { "icon-button icon-button-lg press" },
                            r#type: "button",
                            "aria-label": if let Some(n) = &note { format!("Notes: {} open", pg::note_title(n)) } else { "Notes".to_string() },
                            onclick: move |_| menu::drop_down(&mut menu_at, &a3, notes.clone()),
                            Glyph { icon: Icon::NotebookText, size: 18 }
                        }
                    }
                    button { id: "{more_id}", class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "More",
                        onclick: move |_| menu::drop_down(&mut menu_at, &a2, more.clone()),
                        Glyph { icon: Icon::Ellipsis, size: 18 }
                    }
                }
                // always on screen, so a page can't pass itself off as the app asking for a password
                div { class: "sandbox-line mono",
                    Glyph { icon: Icon::ShieldCheck, size: 14 }
                    span { class: "truncate", "{SANDBOX_NOTE}" }
                }
                if let Some(n) = &note {
                    div { class: "note-line",
                        Glyph { icon: Icon::NotebookText, size: 14, class: "fg-3" }
                        span { class: "truncate", {pg::note_title(n)} }
                        button { class: "note-back press", r#type: "button", onclick: move |_| v2.pick_part(part), "Back to {part.label()}" }
                    }
                }
            }
            div { class: "viewer-stage",
                if let (Some((s, k)), true) = (src, shows_frame) {
                    PageFrame { key: "{k}", src: s, title: p.name.clone(), class: "viewer-frame-full" }
                } else {
                    FrameState { page: p.clone(), m: m.clone(), error: v.error(&id), on_retry: move |_| v.retry(&pr), on_back: move |_| route::replace(&mut here, Route::Pages { page: None }) }
                }
            }
        }
    }
}

// ---------- desktop: list and viewer side by side ----------

#[component]
fn DesktopPages(selected: Option<(String, String)>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let q = use_signal(String::new);
    let mut shut = use_signal(HashSet::<String>::new);
    use_hook(|| dom::slash_focuses(Some(FILTER_ID)));
    use_drop(|| dom::slash_focuses(None));
    let list = PAGES.read();
    let h = hub.read();
    let running = pg::running_names(&h.hosts);
    let query = q();
    let all = list.pages.as_deref().map(|v| grouped(v, &h, "", &running)).unwrap_or_default();
    let groups = if query.trim().is_empty() { all.clone() } else { list.pages.as_deref().map(|v| grouped(v, &h, &query, &running)).unwrap_or_default() };
    let count = list.pages.as_ref().map_or(0, Vec::len);
    // nothing picked: open the page that changed last (or the first) instead of an empty pane
    let first: Vec<PageEntry> = all.iter().flat_map(|g| g.projects.iter().flat_map(|p| p.pages.iter().cloned())).collect();
    let pick = first.iter().find(|p| pg::is_changed(p, &list.seen)).or(first.first()).map(|p| (p.host.clone(), p.slug.clone()));
    let none_picked = selected.is_none();
    use_effect(use_reactive!(|none_picked, count, pick| {
        if none_picked && count > 0 {
            if let Some(p) = pick {
                route::replace(&mut here, Route::Pages { page: Some(p) });
            }
        }
    }));
    let page = selected.as_ref().and_then(|(host, slug)| list.find(host, slug).cloned());
    let seen = list.seen.clone();
    let loaded = list.pages.is_some();
    drop(list);
    let mut q2 = q;
    rsx! {
        div { class: "pages-desktop",
            section { class: "pages-tree", "aria-label": "Pages",
                div { class: "pages-tree-head",
                    h1 { class: "pages-tree-title", "Pages" }
                    button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Add a folder", title: "Add a folder", onclick: move |_| open_sheet(PageSheet::Add),
                        Glyph { icon: Icon::FolderPlus, size: 16 }
                    }
                }
                if count > 0 {
                    div { class: "pages-tree-filter",
                        FilterInput { q, placeholder: "Filter", class: "filter-sm" }
                    }
                }
                div { class: "pages-tree-list",
                    if !loaded {
                        WaitRows { label: "Loading pages", count: 3 }
                    } else if count == 0 {
                        p { class: "pages-tree-empty", "Nothing here yet." }
                    } else if groups.is_empty() {
                        div { class: "pages-tree-empty",
                            "No page matches “{query}”."
                            div {
                                button { class: "button button-sm button-quiet press no-match-clear", r#type: "button", onclick: move |_| q2.set(String::new()), "Clear filter" }
                            }
                        }
                    } else {
                        for g in groups {
                            {
                                let m = pg::machine_of(&h.hosts, &g.host);
                                rsx! {
                                    div { key: "{g.host}", class: "tree-machine",
                                        MachineHeading { m: m.clone(), small: true }
                                        for pr in g.projects {
                                            {
                                                let closed = shut.read().contains(&pr.key) && query.trim().is_empty();
                                                let key = pr.key.clone();
                                                rsx! {
                                                    div { key: "{pr.key}",
                                                        button {
                                                            class: "tree-group",
                                                            r#type: "button",
                                                            "aria-expanded": "{!closed}",
                                                            onclick: move |_| {
                                                                let mut s = shut.write();
                                                                if !s.remove(&key) {
                                                                    s.insert(key.clone());
                                                                }
                                                            },
                                                            Glyph { icon: if closed { Icon::ChevronRight } else { Icon::ChevronDown }, size: 14, class: "fg-3" }
                                                            span { class: "truncate tree-group-name", "{pr.name}" }
                                                            span { class: "mono fg-3 tree-group-count", "{pr.pages.len()}" }
                                                        }
                                                        if !closed {
                                                            for p in pr.pages {
                                                                TreeRow {
                                                                    key: "{p.host}/{p.slug}",
                                                                    active: selected.as_ref().is_some_and(|(h, s)| *h == p.host && *s == p.slug),
                                                                    changed: pg::is_changed(&p, &seen),
                                                                    p,
                                                                    m: m.clone(),
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // the empty viewer shows the whole command already
                if count > 0 {
                    div { class: "pages-tree-foot",
                        "Agents add pages with "
                        code { class: "mono", "inyon pages add" }
                    }
                }
            }
            section { class: "pages-viewer", "aria-label": "Viewer",
                if loaded && count == 0 {
                    div { class: "pages-viewer-empty", NoPages { wide: true } }
                } else if let Some(p) = page {
                    DesktopViewer { key: "{p.host}/{p.slug}", m: pg::machine_of(&h.hosts, &p.host), p }
                } else if loaded && selected.is_some() {
                    div { class: "pages-viewer-empty",
                        div { class: "empty",
                            span { class: "empty-icon", Glyph { icon: Icon::FolderOpen, size: 20 } }
                            p { class: "empty-title", "This page is gone" }
                            p { class: "empty-text", "It isn't on the list any more. Pick another on the left." }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TreeRow(p: PageEntry, m: PageMachine, active: bool, changed: bool) -> Element {
    let items = page_menu(&p, &m, true);
    let href = page_route(&p).href();
    let label = pg::page_label(&p, changed, m.online, platform::clock());
    let id = pg::id_of(&p);
    let dim = if m.online { "" } else { " dim" };
    rsx! {
        a {
            class: if active { "tree-row tree-row-on" } else { "tree-row" },
            href: "{href}",
            "aria-current": active.then_some("page"),
            "aria-label": "{label}",
            oncontextmenu: menu::open(items),
            // the open page clicked again opens with a new link, as Reload does
            onclick: move |_| {
                if active {
                    renew_link(&id);
                }
            },
            span { class: "tree-row-name{dim}",
                span { class: "truncate", "{p.name}" }
                if changed {
                    ChangedDot {}
                }
            }
            if let Some(g) = &p.progress {
                span { class: "tree-row-progress mono tabular fg-3{dim}",
                    ProgressBar { done: g.done, total: g.total, class: "progress-sm" }
                    "{g.done}/{g.total}"
                }
            }
        }
    }
}

#[component]
fn DeviceSwitch(value: Device, on_change: EventHandler<Device>) -> Element {
    let options = [
        (Device::Fit, "Fit the pane", None),
        (Device::Phone, "Phone 390 × 844", Some(Icon::Smartphone)),
        (Device::Tablet, "Tablet 820 × 1180", Some(Icon::Tablet)),
        (Device::Desktop, "Desktop 1440 × 900", Some(Icon::Monitor)),
    ];
    rsx! {
        div { class: "segmented segmented-inline segmented-sm", role: "radiogroup", "aria-label": "Frame size",
            for (d , name , icon) in options {
                button {
                    key: "{d.key()}",
                    class: if d == value { "segment segment-on" } else { "segment" },
                    r#type: "button",
                    role: "radio",
                    "aria-checked": "{d == value}",
                    "aria-label": "{name}",
                    title: "{name}",
                    onclick: move |_| on_change.call(d),
                    if let Some(i) = icon {
                        Glyph { icon: i, size: 15 }
                    } else {
                        "Fit"
                    }
                }
            }
        }
    }
}

#[component]
fn DesktopViewer(p: PageEntry, m: PageMachine) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut menu_at = use_context::<Signal<Option<menu::Menu>>>();
    let v = use_viewer(Some(p.clone()));
    let src = use_frame_src(v, Some(&p));
    let mut device = use_signal(|| Device::Fit);
    let mut stage = use_signal(|| (0.0f64, 0.0f64));
    let mut now = use_signal(|| platform::clock().now);
    use_hook(move || {
        spawn(async move {
            loop {
                platform::sleep(60_000).await;
                now.set(platform::clock().now);
            }
        });
    });
    let (more_id, notes_id) = use_hook(|| (format!("more-{}", dom::random_id()), format!("notes-{}", dom::random_id())));
    let id = pg::id_of(&p);
    let link = v.link(&id);
    let parts = v.parts(&id);
    let part = v.part(&id);
    let note = v.note(&p);
    let names = pg::running_names(&hub.read().hosts);
    let group = pg::group_name(&p, &names);
    let menu_items = vec![
        Item::new("Revoke links", Icon::Link2Off, Action::Page(PageOp::Revoke(p.clone()))),
        Item::new("Copy folder path", Icon::Copy, Action::Page(PageOp::CopyFolder(p.folder.clone()))),
        Item::new("Show folder", Icon::FolderOpen, Action::Page(PageOp::ShowFolder(p.clone()))),
        Item::new("Remove", Icon::Trash2, Action::Page(PageOp::Remove { page: p.clone(), back: true })).danger().separated(),
    ];
    let notes = notes_menu(&p, part, note.as_deref(), link.is_some());
    let has_notes = p.notes.as_ref().is_some_and(|n| !n.is_empty());
    let size = device().size();
    let (sw, sh) = stage();
    let scale = size.map_or(1.0, |(_, w, h)| pg::fit_scale(w, h, sw, sh));
    let kind = m.known.as_ref().map_or(MachineKind::Server, |k| k.1);
    let (a1, a2) = (more_id.clone(), notes_id.clone());
    let browse = p.clone();
    let retry_p = p.clone();
    let reload_p = p.clone();
    let mut vp = v;
    let left = link.as_ref().map(|l| format!(" · link lasts {}", pg::left_text(l.until, now()))).unwrap_or_default();
    rsx! {
        div { class: "viewer-bar",
            nav { class: "crumbs", "aria-label": "Page location",
                span { class: "crumb-machine",
                    MachineGlyph { kind, size: 14, class: "fg-3" }
                    "{m.name}"
                }
                span { class: "fg-3", "/" }
                span { class: "truncate", "{group}" }
                span { class: "fg-3", "/" }
                b { class: "truncate crumb-page", "{p.name}" }
            }
            if parts.len() > 1 {
                Segmented {
                    value: if note.is_some() { String::new() } else { part.key().to_string() },
                    options: parts.iter().map(|pt| (pt.key().to_string(), pt.label().to_string())).collect::<Vec<_>>(),
                    label: "Part of the page",
                    on_change: move |k: String| vp.pick_part(Part::of(&k)),
                }
            }
            if has_notes {
                button {
                    id: "{notes_id}",
                    class: if note.is_some() { "notes-button notes-button-on press" } else { "notes-button press" },
                    r#type: "button",
                    "aria-label": if let Some(n) = &note { format!("Notes: {} open", pg::note_title(n)) } else { "Notes".to_string() },
                    onclick: move |_| menu::drop_down(&mut menu_at, &a2, notes.clone()),
                    Glyph { icon: Icon::NotebookText, size: 14 }
                    span { class: "truncate", if let Some(n) = &note { {pg::note_title(n)} } else { "Notes" } }
                    Glyph { icon: Icon::ChevronDown, size: 14, class: "fg-3" }
                }
            }
            DeviceSwitch { value: device(), on_change: move |d| device.set(d) }
            button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Reload", title: "Reload", disabled: link.is_none(),
                onclick: move |_| v.reload(&reload_p),
                Glyph { icon: Icon::RotateCw, size: 16 }
            }
            // a fresh 12 h link each time; the tab sits at the hub's address, which is never a sign-in page
            button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Open in new tab", title: "Open in new tab. A 12 h link; never sign in there.", disabled: link.is_none(),
                onclick: move |_| open_in_browser(browse.clone()),
                Glyph { icon: Icon::ExternalLink, size: 16 }
            }
            button { id: "{more_id}", class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "More for {p.name}",
                onclick: move |_| menu::drop_down(&mut menu_at, &a1, menu_items.clone()),
                Glyph { icon: Icon::Ellipsis, size: 16 }
            }
        }
        div {
            class: if size.is_some() { "viewer-stage-wide viewer-stage-device" } else { "viewer-stage-wide" },
            onresize: move |e: Event<ResizeData>| {
                if let Ok(s) = e.get_border_box_size() {
                    stage.set((s.width, s.height));
                }
            },
            match (src, m.online) {
                (Some((s, k)), true) => match size {
                    Some((_, w, h)) => rsx! {
                        div { class: "device-box", style: "width:{w * scale}px;height:{h * scale}px",
                            div { class: "device-frame", style: "width:{w}px;height:{h}px;transform:scale({scale})",
                                PageFrame { key: "{k}", src: s, title: p.name.clone() }
                            }
                        }
                    },
                    None => rsx! {
                        div { class: "fit-frame",
                            PageFrame { key: "{k}", src: s, title: p.name.clone() }
                        }
                    },
                },
                _ => rsx! {
                    div { class: "viewer-state",
                        FrameState {
                            page: p.clone(),
                            m: m.clone(),
                            error: v.error(&id),
                            on_retry: move |_| v.retry(&retry_p),
                            on_back: move |_| route::replace(&mut here, Route::Pages { page: None }),
                        }
                    }
                },
            }
        }
        div { class: "viewer-foot mono",
            Glyph { icon: Icon::ShieldCheck, size: 14 }
            span { class: "truncate", "{SANDBOX_NOTE}{left}" }
            span { class: "viewer-size", {pg::size_note(device(), sw, sh)} }
        }
    }
}

// ---------- the sheets ----------

/// The sheets the page menus open, mounted once (in the shell).
#[component]
pub fn PageSheets() -> Element {
    let close = move |_| *SHEET_OPEN.write() = false;
    match SHEET() {
        None => rsx! {},
        Some(PageSheet::Folder(p)) => rsx! { FolderSheet { page: *p, open: SHEET_OPEN(), on_close: close } },
        Some(PageSheet::Add) => rsx! { AddFolderSheet { open: SHEET_OPEN(), on_close: close } },
    }
}

#[component]
fn FolderSheet(page: PageEntry, open: bool, on_close: EventHandler<()>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let here = use_context::<Signal<Route>>();
    let m = pg::machine_of(&hub.read().hosts, &page.host);
    let added = inyon_chat_core::js::local_time(page.added as f64, platform::clock().tz_minutes).month_day();
    let rows = [
        ("Page", page.name.clone(), false),
        ("Folder", page.folder.clone(), true),
        ("Machine", if m.online { m.name.clone() } else { format!("{} · offline", m.name) }, false),
        ("Project", pg::project_name(&page.project), false),
        ("Added by", format!("{} · {added}", pg::added_by(&page.by)), false),
    ];
    let (gone, folder) = (page.clone(), page.folder.clone());
    rsx! {
        Sheet {
            open,
            on_close: move |_| on_close.call(()),
            title: "Folder",
            icon: rsx! { Glyph { icon: Icon::FolderOpen, size: 16, class: "fg-2" } },
            footer: rsx! {
                div { class: "sheet-actions",
                    button { class: "button button-md button-danger press", r#type: "button",
                        onclick: move |_| {
                            on_close.call(());
                            run(PageOp::Remove { page: gone.clone(), back: false }, here, hub);
                        },
                        Glyph { icon: Icon::Trash2, size: 14 }
                        "Remove"
                    }
                    button { class: "button button-md button-primary press", r#type: "button", onclick: move |_| run_copy(folder.clone()),
                        Glyph { icon: Icon::Copy, size: 14 }
                        "Copy path"
                    }
                }
            },
            dl { class: "folder-rows",
                for (k , val , mono) in rows {
                    div { key: "{k}", class: "contents",
                        dt { "{k}" }
                        dd { class: if mono { "mono" } else { "" }, "{val}" }
                    }
                }
            }
            p { class: "pages-note", "Remove takes the page out of Inyon. The folder and its files stay." }
        }
    }
}

/// Whether a folder path is a full one, as the Add sheet asks: `/…`, or a Windows drive.
pub fn full_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/') || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\'))
}

#[component]
fn AddFolderSheet(open: bool, on_close: EventHandler<()>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut folder = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    use_effect(use_reactive!(|open| {
        if open {
            folder.set(String::new());
            name.set(String::new());
            error.set(String::new());
        }
    }));
    let submit = move || {
        let path = folder.peek().trim().to_string();
        if !full_path(&path) {
            error.set("Give the full path, starting with /.".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let n = name.peek().trim().to_string();
        spawn(async move {
            // the hub's own machine: a phone app adds on others, the hub's app on itself
            let host = "local";
            match api::add_page(host, &PageBody { path, name: (!n.is_empty()).then_some(n), slug: None }).await {
                Ok(added) => {
                    toast::toast(format!("Added “{}”", added.name), Some(format!("On {}", machine(hub, host).name)), ToastTone::Success);
                    on_close.call(());
                }
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    };
    let mut submit_f = submit;
    let ready = !folder().trim().is_empty();
    rsx! {
        Sheet {
            open,
            on_close: move |_| on_close.call(()),
            title: "Add a folder",
            footer: rsx! {
                div { class: "sheet-actions",
                    button { class: "button button-md button-ghost press", r#type: "button", onclick: move |_| on_close.call(()), "Cancel" }
                    button { class: "button button-md button-primary press", r#type: "submit", form: "add-page", disabled: !ready || busy(),
                        Busy { on: busy() }
                        "Add"
                    }
                }
            },
            form {
                id: "add-page",
                class: "stack",
                onsubmit: move |e| {
                    e.prevent_default();
                    if ready && !busy() {
                        submit_f();
                    }
                },
                Field { label: "Folder", hint: "The site's own folder, the one with index.html in it.",
                    input {
                        class: "field mono-field",
                        value: "{folder}",
                        placeholder: "/home/me/project/private/plans/my-plan",
                        autocapitalize: "off",
                        spellcheck: "false",
                        "data-autofocus": "true",
                        oninput: move |e| folder.set(e.value()),
                    }
                }
                Field { label: "Name",
                    input { class: "field", value: "{name}", placeholder: "Shown in the list", oninput: move |e| name.set(e.value()) }
                }
                if !error().is_empty() {
                    p { class: "form-error", role: "alert", "{error}" }
                }
                p { class: "pages-note",
                    "Agents do this for you with "
                    code { class: "mono", "inyon pages add" }
                    "."
                }
            }
        }
    }
}

// ---------- pages listed elsewhere ----------

/// Pages as rows in one card, each a link to its viewer with the page menu: on a machine's page, in
/// a session's pages and under a reply that links to them.
#[component]
pub fn PageRows(pages: Vec<PageEntry>, #[props(default)] class: String) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let seen = PAGES.read().seen.clone();
    let h = hub.read();
    rsx! {
        div { class: "panel rows page-rows {class}",
            for p in pages {
                PageRow { key: "{p.host}/{p.slug}", changed: pg::is_changed(&p, &seen), m: pg::machine_of(&h.hosts, &p.host), p }
            }
        }
    }
}

#[component]
fn PageRow(p: PageEntry, m: PageMachine, changed: bool) -> Element {
    let items = page_menu(&p, &m, true);
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let href = page_route(&p).href();
    let line = format!(
        "{} · {}",
        p.progress.as_ref().and_then(|g| g.now.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| pg::project_name(&p.project)),
        if m.online { pg::updated_line(&p, platform::clock()) } else { "offline".into() }
    );
    rsx! {
        a {
            class: "row page-row",
            href: "{href}",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            onclick: move |e| {
                p4.click(&e);
            },
            Glyph { icon: Icon::PanelsTopLeft, size: 16, class: "fg-3" }
            div { class: "page-row-text",
                div { class: "page-row-name",
                    span { class: "truncate", "{p.name}" }
                    if changed {
                        ChangedDot {}
                    }
                }
                div { class: "page-row-line mono truncate", "{line}" }
            }
            if let Some(g) = &p.progress {
                span { class: "page-row-progress mono tabular fg-2",
                    ProgressBar { done: g.done, total: g.total, class: "progress-row wide-only" }
                    "{g.done}/{g.total}"
                }
            }
            Glyph { icon: Icon::ChevronRight, size: 16, class: "fg-3" }
        }
    }
}

/// "Pages on this machine", on a machine's page; nothing when it has none.
#[component]
pub fn MachinePages(host: String) -> Element {
    let mut pages: Vec<PageEntry> = PAGES.read().pages.iter().flatten().filter(|p| p.host == host).cloned().collect();
    if pages.is_empty() {
        return rsx! {};
    }
    pages.sort_by(|a, b| pg::changed_at(b).partial_cmp(&pg::changed_at(a)).unwrap_or(std::cmp::Ordering::Equal));
    rsx! {
        section { class: "machine-pages",
            div { class: "section-label", h2 { "Pages on this machine" } }
            PageRows { pages }
        }
    }
}

/// The pages a finished reply links to (what `inyon pages add` prints), as rows that open them.
/// Only pages the list has, so a link to one that's gone shows nothing.
#[component]
pub fn PageCards(text: String, host: String) -> Element {
    let ids = pg::linked_pages(&text, &host);
    let list = PAGES.read();
    let pages: Vec<PageEntry> = ids.iter().filter_map(|id| list.pages.iter().flatten().find(|p| pg::id_of(p) == *id).cloned()).take(3).collect();
    drop(list);
    if pages.is_empty() {
        return rsx! {};
    }
    rsx! { PageRows { pages, class: "reply-pages" } }
}

/// The session header's Pages button: the pages this session works on (see
/// [`pg::session_pages`]), in a sheet. Nothing for a session with none.
#[component]
pub fn SessionPages(host: String, cwd: Option<String>, session: Option<String>) -> Element {
    let mut open = use_signal(|| false);
    let pages = pg::session_pages(PAGES.read().pages.as_deref().unwrap_or_default(), &host, cwd.as_deref(), session.as_deref());
    if pages.is_empty() && !open() {
        return rsx! {};
    }
    let n = pages.len();
    rsx! {
        button { class: "button button-sm button-ghost press session-pages", r#type: "button", "aria-label": "Pages: {n}", title: "This session's pages",
            onclick: move |_| open.set(true),
            Glyph { icon: Icon::PanelsTopLeft, size: 14 }
            span { class: "wide-label", "Pages" }
            span { class: "mono tabular", "{n}" }
        }
        Sheet { open: open(), on_close: move |_| open.set(false), title: "Pages", icon: rsx! { Glyph { icon: Icon::PanelsTopLeft, size: 16, class: "fg-2" } },
            if pages.is_empty() {
                p { class: "sheet-text",
                    "Sites an agent adds with "
                    code { class: "mono", "inyon pages add" }
                    " in this project show up here."
                }
            } else {
                div { onclick: move |_| open.set(false), PageRows { pages } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> PageEntry {
        PageEntry {
            host: "local".into(),
            slug: "plan".into(),
            name: "Plan".into(),
            project: "/p".into(),
            by: "cli".into(),
            added: 1,
            folder: "/p/plan".into(),
            progress: None,
            notes: Some(vec!["a.md".into(), "b.md".into()]),
            session: None,
            session_name: None,
        }
    }

    fn labels(v: &[Item]) -> Vec<&str> {
        v.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn a_pages_menu_is_the_same_wherever_it_is_listed() {
        let p = entry();
        let on = PageMachine { name: "studio".into(), online: true, hub: true, known: None };
        let m = page_menu(&p, &on, true);
        assert_eq!(labels(&m), ["Open", "Open in browser", "Revoke links", "Show folder", "Remove"]);
        assert_eq!(m[0].action, Action::Go(Route::Pages { page: Some(("local".into(), "plan".into())) }));
        assert_eq!(m[1].hint.as_deref(), Some(BROWSER_HINT));
        assert!(m[2].separated && m[4].separated && m[4].danger);
        // its machine offline: no link can be minted
        let off = PageMachine { online: false, ..on };
        let m = page_menu(&p, &off, false);
        assert_eq!(labels(&m), ["Open in browser", "Revoke links", "Show folder", "Remove"]);
        assert!(m[0].disabled);
        assert_eq!(m[0].hint.as_deref(), Some("studio is offline"));
    }

    #[test]
    fn the_notes_menu_offers_the_way_back_while_a_note_is_open() {
        let p = entry();
        assert_eq!(labels(&notes_menu(&p, Part::Plan, None, true)), ["a", "b"]);
        let m = notes_menu(&p, Part::Mockups, Some("b.md"), true);
        assert_eq!(labels(&m), ["Back to Mockups", "a", "b"]);
        assert!(m[1].separated && m[2].checked == Some(true) && m[1].checked == Some(false));
        // before the link comes, a note can't open
        assert!(notes_menu(&p, Part::Plan, None, false).iter().all(|i| i.disabled));
    }

    #[test]
    fn a_folder_is_added_by_its_full_path() {
        assert!(full_path("/home/me/plan"));
        assert!(full_path("C:\\Dev\\plan") && full_path("d:/x"));
        assert!(!full_path("./private/plans/x") && !full_path("~/x") && !full_path("C:"));
    }
}
