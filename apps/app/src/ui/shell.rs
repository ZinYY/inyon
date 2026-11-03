//! The frame around the pages: a sidebar from tablet width up, a tab bar on phones
//! (components/AppShell.tsx), around the Overview, the session screen, Machines, Pages, Settings
//! and Temp files. The layers every page shares (menus, toasts, the confirm sheet, the sheets a
//! menu opens, Can't connect?, "Inyon was updated") are mounted here once.

use dioxus::prelude::*;

use super::icons::{Glyph, Icon, Logo};
use super::menu::MenuLayer;
use super::overview::Overview;
use super::session::Session;
use super::{add, companion, connect, machines, notices, pages, settings, temp, toast};
use crate::model::{self, AgentPane};
use crate::route::Route;
use crate::state::Hub;

/// The Overview page's key, beside the sessions' `host:pane` ones.
const OVERVIEW_KEY: &str = "overview";
const MACHINES_KEY: &str = "machines";
const PAGES_KEY: &str = "pages";
const SETTINGS_KEY: &str = "settings";
const TEMP_KEY: &str = "temp";

#[component]
pub fn Shell() -> Element {
    let here = use_context::<Signal<Route>>();
    let route = here();
    let section = match route {
        Route::Overview => Section::Overview,
        Route::Machines { .. } | Route::Machine { .. } => Section::Machines,
        Route::Pages { .. } => Section::Pages,
        Route::Settings | Route::Temp { .. } => Section::Settings,
        Route::Session { .. } => Section::None,
    };
    let current = match &route {
        Route::Session { host, pane, .. } => Some((host.clone(), pane.clone())),
        _ => None,
    };
    rsx! {
        div { class: "shell",
            Sidebar { section, current }
            main { class: "main",
                match route {
                    Route::Overview => rsx! {
                        div { key: "{OVERVIEW_KEY}", class: "page", Overview {} }
                        TabBar { section }
                    },
                    // a session has the screen to itself on a phone, as in the web app
                    Route::Session { host, pane, view } => rsx! {
                        div { key: "{host}:{pane}", class: "page", Session { host, pane, view } }
                    },
                    Route::Machines { add } => rsx! {
                        div { key: "{MACHINES_KEY}", class: "page", machines::MachinesPage { add } }
                        TabBar { section }
                    },
                    Route::Machine { id } => rsx! {
                        div { key: "machine:{id}", class: "page", machines::MachinePage { id } }
                        TabBar { section }
                    },
                    // a page open on a phone takes the whole screen
                    Route::Pages { page } => rsx! {
                        div { key: "{PAGES_KEY}", class: "page", pages::PagesPage { page: page.clone() } }
                        if !pages::full_screen(&page) {
                            TabBar { section }
                        }
                    },
                    Route::Settings => rsx! {
                        div { key: "{SETTINGS_KEY}", class: "page", settings::SettingsPage {} }
                        TabBar { section }
                    },
                    Route::Temp { host } => rsx! {
                        div { key: "{TEMP_KEY}", class: "page", temp::TempPage { host } }
                        TabBar { section }
                    },
                }
            }
            MenuLayer {}
            add::ActionSheets {}
            pages::PageSheets {}
            connect::ConnectHelpHost {}
            toast::ConfirmHost {}
            toast::Toasts {}
            notices::UpdatePrompt {}
        }
    }
}

/// Which item the sidebar and the tab bar light up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    Machines,
    Pages,
    Settings,
    None,
}

fn nav_class(on: bool) -> &'static str {
    if on {
        "nav-item nav-active"
    } else {
        "nav-item"
    }
}

fn tab_class(on: bool) -> &'static str {
    if on {
        "tab tab-active"
    } else {
        "tab"
    }
}

