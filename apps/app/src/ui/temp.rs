//! Temp files (pages/Temp.tsx): what agents leave in a machine's temp folders (pasted images,
//! screenshots, scratchpads), listed by the machine's own `inyon temp` through the hub. See them,
//! preview them, delete them. Deleting is permanent, so the question says how much goes and warns
//! about anything an agent may still be using.

use std::cell::RefCell;
use std::collections::HashMap;

use dioxus::prelude::*;
use inyon_chat_core::format::{basename, bytes};
use inyon_protocol::{HostState, HostStatus};

use super::icons::{Glyph, Icon, Logo};
use super::kit::{Opt, Select, Sheet, WaitRows};
use super::menu::{self, Action, Item};
use super::toast::{self, confirm, Ask, ToastTone};
use crate::api::{self, TempFile, TempItem, TempListing, TempPreview};
use crate::route::{self, Route};
use crate::settings::{self as st, TempGroup};
use crate::state::Hub;
use crate::{format, platform};

// ---------- thumbnails: fetched when on screen, a few at a time, kept for the session ----------

const THUMBS_AT_ONCE: usize = 3;
/// About 2 MB each at most, so the cache starts over past this many.
const THUMBS_KEPT: usize = 64;

thread_local! {
    static THUMBS: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
    static RUNNING: RefCell<usize> = const { RefCell::new(0) };
}

fn thumb_key(host: &str, path: &str) -> String {
    format!("{host}\0{path}")
}

/// One of the [`THUMBS_AT_ONCE`] requests, given back when it's dropped: also when the row
/// scrolls away and its request is dropped half way.
struct ThumbSlot;

impl ThumbSlot {
    fn take() -> ThumbSlot {
        RUNNING.with(|r| *r.borrow_mut() += 1);
        ThumbSlot
    }
}

impl Drop for ThumbSlot {
    fn drop(&mut self) {
        RUNNING.with(|r| *r.borrow_mut() -= 1);
    }
}

fn keep_thumb(key: String, t: Option<String>) {
    THUMBS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= THUMBS_KEPT && !c.contains_key(&key) {
            c.clear();
        }
        c.insert(key, t);
    });
}

async fn thumb(host: String, path: String) -> Option<String> {
    let key = thumb_key(&host, &path);
    if let Some(t) = THUMBS.with(|t| t.borrow().get(&key).cloned()) {
        return t;
    }
    while RUNNING.with(|r| *r.borrow()) >= THUMBS_AT_ONCE {
        platform::sleep(60).await;
    }
    let slot = ThumbSlot::take();
    let answer = api::temp_preview(&host, &path, Some(st::THUMB_MAX)).await;
    drop(slot);
    let kept = thumb_answer(answer);
    if let Some(t) = &kept {
        keep_thumb(key, t.clone());
    }
    kept.flatten()
}

/// What a preview answer leaves in the cache: the picture, or that there is none; nothing for a
/// failed request, which a later look tries again.
fn thumb_answer(answer: Result<TempPreview, api::ApiError>) -> Option<Option<String>> {
    let p = answer.ok()?;
    Some(Some(p).filter(|p| p.mime.starts_with("image/")).and_then(|p| st::data_url(&p.mime, p.base64.as_deref())))
}

/// Forgets the thumbnails of what was deleted.
fn forget_thumbs(host: &str, paths: &[String]) {
    let prefix = format!("{host}\0");
    THUMBS.with(|t| t.borrow_mut().retain(|k, _| !(k.starts_with(&prefix) && paths.iter().any(|p| k[prefix.len()..].starts_with(p.as_str())))));
}

