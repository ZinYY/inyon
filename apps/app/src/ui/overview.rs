//! The Overview (apps/web/src/pages/Home.tsx and components/Overview.tsx): every agent session as a
//! card, grouped by machine when there is more than one, the machines, and the plan limits.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use inyon_protocol::{AgentState, HostState, HostStatus};

use super::icons::{AgentMark, Glyph, Icon, Logo, MachineGlyph};
use crate::format::{self, Clock};
use crate::model::{self, AgentPane, Filter, PaneRef, Shown};
use crate::state::Hub;
use crate::{api, platform};

/// Folded machine groups, in the web app's key (`inyon.folded`, entries `home:<machine>`), so a
/// group folded in one app is folded in the other.
const FOLDED: &str = "inyon.folded";

fn load_folded() -> BTreeSet<String> {
    platform::load(FOLDED).and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default().into_iter().collect()
}

#[component]
pub fn Overview() -> Element {
    let hub = use_context::<Signal<Hub>>();
    let mut folded = use_signal(load_folded);
    let mut filter = use_signal(|| Filter::All);
    let h = hub.read();
    let clock = platform::clock();
    let panes = model::agent_panes(&h.hosts);
    // the switch goes with the sessions it would sort; with few of them it's back to all
    let filtering = panes.len() >= model::FILTER_FROM;
    let shown_filter = if filtering { filter() } else { Filter::All };
    let groups = model::group_panes(&h.hosts, shown_filter);
    let grouped = h.hosts.len() > 1;
    let mut toggle = move |id: String| {
        let key = format!("home:{id}");
        let mut f = folded.write();
        if !f.remove(&key) {
            f.insert(key);
        }
        let list: Vec<&String> = f.iter().collect();
        platform::save(FOLDED, &serde_json::to_string(&list).unwrap_or_default());
    };

    rsx! {
        div { class: "scroll",
            header { class: "topbar",
                div { class: "topbar-row",
                    span { class: "phone-only", Logo { connecting: !h.connected, target: true } }
                    span { class: "topbar-counts mono wide-only", {model::counts_line(&h.hosts, panes.len())} }
                    if !h.connected {
                        span { class: "reconnecting",
                            Glyph { icon: Icon::WifiOff, size: 14 }
                            "Reconnecting"
                            button { class: "help-link", r#type: "button", onclick: move |_| super::connect::open_connect_help(None), "Can't connect?" }
                        }
                    }
                }
            }
            div { class: "content",
                div { class: "content-main",
                    if !h.loaded {
                        div { class: "skeletons", "aria-label": "Loading",
                            for i in 0..4 {
                                div { key: "{i}", class: "skeleton" }
                            }
                        }
                    } else if h.hosts.is_empty() {
                        div { class: "empty",
                            div { class: "empty-icon", Logo { class: "mark-drawn logo-28" } }
                            p { class: "empty-title", "No machines yet" }
                            p { class: "empty-text", "Pair a machine or add one over SSH to see the coding agents running in tmux on it." }
                            a { class: "button button-primary button-md press empty-action", href: "/next/machines?add=1", "Add a machine →" }
                        }
                    } else {
                        div {
                            h1 { class: "title", "Overview" }
                            p { class: "lede", span { class: "truncate", {model::status_line(&panes)} } }
                        }
                        super::companion::CompanionBar {}
                        section {
                            div { class: "section-label",
                                h2 { "Agent sessions" }
                                if filtering {
                                    div { class: "segmented", role: "radiogroup", "aria-label": "Show",
                                        for f in Filter::ALL {
                                            button {
                                                key: "{f.label()}",
                                                class: if f == shown_filter { "segment segment-on" } else { "segment" },
                                                r#type: "button",
                                                role: "radio",
                                                "aria-checked": "{f == shown_filter}",
                                                onclick: move |_| filter.set(f),
                                                "{f.label()}"
                                            }
                                        }
                                    }
                                }
                            }
                            if groups.is_empty() && !panes.is_empty() {
                                div { class: "empty",
                                    p { class: "empty-title", "Nothing here" }
                                    p { class: "empty-text", "Try another filter." }
                                }
                            } else if groups.is_empty() {
                                div { class: "empty",
                                    p { class: "empty-title", "No agent sessions" }
                                    p { class: "empty-text", "Start Claude Code, Codex or another agent in tmux on any machine, or press N in the current app to start one from here." }
                                }
                            } else if grouped {
                                div { class: "groups",
                                    for g in groups.iter() {
                                        {
                                            let id = g.host.config.id.clone();
                                            let open = !folded.read().contains(&format!("home:{id}"));
                                            let online = g.host.status == HostStatus::Online;
                                            rsx! {
                                                div { key: "{id}",
                                                    button {
                                                        class: "group-head",
                                                        r#type: "button",
                                                        "aria-expanded": "{open}",
                                                        onclick: { let id = id.clone(); move |_| toggle(id.clone()) },
                                                        Glyph { icon: Icon::ChevronRight, size: 16, class: if open { "chevron chevron-open" } else { "chevron" } }
                                                        MachineGlyph { kind: model::machine_kind(g.host), size: 16, class: if online { "fg-2" } else { "fg-3" } }
                                                        span { class: if online { "group-name" } else { "group-name fg-2" }, "{g.host.config.name}" }
                                                        if !online {
                                                            span { class: "group-state", if g.host.status == HostStatus::Error { "Offline" } else { "Connecting" } }
                                                        }
                                                        span { class: "group-count mono tabular", "{g.panes.len()}" }
                                                    }
                                                    div { class: "expand", "data-open": "{open}",
                                                        div { "inert": (!open).then_some(""),
                                                            div { class: "cards cards-grouped",
                                                                for r in g.panes.iter() {
                                                                    {card(r, false, clock)}
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                div { class: "cards",
                                    for g in groups.iter() {
                                        for r in g.panes.iter() {
                                            {card(r, true, clock)}
                                        }
                                    }
                                }
                            }
                        }
                        section {
                            div { class: "section-label",
                                h2 { "Machines" }
                                a { class: "section-link", href: "/next/machines", "Manage →" }
                            }
                            {machines(&h.hosts, clock)}
                        }
                    }
                }
                aside { class: "content-side",
                    {limits(&h.hosts, clock)}
                }
            }
        }
    }
}

/// An agent session as a card. The whole card opens the session; one that asks something shows its
/// answers in place and keeps its size, so nothing around it moves.
fn card(r: &PaneRef, show_host: bool, clock: Clock) -> Element {
    let a = r.agent();
    let shown = Shown::of(a, r.host);
    let title = model::pane_title(r);
    let href = model::pane_href(&r.host.config.id, &r.pane.id);
    let waiting = a.state == AgentState::Waiting;
    let st = a.status.as_ref();
    let cost = st.and_then(|s| s.cost_usd).map(|c| format::money(*c));
    let ctx = st.and_then(|s| s.context_pct).map(|p| *p);
    let ctx_title = format!("{} of {} tokens", format::tokens(st.and_then(|s| s.context_used).map(|n| *n)), format::tokens(st.and_then(|s| s.context_size).map(|n| *n)));
    let options: Vec<(u32, String)> = a.options.iter().flatten().map(|o| (o.n, o.label.clone())).collect();
    rsx! {
        div { key: "{r.key()}", class: if waiting { "card card-waiting" } else { "card" }, "data-waiting": waiting.then_some("true"),
            a { class: "card-link", href: "{href}", "aria-label": "{title}" }
            div { class: "card-top",
                AgentMark { kind: a.kind, size: 26 }
                span { class: "card-agent", {model::agent_name(a)} }
                span { class: "pill", title: a.background.clone(),
                    span { class: "dot dot-{shown.class()}" }
                    "{shown.label()}"
                }
            }
            div { class: "card-head",
                div { class: "card-title", "{title}" }
                div { class: "card-meta mono", {model::card_meta(r, show_host)} }
            }
            div { class: "card-line", {model::card_line(r, clock)} }
            div { class: "card-foot mono",
                if waiting && r.host.status == HostStatus::Online {
                    Answers { host: r.host.config.id.clone(), pane: r.pane.id.clone(), href: href.clone(), options }
                } else {
                    span { class: "truncate", {model::model_line(a)} }
                    if let Some(cost) = cost {
                        span { class: "tabular shrink-0", "{cost}" }
                    }
                    if let Some(pct) = ctx {
                        span { class: "card-ctx", title: "{ctx_title}",
                            span { class: "tabular", "{pct.round()}%" }
                            {meter(pct, "meter-sm")}
                        }
                    }
                }
            }
        }
    }
}

/// A waiting session's answers, in the row where the card shows its model and cost.
#[component]
fn Answers(host: String, pane: String, href: String, options: Vec<(u32, String)>) -> Element {
    let mut busy = use_signal(|| None::<u32>);
    let mut sent = use_signal(|| false);
    let mut failed = use_signal(|| None::<String>);
    if sent() {
        return rsx! {
            span { class: "answered fade-in", Glyph { icon: Icon::Check, size: 16 } "Sent" }
        };
    }
    if let Some(msg) = failed() {
        return rsx! {
            span { class: "answer-error fade-in", role: "alert", "Couldn't answer: {msg}" }
        };
    }
    if options.is_empty() {
        return rsx! {
            span { class: "answers",
                a { class: "answer answer-first press", href: "{href}", "data-answer": "", "Answer" }
            }
        };
    }
    rsx! {
        span { class: "answers",
            for (i, (n, label)) in options.iter().cloned().enumerate() {
                {
                    let host = host.clone();
                    let pane = pane.clone();
                    let kind = if i == 0 { "answer answer-first press" } else if label.to_lowercase().starts_with("no") { "answer answer-no press" } else { "answer press" };
                    rsx! {
                        button {
                            key: "{n}",
                            class: kind,
                            r#type: "button",
                            title: "{label}",
                            "data-answer": (i == 0).then_some(""),
                            disabled: busy().is_some(),
                            onclick: move |_| {
                                let (host, pane) = (host.clone(), pane.clone());
                                spawn(async move {
                                    busy.set(Some(n));
                                    match api::answer(&host, &pane, &api::Answer { option: n, text: None, question: None }).await {
                                        Ok(()) => {
                                            sent.set(true);
                                            // still asking after a moment: the choices again
                                            platform::sleep(4000).await;
                                            sent.set(false);
                                        }
                                        Err(e) => {
                                            failed.set(Some(e.message));
                                            platform::sleep(4000).await;
                                            failed.set(None);
                                        }
                                    }
                                    busy.set(None);
                                });
                            },
                            if busy() == Some(n) { "Sending…" } else { "{label}" }
                        }
                    }
                }
            }
        }
    }
}

fn meter(pct: f64, class: &str) -> Element {
    let v = pct.clamp(0.0, 100.0);
    rsx! {
        span { class: "meter {class}",
            span { class: "meter-fill meter-{model::meter_tone(v)}", style: "transform: scaleX({v / 100.0})" }
        }
    }
}

pub(super) fn host_dot(status: HostStatus) -> Element {
    let s = match status {
        HostStatus::Online => "online",
        HostStatus::Connecting => "connecting",
        HostStatus::Error => "error",
    };
    rsx! { span { class: "host-dot host-dot-{s}" } }
}

/// The agents installed on a machine: in colour when one runs there now, grey when only installed.
pub(super) fn badges(h: &HostState, size: u32) -> Element {
    let list = model::agent_badges(h);
    if list.is_empty() {
        return rsx! { span { class: "fg-3", "—" } };
    }
    rsx! {
        span { class: "badges",
            for (kind, n) in list {
                span {
                    key: "{kind.as_str()}",
                    class: if n > 0 { "badge" } else { "badge badge-idle" },
                    title: if n > 0 { format!("{}: {n} running", kind.def().name) } else { format!("{}: installed, not running", kind.def().name) },
                    AgentMark { kind, size }
                    if n > 1 {
                        span { class: "mono tabular fg-3 badge-n", "{n}" }
                    }
                }
            }
        }
    }
}

/// The machines: a list in a narrow column, a table once there's room for one (about 600px).
fn machines(hosts: &[HostState], clock: Clock) -> Element {
    rsx! {
        div { class: "machines",
            div { class: "panel machine-list",
                for h in hosts.iter() {
                    a {
                        key: "{h.config.id}",
                        class: if h.status == HostStatus::Online { "machine-row" } else { "machine-row dim" },
                        href: "{model::machine_href(&h.config.id)}",
                        {host_dot(h.status)}
                        div { class: "machine-text",
                            div { class: "machine-name-row",
                                span { class: "machine-name", "{h.config.name}" }
                                {badges(h, 15)}
                            }
                            div { class: "machine-line mono", {model::machine_line(h, clock)} }
                        }
                        Glyph { icon: Icon::ChevronRight, size: 16, class: "fg-3" }
                    }
                }
            }
            div { class: "panel machine-table",
                table {
                    thead {
                        tr {
                            th { "Machine" }
                            th { title: "In colour: running now. Grey: installed.", "Agents" }
                            th { "tmux" }
                            th { class: "num", title: "How long the machine took to report its sessions at the last check", "Response" }
                        }
                    }
                    tbody {
                        for h in hosts.iter() {
                            tr { key: "{h.config.id}", class: if h.status == HostStatus::Online { "" } else { "dim" },
                                td {
                                    a { class: "machine-cell", href: "{model::machine_href(&h.config.id)}",
                                        {host_dot(h.status)}
                                        div { class: "machine-text",
                                            div { class: "machine-name", "{h.config.name}" }
                                            div { class: "machine-line mono", {model::machine_address(h, hosts)} }
                                        }
                                    }
                                }
                                td { {badges(h, 18)} }
                                td { class: "fg-2", {model::tmux_cell(h)} }
                                td { class: "num mono tabular fg-2",
                                    {h.latency_ms.map_or("—".to_string(), format::response_time)}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Plan limits per machine and agent.
fn limits(hosts: &[HostState], clock: Clock) -> Element {
    let rows = model::limit_rows(hosts);
    if rows.is_empty() {
        return rsx! {};
    }
    rsx! {
        section { class: "panel limits",
            h2 { class: "limits-title", Glyph { icon: Icon::Gauge, size: 16, class: "fg-3" } "Plan limits" }
            div { class: "limits-rows",
                for row in rows {
                    div { key: "{row.host_id}{row.kind.as_str()}",
                        div { class: "limits-head",
                            AgentMark { kind: row.kind, size: 16 }
                            span { class: "limits-host", "{row.host}" }
                            if let Some(n) = row.resets {
                                span { class: "limits-banked mono", title: "Limit resets this account has banked. Inyon never uses them.",
                                    if n == 1 { "1 reset banked" } else { "{n} resets banked" }
                                }
                            }
                        }
                        for (label, w) in [("5-hour", format::live_window(row.five_hour.as_ref(), clock)), ("Weekly", format::live_window(row.seven_day.as_ref(), clock))] {
                            if let Some(w) = w {
                                div { key: "{label}", class: "limit",
                                    div { class: "limit-line",
                                        span { class: "fg-2", "{label}" }
                                        span { class: "limit-reset mono", if w.reset { "reset" } else { {format::resets_in(w.resets_at, clock)} } }
                                        span { class: "limit-pct mono tabular", "{w.pct.round()}%" }
                                    }
                                    {meter(w.pct, "")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