#[component]
fn Sidebar(section: Section, current: Option<(String, String)>) -> Element {
    let overview = section == Section::Overview;
    let on_settings = section == Section::Settings;
    let on_machines = section == Section::Machines;
    let on_pages = section == Section::Pages;
    let (changed, pages_total) = {
        let l = pages::PAGES.read();
        (l.changed(), l.pages.as_ref().map_or(0, Vec::len))
    };
    let hub = use_context::<Signal<Hub>>();
    let h = hub.read();
    let online = h.hosts.iter().filter(|x| x.status == inyon_protocol::HostStatus::Online).count();
    let total = h.hosts.len();
    let count = if online == total { total.to_string() } else { format!("{online}/{total}") };
    rsx! {
        aside { class: "sidebar",
            div { class: "sidebar-head",
                Logo { connecting: !h.connected, target: true }
                span { class: "brand", "Inyon" }
                span { class: "chip chip-quiet", title: "The new app, which takes over from the current one in 1.0", "Preview" }
                if !h.connected {
                    button {
                        class: "sidebar-offline icon-button press",
                        r#type: "button",
                        "aria-label": "Reconnecting. Can't connect?",
                        title: "Reconnecting. Can't connect?",
                        onclick: move |_| connect::open_connect_help(None),
                        Glyph { icon: Icon::WifiOff, size: 16 }
                    }
                }
            }
            nav { class: "sidebar-nav",
                a { class: nav_class(overview), href: "/next/", "aria-current": overview.then_some("page"),
                    Glyph { icon: Icon::LayoutGrid, size: 16 }
                    "Overview"
                }
                companion::CompanionNav { current }
            }
            div { class: "sidebar-foot",
                a { class: nav_class(on_machines), href: "/next/machines", "aria-current": on_machines.then_some("page"),
                    Glyph { icon: Icon::Monitor, size: 16 }
                    "Machines"
                    if total > 0 {
                        span { class: "nav-count mono tabular", title: "{online} of {total} online", "{count}" }
                    }
                }
                a { class: nav_class(on_pages), href: "/next/pages", "aria-current": on_pages.then_some("page"),
                    Glyph { icon: Icon::PanelsTopLeft, size: 16 }
                    "Pages"
                    if changed > 0 {
                        span {
                            class: "nav-badge mono tabular",
                            title: if changed == 1 { "1 changed since you last opened it".to_string() } else { format!("{changed} changed since you last opened them") },
                            "{changed}"
                        }
                    } else if pages_total > 0 {
                        span { class: "nav-count mono tabular", "{pages_total}" }
                    }
                }
                a { class: nav_class(on_settings), href: "/next/settings", "aria-current": on_settings.then_some("page"),
                    Glyph { icon: Icon::Settings, size: 16 }
                    "Settings"
                }
            }
        }
    }
}

#[component]
fn TabBar(section: Section) -> Element {
    let overview = section == Section::Overview;
    let on_machines = section == Section::Machines;
    let on_pages = section == Section::Pages;
    let on_settings = section == Section::Settings;
    let changed = pages::PAGES.read().changed();
    let hub = use_context::<Signal<Hub>>();
    let waiting = model::agent_panes(&hub.read().hosts).iter().filter(|r| r.agent().state == inyon_protocol::AgentState::Waiting).count();
    rsx! {
        nav { class: "tabbar",
            a { class: tab_class(overview), href: "/next/", "aria-current": overview.then_some("page"),
                span { class: "tab-icon",
                    Glyph { icon: Icon::LayoutGrid, size: 21 }
                    if waiting > 0 {
                        span { class: "tab-badge", "aria-label": "{waiting} need you", "{waiting}" }
                    }
                }
                "Overview"
            }
            a { class: tab_class(on_pages), href: "/next/pages", "aria-current": on_pages.then_some("page"),
                span { class: "tab-icon",
                    Glyph { icon: Icon::PanelsTopLeft, size: 21 }
                    if changed > 0 {
                        span { class: "tab-badge tab-badge-pages mono tabular", "aria-label": "{changed} changed", "{changed}" }
                    }
                }
                "Pages"
            }
            a { class: tab_class(on_machines), href: "/next/machines", "aria-current": on_machines.then_some("page"),
                Glyph { icon: Icon::Monitor, size: 21 }
                "Machines"
            }
            a { class: tab_class(on_settings), href: "/next/settings", "aria-current": on_settings.then_some("page"),
                Glyph { icon: Icon::Settings, size: 21 }
                "Settings"
            }
        }
    }
}