#[component]
fn Thumb(host: String, path: String, #[props(default)] class: String) -> Element {
    let mut seen = use_signal(|| false);
    let mut src = use_signal(|| None::<String>);
    let (h, p) = (host.clone(), path.clone());
    use_effect(use_reactive!(|h, p| {
        if !seen() {
            return;
        }
        let (h, p) = (h.clone(), p.clone());
        spawn(async move {
            src.set(thumb(h, p).await);
        });
    }));
    rsx! {
        span {
            class: "temp-thumb {class}",
            onvisible: move |e: VisibleEvent| {
                if e.is_intersecting().unwrap_or(true) && !seen() {
                    seen.set(true);
                }
            },
            if let Some(s) = src() {
                img { src: "{s}", alt: "" }
            } else {
                Glyph { icon: Icon::Image, size: 14 }
            }
        }
    }
}

// ---------- what a menu asks of the screen ----------

/// What a temp item's or file's menu does, handed to the screen that's open.
#[derive(Debug, Clone, PartialEq)]
pub enum TempOp {
    Open(String),
    Toggle(String),
    Delete(String),
    Preview(String),
    DeleteFile(String),
}

static OP: GlobalSignal<Option<(u64, TempOp)>> = Signal::global(|| None);

pub fn run(op: TempOp) {
    let n = OP.peek().as_ref().map_or(0, |o| o.0) + 1;
    *OP.write() = Some((n, op));
}

/// Asks, deletes, and says what was freed. The paths it deleted: what failed stays picked and
/// open, for another try.
async fn delete_paths(host: &str, machine: &str, items: Vec<(String, String, f64, bool)>) -> Vec<String> {
    let (title, body) = st::delete_question(&items.iter().map(|(_, n, s, r)| (n.clone(), *s, *r)).collect::<Vec<_>>(), machine);
    if !confirm(Ask { title, body: Some(body), yes: "Delete".into(), danger: true, ..Ask::default() }).await {
        return Vec::new();
    }
    let paths: Vec<String> = items.into_iter().map(|i| i.0).collect();
    match api::temp_delete(host, &paths).await {
        Ok(r) => {
            let removed = r.removed.unwrap_or_default();
            forget_thumbs(host, &removed);
            let removed_too = removed.clone();
            if !removed.is_empty() {
                toast::toast(format!("Freed {}", bytes(r.freed.unwrap_or(0.0))), None, ToastTone::Success);
            }
            let failed = r.failed.unwrap_or_default();
            if !failed.is_empty() {
                let title = if failed.len() == 1 { format!("Couldn't delete {}", basename(Some(&failed[0].0))) } else { format!("Couldn't delete {} items", failed.len()) };
                let body =
                    failed.iter().take(3).map(|(p, why)| if failed.len() == 1 { why.clone() } else { format!("{}: {why}", basename(Some(p))) }).collect::<Vec<_>>().join("\n");
                toast::toast(title, Some(body), ToastTone::Error);
            }
            removed_too
        }
        Err(e) => {
            toast::toast("Couldn't delete", Some(e.message), ToastTone::Error);
            Vec::new()
        }
    }
}

// ---------- the screen ----------

/// `/temp` and `/temp/<machine>`: one machine's temp files, with a picker when there are several.
#[component]
pub fn TempPage(host: Option<String>) -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut here = use_context::<Signal<Route>>();
    let mut reload = use_signal(|| 0u64);
    use_hook(|| crate::dom::set_title("Temp files · Inyon"));
    let h = hub.read();
    let hosts = h.hosts.clone();
    let loaded = h.loaded || !hosts.is_empty();
    drop(h);
    let id = host.clone().or_else(|| hosts.iter().find(|h| h.status == HostStatus::Online).or(hosts.first()).map(|h| h.config.id.clone()));
    let current = id.as_ref().and_then(|id| hosts.iter().find(|h| h.config.id == *id)).cloned();
    let options: Vec<Opt> = hosts.iter().map(|h| Opt::new(h.config.id.clone(), h.config.name.clone()).hint((h.status != HostStatus::Online).then(|| "offline".into()))).collect();
    rsx! {
        div { class: "scroll",
            header { class: "topbar",
                div { class: "topbar-row page-head",
                    span { class: "phone-only", Logo {} }
                    h1 { class: "page-title truncate", "Temp files" }
                    if current.is_some() {
                        button { class: "icon-button icon-button-lg press", r#type: "button", "aria-label": "Refresh", title: "Refresh", onclick: move |_| reload += 1,
                            Glyph { icon: Icon::RotateCw, size: 16 }
                        }
                    }
                }
            }
            div { class: "narrow temp-page",
                if hosts.len() > 1 {
                    div { class: "temp-picker",
                        Select {
                            value: id.clone().unwrap_or_default(),
                            options,
                            title: "Machine",
                            on_change: move |v: String| route::replace(&mut here, Route::Temp { host: Some(v) }),
                        }
                    }
                }
                if !loaded {
                    WaitRows { label: "Loading machines" }
                } else if hosts.is_empty() {
                    Empty { icon: Icon::FolderOpen, title: "No machines yet", text: "Pair a machine or add one over SSH to see its temp files." }
                } else if let Some(h) = current {
                    TempFiles { key: "{h.config.id}", host: h, reload: reload() }
                } else {
                    div { class: "empty",
                        p { class: "empty-title", "No machine called {id.clone().unwrap_or_default()}" }
                    }
                }
            }
        }
    }
}

#[component]
fn Empty(icon: Icon, title: String, #[props(default)] text: Option<String>, #[props(default)] action: Option<Element>) -> Element {
    rsx! {
        div { class: "empty",
            span { class: "empty-icon", Glyph { icon, size: 20 } }
            p { class: "empty-title", "{title}" }
            if let Some(t) = text {
                p { class: "empty-text", "{t}" }
            }
            {action}
        }
    }
}

/// One machine's temp files: what the machine page's Temp files tab and `/temp` show. `reload`
/// lists them again each time it changes.
#[component]
pub fn TempFiles(host: HostState, #[props(default)] reload: u64) -> Element {
    let id = host.config.id.clone();
    let machine = host.config.name.clone();
    let online = host.status == HostStatus::Online;
    let mut list = use_signal(|| None::<TempListing>);
    let mut error = use_signal(|| None::<(u16, String)>);
    let mut picked = use_signal(Vec::<String>::new);
    let mut last = use_signal(|| None::<usize>);
    let mut open = use_signal(|| None::<TempItem>);
    let mut tries = use_signal(|| 0u64);
    // the latest listing asked for: an earlier one that answers late is dropped
    let mut asked = use_signal(|| 0u64);
    let load_id = id.clone();
    use_effect(use_reactive!(|online, reload| {
        let _ = (reload, tries());
        if !online {
            return;
        }
        let id = load_id.clone();
        let n = *asked.peek() + 1;
        asked.set(n);
        spawn(async move {
            let answer = api::temp_list(&id).await;
            if *asked.peek() != n {
                return;
            }
            match answer {
                Ok(l) => {
                    let items = l.items.clone().unwrap_or_default();
                    // what went away can't stay picked
                    picked.with_mut(|p| p.retain(|x| items.iter().any(|i| i.path == *x)));
                    // the anchor is a place in the list, which may hold something else now
                    last.set(None);
                    list.set(Some(l));
                    error.set(None);
                }
                Err(e) => error.set(Some((e.status, e.message))),
            }
        });
    }));
    let groups: Vec<TempGroup> = list.read().as_ref().map(st::temp_groups).unwrap_or_default();
    let order: Vec<TempItem> = groups.iter().flat_map(|g| g.items.clone()).collect();
    let paths: Vec<String> = order.iter().map(|i| i.path.clone()).collect();

    // a menu item for this list
    let start = use_hook(|| OP.peek().as_ref().map_or(0, |o| o.0));
    let op = OP();
    {
        let (order, paths, id, machine) = (order.clone(), paths.clone(), id.clone(), machine.clone());
        use_effect(use_reactive!(|op| {
            let Some((n, op)) = op else { return };
            if n <= start {
                return;
            }
            match op {
                TempOp::Open(p) => open.set(order.iter().find(|i| i.path == p).cloned()),
                TempOp::Toggle(p) => {
                    if let Some(at) = paths.iter().position(|x| *x == p) {
                        let next = st::toggle_pick(&picked.peek(), &paths, at, *last.peek(), false);
                        picked.set(next);
                        last.set(Some(at));
                    }
                }
                TempOp::Delete(p) => {
                    let Some(i) = order.iter().find(|i| i.path == p).cloned() else { return };
                    let (id, machine) = (id.clone(), machine.clone());
                    spawn(async move {
                        if delete_paths(&id, &machine, vec![(i.path.clone(), i.name.clone(), i.size, i.recent)]).await.contains(&i.path) {
                            picked.with_mut(|v| v.retain(|x| *x != i.path));
                            if open.peek().as_ref().is_some_and(|o| o.path == i.path) {
                                open.set(None);
                            }
                            tries += 1;
                        }
                    });
                }
                TempOp::Preview(_) | TempOp::DeleteFile(_) => {}
            }
        }));
    }

    if !online {
        return rsx! {
            Empty { icon: Icon::CloudOff, title: "Not connected", text: "Temp files show once the machine is reachable." }
        };
    }
    let err = error();
    if let (Some((status, message)), true) = (&err, list.read().is_none()) {
        let conflict = *status == 409;
        return rsx! {
            Empty {
                icon: Icon::FileText,
                title: if conflict { message.clone() } else { "Couldn't list temp files".to_string() },
                text: (!conflict).then(|| message.clone()),
                action: rsx! {
                    button { class: "button button-sm button-quiet press empty-action", r#type: "button", onclick: move |_| tries += 1,
                        Glyph { icon: Icon::RotateCw, size: 14 }
                        "Try again"
                    }
                },
            }
        };
    }
    if list.read().is_none() {
        return rsx! { WaitRows { label: "Looking through temp files on {machine}" } };
    }
    // a refresh that failed: the list is as it was, and says so
    let stale = err.map(|(_, message)| {
        rsx! {
            div { class: "stale-note", role: "status",
                Glyph { icon: Icon::CircleAlert, size: 14 }
                span { class: "stale-note-text", "Couldn't refresh the list: {message}" }
                button { class: "button button-xs button-ghost press", r#type: "button", onclick: move |_| tries += 1, "Try again" }
            }
        }
    });
    if order.is_empty() {
        return rsx! {
            {stale}
            Empty { icon: Icon::FileText, title: "Nothing here", text: "Agents' temp files show up here as they make them." }
        };
    }
    let total: f64 = order.iter().map(|i| i.size).sum();
    let chosen: Vec<TempItem> = order.iter().filter(|i| picked.read().contains(&i.path)).cloned().collect();
    let chosen_size: f64 = chosen.iter().map(|i| i.size).sum();
    let all = paths.clone();
    let old_items = order.clone();
    let (del_id, del_machine) = (id.clone(), machine.clone());
    rsx! {
        div { class: "temp-files",
            {stale}
            div { class: "temp-summary",
                p { class: "temp-total tabular", "{bytes(total)} in {st::items_text(order.len())}" }
                div { class: "temp-quick",
                    button { class: "button button-xs button-ghost press", r#type: "button", onclick: move |_| picked.set(all.clone()), "Select all" }
                    button { class: "button button-xs button-ghost press", r#type: "button",
                        onclick: move |_| {
                            let old = st::older_than_a_week(&old_items, platform::clock().now);
                            if old.is_empty() {
                                toast::toast("Nothing older than a week", None, ToastTone::Info);
                            } else {
                                picked.set(old);
                            }
                        },
                        "Older than a week"
                    }
                    if !picked.read().is_empty() {
                        button { class: "button button-xs button-ghost press", r#type: "button", onclick: move |_| picked.set(Vec::new()), "Clear" }
                    }
                }
            }
            for g in groups {
                section { key: "{g.id}", class: "temp-group", "aria-label": "{g.label}",
                    div { class: "temp-group-head",
                        h2 { class: "truncate", "{g.label}" }
                        span { class: "mono tabular fg-3", "{bytes(g.size)} · {g.items.len()}" }
                    }
                    div { class: "panel rows",
                        for item in g.items.clone() {
                            {
                                let at = paths.iter().position(|p| *p == item.path).unwrap_or(0);
                                let on = picked.read().contains(&item.path);
                                let paths = paths.clone();
                                rsx! {
                                    ItemRow {
                                        key: "{item.path}",
                                        host: id.clone(),
                                        item: item.clone(),
                                        on,
                                        on_toggle: move |range: bool| {
                                            let next = st::toggle_pick(&picked.peek(), &paths, at, *last.peek(), range);
                                            picked.set(next);
                                            last.set(Some(at));
                                        },
                                        on_open: move |_| open.set(Some(item.clone())),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !chosen.is_empty() {
                div { class: "temp-selection",
                    div { class: "temp-selection-bar slide-up", role: "region", "aria-label": "Selection",
                        span { class: "tabular truncate", "{chosen.len()} selected · {bytes(chosen_size)}" }
                        button { class: "icon-button press", r#type: "button", "aria-label": "Clear selection", title: "Clear selection", onclick: move |_| picked.set(Vec::new()),
                            Glyph { icon: Icon::X, size: 16 }
                        }
                        button { class: "button button-sm button-danger press", r#type: "button",
                            onclick: move |_| {
                                let items: Vec<(String, String, f64, bool)> = chosen.iter().map(|i| (i.path.clone(), i.name.clone(), i.size, i.recent)).collect();
                                let (id, machine) = (del_id.clone(), del_machine.clone());
                                spawn(async move {
                                    let gone = delete_paths(&id, &machine, items).await;
                                    if !gone.is_empty() {
                                        picked.with_mut(|v| v.retain(|x| !gone.contains(x)));
                                        if open.peek().as_ref().is_some_and(|o| gone.contains(&o.path)) {
                                            open.set(None);
                                        }
                                        tries += 1;
                                    }
                                });
                            },
                            Glyph { icon: Icon::Trash2, size: 14 }
                            "Delete"
                        }
                    }
                }
            }
            ItemSheet { host: host.clone(), item: open(), on_close: move |_| open.set(None), on_changed: move |_| tries += 1 }
        }
    }
}

#[component]
fn ItemRow(host: String, item: TempItem, on: bool, on_toggle: EventHandler<bool>, on_open: EventHandler<()>) -> Element {
    let items = vec![
        Item::new("Open", Icon::FolderOpen, Action::Temp(TempOp::Open(item.path.clone()))),
        Item::new(if on { "Deselect" } else { "Select" }, Icon::SquareCheck, Action::Temp(TempOp::Toggle(item.path.clone()))),
        Item::copy("Copy path", &item.path),
        Item::new("Delete…", Icon::Trash2, Action::Temp(TempOp::Delete(item.path.clone()))).danger().separated(),
    ];
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4, p5) = (press.clone(), press.clone(), press.clone(), press.clone(), press);
    let ago = format::time_ago(item.modified, platform::clock());
    let line = st::item_line(&item, &ago);
    let n = item.images.len();
    rsx! {
        div {
            class: if on { "temp-row temp-row-on" } else { "temp-row" },
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            button {
                class: "temp-check press",
                r#type: "button",
                role: "checkbox",
                "aria-checked": "{on}",
                "aria-label": "Select {item.name}",
                onclick: move |e: MouseEvent| {
                    if !p4.click(&e) {
                        on_toggle.call(e.modifiers().shift());
                    }
                },
                span { class: if on { "check-box check-box-on" } else { "check-box" },
                    if on {
                        Glyph { icon: Icon::Check, size: 12 }
                    }
                }
            }
            button {
                class: "temp-open",
                r#type: "button",
                onclick: move |e| {
                    if !p5.click(&e) {
                        on_open.call(());
                    }
                },
                span { class: "temp-words",
                    span { class: "temp-name-row",
                        span { class: "temp-name truncate", "{item.name}" }
                        if item.recent {
                            span { class: "chip", title: "Changed in the last 30 minutes", "recent" }
                        }
                    }
                    span { class: "temp-line mono tabular truncate", "{line}" }
                }
                if n > 0 {
                    span { class: "temp-thumbs", "aria-label": if n == 1 { "1 image".to_string() } else { format!("{n} images") },
                        for (i , p) in item.images.iter().take(4).enumerate() {
                            Thumb { key: "{p}", host: host.clone(), path: p.clone(), class: if i >= 2 { "wide-only" } else { "" } }
                        }
                    }
                }
            }
        }
    }
}

// ---------- one item's files, and a file's preview ----------

#[component]
fn ItemSheet(host: HostState, item: Option<TempItem>, on_close: EventHandler<()>, on_changed: EventHandler<()>) -> Element {
    let id = host.config.id.clone();
    let machine = host.config.name.clone();
    let mut shown = use_signal(|| item.clone());
    let mut files = use_signal(|| None::<api::TempFiles>);
    let mut error = use_signal(String::new);
    let mut preview = use_signal(|| None::<TempFile>);
    // each opening and listing has its own number, so a slow answer can't fill in an older item
    let mut opening = use_signal(|| 0u64);
    let path = item.as_ref().map(|i| i.path.clone());
    let load = {
        let id = id.clone();
        move |p: String| {
            let n = *opening.peek() + 1;
            opening.set(n);
            let id = id.clone();
            spawn(async move {
                let r = api::temp_files(&id, &p).await;
                if *opening.peek() != n {
                    return;
                }
                match r {
                    Ok(f) => {
                        files.set(Some(f));
                        error.set(String::new());
                    }
                    Err(e) => error.set(e.message),
                }
            });
        }
    };
    {
        let item = item.clone();
        let mut load = load.clone();
        use_effect(use_reactive!(|path| {
            let _ = path;
            opening += 1;
            let Some(i) = item.clone() else { return };
            shown.set(Some(i.clone()));
            files.set(None);
            error.set(String::new());
            if i.kind == "dir" {
                load(i.path.clone());
            }
        }));
    }
    let single =
        item.as_ref().filter(|i| i.kind == "file").map(|i| TempFile { path: i.path.clone(), rel: i.name.clone(), size: i.size, modified: i.modified, kind: "other".into() });
    let remove_file = {
        let (id, machine, load) = (id.clone(), machine.clone(), load.clone());
        move |f: TempFile| {
            let (id, machine, mut load) = (id.clone(), machine.clone(), load.clone());
            spawn(async move {
                let recent = platform::clock().now - f.modified < 30.0 * 60_000.0;
                if !delete_paths(&id, &machine, vec![(f.path.clone(), f.rel.clone(), f.size, recent)]).await.contains(&f.path) {
                    return;
                }
                if preview.peek().as_ref().is_some_and(|p| p.path == f.path) {
                    preview.set(None);
                }
                let s = shown.peek().clone();
                match s {
                    Some(s) if s.kind == "file" => on_close.call(()),
                    Some(s) => load(s.path.clone()),
                    None => {}
                }
                on_changed.call(());
            });
        }
    };
    // its files' menus
    let start = use_hook(|| OP.peek().as_ref().map_or(0, |o| o.0));
    let op = OP();
    {
        let listed = files.read().as_ref().and_then(|f| f.files.clone()).unwrap_or_default();
        let remove_file = remove_file.clone();
        use_effect(use_reactive!(|op| {
            let Some((n, op)) = op else { return };
            if n <= start {
                return;
            }
            match op {
                TempOp::Preview(p) => preview.set(listed.iter().find(|f| f.path == p).cloned()),
                TempOp::DeleteFile(p) => {
                    if let Some(f) = listed.iter().find(|f| f.path == p).cloned() {
                        remove_file.clone()(f);
                    }
                }
                _ => {}
            }
        }));
    }
    let title = shown.read().as_ref().map(|s| s.name.clone()).unwrap_or_default();
    let full = shown.read().as_ref().map(|s| s.path.clone()).unwrap_or_default();
    let err = error();
    let listing = files();
    let remove_for_preview = remove_file.clone();
    rsx! {
        Sheet { open: item.is_some() && single.is_none(), on_close: move |_| on_close.call(()), title, wide: true,
            p { class: "temp-path mono truncate", title: "{full}", "{full}" }
            if !err.is_empty() {
                p { class: "temp-sheet-note danger", "{err}" }
            } else if let Some(l) = listing {
                if l.files.as_deref().unwrap_or_default().is_empty() {
                    p { class: "temp-sheet-note fg-3", "This folder is empty." }
                } else {
                    div { class: "temp-file-list",
                        for f in l.files.clone().unwrap_or_default() {
                            FileRow { key: "{f.path}", host: id.clone(), file: f.clone(), on_open: move |_| preview.set(Some(f.clone())) }
                        }
                        if l.truncated {
                            p { class: "temp-sheet-note fg-3", "Showing the newest {l.files.as_deref().unwrap_or_default().len()} files." }
                        }
                    }
                }
            } else {
                WaitRows { label: "Listing files" }
            }
        }
        PreviewSheet {
            host: id.clone(),
            file: single.clone().or(preview()),
            on_close: move |_| if single.is_some() { on_close.call(()) } else { preview.set(None) },
            can_delete: item.as_ref().is_some_and(|i| i.kind == "dir"),
            on_delete: move |f: TempFile| remove_for_preview.clone()(f),
        }
    }
}

#[component]
fn FileRow(host: String, file: TempFile, on_open: EventHandler<()>) -> Element {
    let items = vec![
        Item::new("Preview", Icon::Eye, Action::Temp(TempOp::Preview(file.path.clone()))),
        Item::copy("Copy path", &file.path),
        Item::new("Delete…", Icon::Trash2, Action::Temp(TempOp::DeleteFile(file.path.clone()))).danger().separated(),
    ];
    let press = menu::long_press(items.clone());
    let (p1, p2, p3, p4) = (press.clone(), press.clone(), press.clone(), press);
    let ago = format::time_ago(file.modified, platform::clock());
    rsx! {
        button {
            class: "temp-file press",
            r#type: "button",
            oncontextmenu: menu::open(items),
            onpointerdown: move |e| p1.down(&e),
            onpointermove: move |e| p2.moved(&e),
            onpointerup: move |_| p3.up(),
            onclick: move |e| {
                if !p4.click(&e) {
                    on_open.call(());
                }
            },
            if file.kind == "image" {
                Thumb { host: host.clone(), path: file.path.clone(), class: "temp-thumb-lg" }
            } else {
                span { class: "temp-thumb temp-thumb-lg", Glyph { icon: Icon::FileText, size: 16 } }
            }
            span { class: "temp-words",
                span { class: "temp-file-name truncate", "{file.rel}" }
                span { class: "temp-line mono tabular truncate", "{bytes(file.size)} · {ago}" }
            }
        }
    }
}

#[component]
fn PreviewSheet(host: String, file: Option<TempFile>, on_close: EventHandler<()>, can_delete: bool, on_delete: EventHandler<TempFile>) -> Element {
    let mut shown = use_signal(|| file.clone());
    let mut p = use_signal(|| None::<TempPreview>);
    let mut error = use_signal(String::new);
    let mut asking = use_signal(|| 0u64);
    let path = file.as_ref().map(|f| f.path.clone());
    {
        let (host, file) = (host.clone(), file.clone());
        use_effect(use_reactive!(|path| {
            let _ = path;
            let Some(f) = file.clone() else { return };
            shown.set(Some(f.clone()));
            p.set(None);
            error.set(String::new());
            let n = *asking.peek() + 1;
            asking.set(n);
            let host = host.clone();
            spawn(async move {
                let r = api::temp_preview(&host, &f.path, None).await;
                if *asking.peek() != n {
                    return;
                }
                match r {
                    Ok(x) => p.set(Some(x)),
                    Err(e) => error.set(e.message),
                }
            });
        }));
    }
    let s = shown();
    let title = s.as_ref().map(|f| basename(Some(&f.rel))).unwrap_or_default();
    let alt = s.as_ref().map(|f| f.rel.clone()).unwrap_or_default();
    let err = error();
    let got = p();
    let src = got.as_ref().filter(|x| x.mime.starts_with("image/")).and_then(|x| st::data_url(&x.mime, x.base64.as_deref()));
    let footer = match (&s, can_delete) {
        (Some(f), true) => {
            let f = f.clone();
            let ago = format::time_ago(f.modified, platform::clock());
            Some(rsx! {
                div { class: "temp-preview-foot",
                    span { class: "mono tabular truncate fg-3", "{bytes(f.size)} · {ago}" }
                    button { class: "button button-sm button-danger press", r#type: "button", onclick: move |_| on_delete.call(f.clone()),
                        Glyph { icon: Icon::Trash2, size: 14 }
                        "Delete"
                    }
                }
            })
        }
        _ => None,
    };
    rsx! {
        Sheet { open: file.is_some(), on_close: move |_| on_close.call(()), title, wide: true, footer,
            if !err.is_empty() {
                p { class: "temp-sheet-note danger", "{err}" }
            } else if let Some(x) = got {
                if let Some(src) = src {
                    img { class: "temp-preview-img", src: "{src}", alt: "{alt}" }
                } else if let Some(text) = &x.text {
                    // shown as text, never as a page or markdown: an agent's HTML stays inert here
                    pre { class: "temp-preview-text mono", "{text}" }
                    if x.truncated {
                        p { class: "temp-sheet-note fg-3", "Truncated: showing the first 256 KB of {bytes(x.size)}." }
                    }
                } else {
                    Empty {
                        icon: Icon::FileText,
                        title: if x.truncated && x.mime.starts_with("image/") { "Too big to preview" } else { "No preview for this kind of file" },
                        text: format!("{} · {}", if x.mime.is_empty() { "unknown type" } else { x.mime.as_str() }, bytes(x.size)),
                    }
                }
            } else {
                div { class: "skeleton skeleton-panel", role: "status", "aria-label": "Opening" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_thumbnail_is_tried_again_and_a_missing_one_is_not() {
        let png = TempPreview { mime: "image/png".into(), base64: Some("iVBO".into()), ..Default::default() };
        assert_eq!(thumb_answer(Ok(png)), Some(Some("data:image/png;base64,iVBO".into())));
        assert_eq!(thumb_answer(Ok(TempPreview { mime: "text/plain".into(), ..Default::default() })), Some(None));
        assert_eq!(thumb_answer(Err(api::ApiError { status: 0, message: "offline".into() })), None);
    }

    #[test]
    fn the_thumbnail_cache_starts_over_when_full() {
        for i in 0..THUMBS_KEPT {
            keep_thumb(thumb_key("box", &format!("/tmp/{i}")), None);
        }
        assert_eq!(THUMBS.with(|c| c.borrow().len()), THUMBS_KEPT);
        // one already there is only replaced
        keep_thumb(thumb_key("box", "/tmp/0"), Some("x".into()));
        assert_eq!(THUMBS.with(|c| c.borrow().len()), THUMBS_KEPT);
        keep_thumb(thumb_key("box", "/tmp/new"), None);
        assert_eq!(THUMBS.with(|c| c.borrow().len()), 1);
    }

    #[test]
    fn a_thumbnail_dropped_half_way_gives_its_slot_back() {
        let running = || RUNNING.with(|r| *r.borrow());
        let before = running();
        let slot = ThumbSlot::take();
        assert_eq!(running(), before + 1);
        // a request future dropped while it waits drops its slot with it
        let waiting = async move {
            let _slot = slot;
            std::future::pending::<()>().await;
        };
        drop(waiting);
        assert_eq!(running(), before);
    }
}
