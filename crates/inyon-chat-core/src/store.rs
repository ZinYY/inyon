//! The pure part of `store.ts`: the machines from every hub merged into one overview, every chat,
//! the notices a hub message raises, toasts, the activity list and the settings they read.
//!
//! What the TS does to the outside world (a toast's timer, a buzz, a phone or browser
//! notification, taking one back) comes back as [`Effect`]s for the app to carry out, and what it
//! reads from the page (the route on screen, whether the app is in front, whether it runs as the
//! phone app) goes in as a [`Context`].

use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use inyon_protocol::{AgentKind, AgentState, HostState, HostStatus, NotifyKind, Organize, ServerMsg};
use serde::{Deserialize, Serialize};

use crate::chat_list::{chat_key_of, Chat, Splice};
use crate::js::{cut16, encode_uri_component, len16, trim_end};
use crate::overview::{all_panes, pane_slug};

// ---------- paired hubs (hubs.ts) ----------

/// A hub the phone app paired with; only what the overview needs of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedHub {
    pub id: String,
    pub name: String,
    /// the hub's origin, https://hub.example or http://box:4777
    pub url: String,
    pub paired_at: u64,
}

const HUB_PREFIX: &str = "hub:";

/// A paired hub's machine as the app names it: hub:<hub id>/<machine id>.
pub fn hub_host_id(hub: &str, id: &str) -> String {
    format!("{HUB_PREFIX}{hub}/{id}")
}

/// The paired hub a machine comes through, and its id there. A machine of a hub that was
/// forgotten keeps its id as it is, so nothing is sent anywhere.
pub fn split_host_id<'a, 'h>(id: &'a str, hubs: &'h [PairedHub]) -> (Option<&'h PairedHub>, &'a str) {
    let Some(rest) = id.strip_prefix(HUB_PREFIX) else { return (None, id) };
    let Some((hub, machine)) = rest.split_once('/') else { return (None, id) };
    match hubs.iter().find(|h| h.id == hub) {
        Some(h) => (Some(h), machine),
        None => (None, id),
    }
}

/// Machines that share a name are told apart by the hub they come through: the machine had
/// longest keeps its name (this hub's own, else the first paired hub's; on a phone every machine
/// comes through a hub, and they answer in any order).
pub fn tell_apart(hosts: Vec<HostState>, hubs: &[PairedHub]) -> Vec<HostState> {
    let since = |h: &HostState| split_host_id(&h.config.id, hubs).0.map_or(0, |hub| hub.paired_at);
    let mut keeps: HashMap<String, usize> = HashMap::new();
    for (i, h) in hosts.iter().enumerate() {
        match keeps.get(&h.config.name) {
            Some(k) if since(h) >= since(&hosts[*k]) => {}
            _ => {
                keeps.insert(h.config.name.clone(), i);
            }
        }
    }
    hosts
        .into_iter()
        .enumerate()
        .map(|(i, mut h)| {
            if keeps.get(&h.config.name) == Some(&i) {
                return h;
            }
            if let (Some(hub), _) = split_host_id(&h.config.id, hubs) {
                let url = &hub.url;
                let where_ = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
                h.config.name = format!("{} · {where_}", h.config.name);
            }
            h
        })
        .collect()
}

// ---------- the overview ----------

/// Every machine the app shows, from the hub it's served by (or the phone's own core) and each
/// paired hub, one list for the screens.
#[derive(Debug, Clone, Default)]
pub struct Overview {
    /// each source's machines: "main", then "hub:<id>" in the order they first answered
    sources: IndexMap<String, Vec<HostState>>,
    /// panes ended from here, until when they stay hidden (unix ms)
    ended: IndexMap<(String, String), u64>,
    pub hosts: Vec<HostState>,
    /// an overview has been drawn (from the hub, or the copy kept on the device)
    pub loaded: bool,
    /// the overview is the copy kept on this device, not yet confirmed by the hub
    pub stale: bool,
    /// how each source organises its sessions (from 0.24; an older hub sends none)
    pub organize: IndexMap<String, Organize>,
}

impl Overview {
    /// The overview this device saw last, drawn first; the hub's replaces it in place.
    pub fn remembered(hosts: Vec<HostState>, hubs: &[PairedHub]) -> Self {
        let mut o = Overview { loaded: true, stale: true, ..Default::default() };
        o.remember_sources(&hosts, hubs);
        o.hosts = hosts;
        o
    }

    /// The remembered machines go back to where they came from, so the first live overview (on a
    /// phone, its own core's, which answers at once) replaces only its own part: a paired hub that
    /// hasn't answered yet keeps its machines on the list instead of leaving a blank page.
    pub fn remember_sources(&mut self, hosts: &[HostState], hubs: &[PairedHub]) {
        self.sources.clear();
        for h in hosts {
            let key = match split_host_id(&h.config.id, hubs).0 {
                Some(hub) => format!("hub:{}", hub.id),
                None => "main".into(),
            };
            self.sources.entry(key).or_default().push(h.clone());
        }
    }

    /// An overview from one source. Returns every machine as the app should keep it on the device
    /// (before panes ended from here are taken out).
    pub fn apply(&mut self, hosts: Vec<HostState>, source: Option<&str>, hubs: &[PairedHub], now: u64) -> Vec<HostState> {
        self.sources.insert(source.unwrap_or("main").to_string(), hosts);
        let all = tell_apart(self.sources.values().flatten().cloned().collect(), hubs);
        self.hosts = self.without_ended(all.clone(), now);
        self.loaded = true;
        self.stale = false;
        all
    }

    /// Every source's marks and projects as one (see [`crate::organize`]).
    pub fn organize(&self) -> Organize {
        crate::organize::merge(self.organize.values())
    }

    /// Panes ended from here: gone from every list at once, and kept out of the updates that still
    /// list them for a moment while tmux closes them. `false` brings one back (the end failed).
    pub fn hide_pane(&mut self, host: &str, pane: &str, hide: bool, now: u64) {
        let k = (host.to_string(), pane.to_string());
        if hide {
            self.ended.insert(k, now + 15_000);
            let hosts = std::mem::take(&mut self.hosts);
            self.hosts = self.without_ended(hosts, now);
        } else {
            self.ended.shift_remove(&k);
        }
    }

    fn without_ended(&mut self, hosts: Vec<HostState>, now: u64) -> Vec<HostState> {
        self.ended.retain(|_, until| *until >= now);
        if self.ended.is_empty() {
            return hosts;
        }
        hosts
            .into_iter()
            .map(|mut h| {
                let id = h.config.id.clone();
                if !self.ended.keys().any(|(host, _)| *host == id) {
                    return h;
                }
                for s in &mut h.sessions {
                    for w in &mut s.windows {
                        w.panes.retain(|p| !self.ended.contains_key(&(id.clone(), p.id.clone())));
                    }
                    s.windows.retain(|w| !w.panes.is_empty());
                }
                h.sessions.retain(|s| !s.windows.is_empty());
                h
            })
            .collect()
    }

    /// Signing out takes the device's copies with it.
    pub fn clear(&mut self) {
        *self = Overview::default();
    }
}

// ---------- toasts, activity, settings ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// given by [`Toasts::push`]
    pub id: u64,
    pub title: String,
    pub body: Option<String>,
    pub tone: Tone,
    pub href: Option<String>,
    pub agent: Option<AgentKind>,
    /// the label of one small action on the toast, such as Undo
    pub action: Option<String>,
}

impl Toast {
    pub fn new(title: impl Into<String>, tone: Tone) -> Self {
        Toast { id: 0, title: title.into(), body: None, tone, href: None, agent: None, action: None }
    }
}

/// At most three toasts at a time, newest last; one per conversation.
#[derive(Debug, Clone, Default)]
pub struct Toasts {
    pub list: Vec<Toast>,
    next: u64,
}

impl Toasts {
    /// Shows a toast. Returns its id and how long it stays (ms), for the app's timer to dismiss it.
    pub fn push(&mut self, mut t: Toast) -> (u64, u64) {
        self.next += 1;
        t.id = self.next;
        // one per session: a newer notice about the same conversation replaces the older one
        let href = t.href.clone().filter(|h| !h.is_empty());
        self.list.retain(|x| href.is_none() || x.href != href);
        let keep = self.list.len().saturating_sub(2);
        self.list.drain(..keep);
        // one with an action (Undo) stays a little longer, so there is time to reach it
        let ms = if t.tone == Tone::Error || t.action.is_some() { 6000 } else { 4500 };
        self.list.push(t);
        (self.next, ms)
    }

    pub fn dismiss(&mut self, id: u64) {
        self.list.retain(|t| t.id != id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActivityKind {
    Waiting,
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityEvent {
    pub id: u64,
    pub at: u64,
    pub host: String,
    pub pane: String,
    pub kind: ActivityKind,
    pub title: String,
    pub body: String,
    pub agent: Option<AgentKind>,
}

/// The activity list keeps the latest 30.
const ACTIVITY_KEPT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyFontSetting {
    Auto,
    Serif,
    Sans,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolDetail {
    All,
    Compact,
}

/// The app's settings, as the web app keeps them (`inyon.settings.v2`): a saved copy fills in
/// over the defaults, key by key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: Theme,
    pub agent_skins: bool,
    pub reply_font: ReplyFontSetting,
    pub density: Density,
    pub term_font_size: f64,
    pub notify_done: bool,
    /// a notice when a task on a page's progress board is done (from 0.22.4)
    pub notify_pages: bool,
    pub side_panel: bool,
    pub haptics: bool,
    /// chat: every tool call and background job as its own row ('all'), or long runs folded ('compact')
    pub tool_detail: ToolDetail,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::System,
            agent_skins: true,
            reply_font: ReplyFontSetting::Auto,
            density: Density::Comfortable,
            term_font_size: 13.0,
            notify_done: true,
            notify_pages: true,
            side_panel: true,
            haptics: true,
            tool_detail: ToolDetail::All,
        }
    }
}

impl Settings {
    /// The saved settings over the defaults; anything unreadable gives the defaults.
    pub fn load(saved: &str) -> Self {
        serde_json::from_str(saved).unwrap_or_default()
    }

    /// `setSettings`: `patch` (a JSON object of some settings) over these.
    pub fn merged(&self, patch: &serde_json::Value) -> Self {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let (Some(o), Some(p)) = (v.as_object_mut(), patch.as_object()) {
            o.extend(p.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        serde_json::from_value(v).unwrap_or_else(|_| self.clone())
    }
}

// ---------- notices ----------

/// What the app knows about where it is, for deciding whether to toast or notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context<'a> {
    /// the app's route on screen, without the base path (`routeOf(location.pathname)`)
    pub route: &'a str,
    /// the pane on screen shows its terminal (?view=terminal), not its chat
    pub terminal_view: bool,
    /// the app is in front (`document.visibilityState === 'visible'`)
    pub visible: bool,
    /// running as the phone app
    pub native: bool,
    /// the browser may show notifications (permission granted)
    pub browser_notices: bool,
}

impl Context<'_> {
    /// The conversation at `href` is on screen, as its chat.
    fn here(&self, href: &str) -> bool {
        self.route == href && !self.terminal_view && self.visible
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Haptic {
    Success,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Waiting,
    Done,
    Offline,
    Online,
    Page,
}

impl From<NotifyKind> for NoticeKind {
    fn from(k: NotifyKind) -> Self {
        match k {
            NotifyKind::Waiting => NoticeKind::Waiting,
            NotifyKind::Done => NoticeKind::Done,
            NotifyKind::Offline => NoticeKind::Offline,
            NotifyKind::Online => NoticeKind::Online,
        }
    }
}

/// Something for the app to do about a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// a toast was added to [`Store::toasts`]; dismiss it after `ms`
    Toast {
        id: u64,
        ms: u64,
    },
    Haptic(Haptic),
    /// a notification from the phone app; one per `tag`, the next replaces it, tapping opens `href`
    NativeNotify {
        title: String,
        body: String,
        href: String,
        tag: String,
        kind: NoticeKind,
    },
    /// a notification from the browser itself; closes the one shown with the same `tag` first
    BrowserNotify {
        title: String,
        body: String,
        href: String,
        tag: String,
    },
    /// take back the notification with this tag (phone and browser): its question was answered
    ClearNotification {
        tag: String,
    },
}

/// One notification per session (or machine): the next one replaces it, opening it clears it.
pub fn notify_tag(host: &str, pane: Option<&str>) -> String {
    match pane {
        Some(p) => format!("{host}:{p}"),
        None => format!("machine:{host}"),
    }
}

/// The notices' memory between messages.
#[derive(Debug, Clone, Default)]
struct Notifier {
    /// sessions with a "needs you" notification up, by tag
    asking: IndexMap<String, (String, String)>,
    /// when each tag:kind last buzzed
    buzzed: HashMap<String, u64>,
    /// companion replies already shown, so one that comes twice shows once
    companion_seen: IndexSet<String>,
}

impl Notifier {
    /// The same prompt reported again within a few seconds doesn't buzz twice.
    fn buzz_once(&mut self, tag: &str, kind: &str, now: u64) -> bool {
        let k = format!("{tag}:{kind}");
        if self.buzzed.get(&k).is_some_and(|at| *at + 10_000 > now) {
            return false;
        }
        self.buzzed.insert(k, now);
        true
    }

    /// A question answered somewhere else (on the computer, say) takes its notification with it,
    /// so what's left in the shade still needs you.
    fn clear_answered(&mut self, hosts: &[HostState], out: &mut Vec<Effect>) {
        self.asking.retain(|tag, (host, pane)| {
            // a machine that went quiet says nothing about the question
            let Some(h) = hosts.iter().find(|x| x.config.id == *host).filter(|h| h.status == HostStatus::Online) else { return true };
            let p = h.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes).find(|p| p.id == *pane);
            if p.and_then(|p| p.agent.as_ref()).is_some_and(|a| a.state == AgentState::Waiting) {
                return true;
            }
            out.push(Effect::ClearNotification { tag: tag.clone() });
            false
        });
    }

    /// Notes or forgets the question a notification asks, before it's posted from the background.
    fn track(&mut self, tag: &str, kind: NotifyKind, host: &str, pane: &str) {
        if kind == NotifyKind::Waiting {
            self.asking.insert(tag.to_string(), (host.to_string(), pane.to_string()));
        } else {
            self.asking.shift_remove(tag);
        }
    }
}

// ---------- the store ----------

/// How the app changed after a message.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    /// the machine list changed: the overview to save on the device (panes ended from here still in it)
    pub hosts: Option<Vec<HostState>>,
    /// the chat that changed, and how its rows did
    pub chat: Option<(String, Vec<Splice>)>,
    /// how the sessions are organised changed: all of it, every source together
    pub organize: Option<Organize>,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, Default)]
pub struct Store {
    pub overview: Overview,
    pub chats: HashMap<String, Chat>,
    pub toasts: Toasts,
    /// agent events, newest first
    pub activity: Vec<ActivityEvent>,
    pub settings: Settings,
    /// the hubs this phone is paired with (hubs.ts keeps them)
    pub hubs: Vec<PairedHub>,
    notifier: Notifier,
}

impl Store {
    pub fn new(settings: Settings) -> Self {
        Store { settings, ..Default::default() }
    }

    /// A chat by its key (`host:pane`), made empty the first time it's asked for.
    pub fn chat(&mut self, key: &str) -> &mut Chat {
        self.chats.entry(key.to_string()).or_default()
    }

    /// Shows a toast; the effect carries its timer.
    pub fn toast(&mut self, t: Toast) -> Effect {
        let (id, ms) = self.toasts.push(t);
        Effect::Toast { id, ms }
    }

    /// Applies one hub message. `source` is where an overview came from ("hub:<id>" for a paired
    /// hub's connection, none for the hub this app is served by or the phone's own core).
    pub fn apply(&mut self, msg: &ServerMsg, source: Option<&str>, ctx: &Context, now: u64) -> Update {
        let mut u = Update::default();
        if let Some(key) = chat_key_of(msg) {
            let splices = self.chat(key).apply(msg, now);
            u.chat = Some((key.to_string(), splices));
            return u;
        }
        match msg {
            ServerMsg::Overview { hosts, organize } => {
                let all = self.overview.apply(hosts.clone(), source, &self.hubs, now);
                let src = source.unwrap_or("main").to_string();
                let before = self.overview.organize.get(&src).cloned();
                match organize {
                    Some(o) => self.overview.organize.insert(src.clone(), (**o).clone()),
                    None => self.overview.organize.shift_remove(&src),
                };
                if self.overview.organize.get(&src) != before.as_ref() {
                    u.organize = Some(self.overview.organize());
                }
                self.notifier.clear_answered(&all, &mut u.effects);
                u.hosts = Some(all);
            }
            ServerMsg::Notify { host, pane, kind, title, body, agent } => self.notify(host, pane, *kind, title, body, *agent, ctx, now, &mut u.effects),
            ServerMsg::PageNotify { host, slug, title, body } => self.page_notify(host, slug, title, body, ctx, &mut u.effects),
            ServerMsg::CompanionReply { id, host, pane, text, error, .. } => self.companion_reply(id, host, pane, text, error.unwrap_or(false), ctx, &mut u.effects),
            _ => {}
        }
        u
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(&mut self, host: &str, pane: &str, kind: NotifyKind, title: &str, body: &str, agent: Option<AgentKind>, ctx: &Context, now: u64, out: &mut Vec<Effect>) {
        let machine = matches!(kind, NotifyKind::Offline | NotifyKind::Online);
        let href = if machine { format!("/machines/{}", encode_uri_component(host)) } else { format!("/m/{}/p/{}", encode_uri_component(host), pane_slug(pane)) };
        if !machine {
            let kind = if kind == NotifyKind::Waiting { ActivityKind::Waiting } else { ActivityKind::Done };
            self.activity.insert(0, ActivityEvent { id: now, at: now, host: host.into(), pane: pane.into(), kind, title: title.into(), body: body.into(), agent });
            self.activity.truncate(ACTIVITY_KEPT);
        }
        if kind == NotifyKind::Done && !self.settings.notify_done {
            return;
        }
        // don't toast about the conversation you're looking at (its chat shows the reply already)
        let here = ctx.here(&href);
        if !here {
            let tone = if matches!(kind, NotifyKind::Waiting | NotifyKind::Offline) { Tone::Warn } else { Tone::Success };
            out.push(self.toast(Toast { body: Some(body.into()), href: Some(href.clone()), agent, ..Toast::new(title, tone) }));
        }
        let tag = notify_tag(host, (!machine).then_some(pane));
        // two short pulses when an agent needs you or finishes, once per prompt; a buzz for a lost machine
        if ctx.visible {
            let kind_name = if kind == NotifyKind::Waiting { "waiting" } else { "done" };
            if matches!(kind, NotifyKind::Waiting | NotifyKind::Done) && self.notifier.buzz_once(&tag, kind_name, now) {
                out.push(Effect::Haptic(Haptic::Success));
            } else if kind == NotifyKind::Offline && !here {
                out.push(Effect::Haptic(Haptic::Warning));
            }
            return;
        }
        if ctx.native {
            self.notifier.track(&tag, kind, host, pane);
            out.push(Effect::NativeNotify { title: title.into(), body: body.into(), href, tag, kind: kind.into() });
        } else if ctx.browser_notices {
            self.notifier.track(&tag, kind, host, pane);
            out.push(Effect::BrowserNotify { title: title.into(), body: body.into(), href, tag });
        }
    }

    /// A page's board moved on: a toast unless that page is on screen, and a notification while the
    /// app is hidden. One per page; the next replaces it.
    fn page_notify(&mut self, host: &str, slug: &str, title: &str, body: &str, ctx: &Context, out: &mut Vec<Effect>) {
        if !self.settings.notify_pages {
            return;
        }
        let href = format!("/pages/{}/{slug}", encode_uri_component(host));
        let tag = format!("page:{host}/{slug}");
        if ctx.visible {
            if ctx.route != href {
                out.push(self.toast(Toast { body: Some(body.into()), href: Some(href), ..Toast::new(title, Tone::Success) }));
            }
            return;
        }
        if ctx.native {
            out.push(Effect::NativeNotify { title: title.into(), body: body.into(), href, tag, kind: NoticeKind::Page });
        } else if ctx.browser_notices {
            out.push(Effect::BrowserNotify { title: title.into(), body: body.into(), href, tag });
        }
    }

    /// The companion agent finished a turn: its reply as a toast unless its chat is open, and as a
    /// notification while the phone app is in the background.
    #[allow(clippy::too_many_arguments)]
    fn companion_reply(&mut self, id: &str, host: &str, pane: &str, text: &str, error: bool, ctx: &Context, out: &mut Vec<Effect>) {
        if !self.notifier.companion_seen.insert(id.to_string()) {
            return;
        }
        if self.notifier.companion_seen.len() > 50 {
            self.notifier.companion_seen.shift_remove_index(0);
        }
        let href = format!("/m/{}/p/{}", encode_uri_component(host), pane_slug(pane));
        let body = if len16(text) > 240 { format!("{}…", trim_end(cut16(text, 240))) } else { text.to_string() };
        let agent = all_panes(&self.overview.hosts).into_iter().find(|r| r.host.config.id == host && r.pane.id == pane).and_then(|r| r.pane.agent.as_ref()).map(|a| a.kind);
        if !ctx.here(&href) {
            let tone = if error { Tone::Error } else { Tone::Info };
            out.push(self.toast(Toast { body: Some(body.clone()), href: Some(href.clone()), agent, ..Toast::new("Companion", tone) }));
        }
        if !ctx.visible && ctx.native {
            out.push(Effect::NativeNotify { title: "Companion".into(), body, href, tag: notify_tag(host, Some(pane)), kind: NoticeKind::Done });
        }
    }

    /// Hides a pane being ended, everywhere at once; `false` brings it back (the end failed).
    pub fn hide_pane(&mut self, host: &str, pane: &str, hide: bool, now: u64) {
        self.overview.hide_pane(host, pane, hide, now);
    }

    /// A hub that forgot this phone: its machines are gone from the lists, and the person should know.
    pub fn hub_forgot(&mut self, name: &str) -> Effect {
        self.toast(Toast { body: Some("Pair it again from the hub to see its machines here.".into()), ..Toast::new(format!("{name} forgot this phone"), Tone::Warn) })
    }

    /// Signing out takes the device's copies with it.
    pub fn sign_out(&mut self) {
        self.overview.clear();
        self.chats.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{agent_pane, host, item};
    use serde_json::json;

    const NOW: u64 = 1_791_720_000_000;
    const HOME: Context = Context { route: "/", terminal_view: false, visible: true, native: false, browser_notices: false };
    const PHONE_HIDDEN: Context = Context { route: "/", terminal_view: false, visible: false, native: true, browser_notices: false };
    const TAB_HIDDEN: Context = Context { route: "/", terminal_view: false, visible: false, native: false, browser_notices: true };

    fn overview(hosts: Vec<HostState>) -> ServerMsg {
        ServerMsg::Overview { hosts, organize: None }
    }
    fn ids(s: &Store) -> Vec<String> {
        let mut v: Vec<String> = s.overview.hosts.iter().map(|h| h.config.id.clone()).collect();
        v.sort();
        v
    }
    fn bare(id: &str) -> HostState {
        host(id, vec![], json!({}))
    }
    fn hub(id: &str, url: &str, paired_at: u64) -> PairedHub {
        PairedHub { id: id.into(), name: "abhash-arm".into(), url: url.into(), paired_at }
    }

    #[test]
    fn overview_replaces_the_machines() {
        let mut s = Store::default();
        assert!(!s.overview.loaded);
        let u = s.apply(&overview(vec![bare("a"), bare("b")]), None, &HOME, NOW);
        assert_eq!(s.overview.hosts.iter().map(|h| h.config.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(s.overview.loaded && !s.overview.stale);
        assert_eq!(u.hosts.unwrap().len(), 2);
    }

    #[test]
    fn a_paired_hubs_remembered_machines_stay_while_the_phones_own_core_answers_first() {
        let hubs = vec![hub("h1", "http://box:4777", 1)];
        let mut s = Store { hubs: hubs.clone(), overview: Overview::remembered(vec![bare("own"), bare("hub:h1/srv")], &hubs), ..Default::default() };
        assert!(s.overview.stale);
        s.apply(&overview(vec![bare("own"), bare("new")]), None, &HOME, NOW);
        assert_eq!(ids(&s), ["hub:h1/srv", "new", "own"]);
        // the hub answers: its part is replaced in turn
        s.apply(&overview(vec![]), Some("hub:h1"), &HOME, NOW);
        assert_eq!(ids(&s), ["new", "own"]);
    }

    #[test]
    fn machines_that_share_a_name_are_told_apart_by_their_hub() {
        let named = |id: &str, name: &str| host(id, vec![], json!({ "config": { "id": id, "name": name, "kind": "node" } }));
        let names = |hosts: Vec<HostState>, hubs: &[PairedHub]| tell_apart(hosts, hubs).into_iter().map(|h| h.config.name).collect::<Vec<_>>();
        let one = [hub("h2", "http://100.97.188.112:47321", 1)];
        assert_eq!(
            names(vec![named("local", "abhash-arm"), named(&hub_host_id("h2", "local"), "abhash-arm"), named("db", "db")], &one),
            ["abhash-arm", "abhash-arm · 100.97.188.112:47321", "db"]
        );
        // on a phone both come through hubs, in whichever order they answer: the one paired first keeps its name
        let two = [hub("h1", "https://abhash-arm", 1), hub("h2", "http://100.97.188.112:47321", 2)];
        let both = vec![named(&hub_host_id("h1", "local"), "abhash-arm"), named(&hub_host_id("h2", "local"), "abhash-arm")];
        assert_eq!(names(both.clone(), &two), ["abhash-arm", "abhash-arm · 100.97.188.112:47321"]);
        assert_eq!(names(both.into_iter().rev().collect(), &two), ["abhash-arm · 100.97.188.112:47321", "abhash-arm"]);
        // a name nobody shares stays as it is
        assert_eq!(names(vec![named(&hub_host_id("h2", "local"), "abhash-arm")], &one), ["abhash-arm"]);
    }

    #[test]
    fn host_ids_of_paired_hubs() {
        let hubs = [hub("h1", "http://x", 1)];
        let id = hub_host_id("h1", "laptop");
        assert_eq!(id, "hub:h1/laptop");
        assert_eq!(split_host_id(&id, &hubs).1, "laptop");
        assert_eq!(split_host_id("laptop", &hubs), (None, "laptop"));
        // a hub that was forgotten leaves the id as it is
        assert_eq!(split_host_id(&id, &[]), (None, id.as_str()));
    }

    #[test]
    fn a_pane_ended_from_here_is_hidden_until_tmux_lets_it_go() {
        let mut s = Store::default();
        let box_ = || host("box", vec![agent_pane("%1", "claude", json!({})), agent_pane("%2", "codex", json!({}))], json!({}));
        s.apply(&overview(vec![box_()]), None, &HOME, NOW);
        s.hide_pane("box", "%1", true, NOW);
        let panes = |s: &Store| all_panes(&s.overview.hosts).iter().map(|r| r.pane.id.clone()).collect::<Vec<_>>();
        assert_eq!(panes(&s), ["%2"]);
        // the next update still lists it: it stays out
        let u = s.apply(&overview(vec![box_()]), None, &HOME, NOW + 1000);
        assert_eq!(panes(&s), ["%2"]);
        // what's kept on the device is the hub's own list
        assert_eq!(all_panes(&u.hosts.unwrap()).len(), 2);
        // fifteen seconds on, a pane tmux still lists is back
        s.apply(&overview(vec![box_()]), None, &HOME, NOW + 15_001);
        assert_eq!(panes(&s), ["%1", "%2"]);
        // and the end failing brings it back at once
        s.hide_pane("box", "%2", true, NOW + 20_000);
        s.hide_pane("box", "%2", false, NOW + 20_001);
        s.apply(&overview(vec![box_()]), None, &HOME, NOW + 20_002);
        assert_eq!(panes(&s), ["%1", "%2"]);
    }

    #[test]
    fn chat_messages_go_to_their_chat_and_say_how_its_rows_changed() {
        let mut s = Store::default();
        let msg = ServerMsg::ChatReset { key: "box:%1".into(), info: Box::new(crate::test_support::chat_info(json!({}))), items: vec![item::user("hi")], has_more: false };
        let u = s.apply(&msg, None, &HOME, NOW);
        let (key, splices) = u.chat.unwrap();
        assert_eq!(key, "box:%1");
        assert_eq!(splices.len(), 1);
        assert_eq!(s.chats["box:%1"].rows().len(), 1);
    }

    fn reply(id: &str, text: &str) -> ServerMsg {
        ServerMsg::CompanionReply { id: id.into(), host: "local".into(), pane: "%9".into(), text: text.into(), error: None, at: 1 }
    }
    fn toasts(s: &Store) -> Vec<(String, Option<String>, Option<String>, Tone)> {
        s.toasts.list.iter().map(|t| (t.title.clone(), t.body.clone(), t.href.clone(), t.tone)).collect()
    }

    #[test]
    fn the_companions_reply_shows_once_as_a_toast_that_opens_its_chat_cut_short_when_long() {
        let mut s = Store::default();
        s.apply(&reply("r-1", "Codex on the laptop needs you."), None, &HOME, NOW);
        s.apply(&reply("r-1", "Codex on the laptop needs you."), None, &HOME, NOW);
        assert_eq!(toasts(&s), [("Companion".into(), Some("Codex on the laptop needs you.".into()), Some("/m/local/p/9".into()), Tone::Info)]);
        // with its agent's mark, once the overview shows the pane
        s.apply(&overview(vec![host("local", vec![agent_pane("%9", "codex", json!({ "companion": true }))], json!({}))]), None, &HOME, NOW);
        s.apply(&reply("r-1b", "again"), None, &HOME, NOW);
        assert_eq!(s.toasts.list.last().unwrap().agent.map(|a| a.as_str()), Some("codex"));
        let long = ServerMsg::CompanionReply { id: "r-2".into(), host: "local".into(), pane: "%9".into(), text: "word ".repeat(80), error: Some(true), at: 1 };
        s.apply(&long, None, &HOME, NOW);
        let last = s.toasts.list.last().unwrap();
        assert_eq!(last.tone, Tone::Error);
        let body = last.body.as_deref().unwrap();
        assert!(len16(body) <= 241);
        assert!(body.ends_with('…') && !body.ends_with(" …"));
    }

    #[test]
    fn the_companions_open_chat_holds_the_toast_back_its_terminal_does_not() {
        let mut s = Store::default();
        let chat = Context { route: "/m/local/p/9", ..HOME };
        s.apply(&reply("v-1", "Done."), None, &chat, NOW);
        assert!(s.toasts.list.is_empty());
        s.apply(&reply("v-2", "Done."), None, &Context { terminal_view: true, ..chat }, NOW);
        assert_eq!(s.toasts.list.iter().map(|t| t.body.clone().unwrap()).collect::<Vec<_>>(), ["Done."]);
    }

    #[test]
    fn the_companions_reply_notifies_from_the_phone_apps_background() {
        let mut s = Store::default();
        let u = s.apply(&reply("n-1", "Done."), None, &PHONE_HIDDEN, NOW);
        assert_eq!(
            u.effects[1],
            Effect::NativeNotify { title: "Companion".into(), body: "Done.".into(), href: "/m/local/p/9".into(), tag: "local:%9".into(), kind: NoticeKind::Done }
        );
    }

    #[test]
    fn at_most_three_toasts_at_a_time_newest_last_one_per_conversation() {
        let mut s = Store::default();
        let titles = |s: &Store| s.toasts.list.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        for n in 1..=4 {
            s.toast(Toast::new(format!("t{n}"), Tone::Info));
        }
        assert_eq!(titles(&s), ["t2", "t3", "t4"]);
        let first = s.toasts.list[0].id;
        s.toasts.dismiss(first);
        assert_eq!(titles(&s), ["t3", "t4"]);
        s.toast(Toast { href: Some("/m/box/p/1".into()), ..Toast::new("working", Tone::Info) });
        s.toast(Toast { href: Some("/m/box/p/1".into()), ..Toast::new("finished", Tone::Success) });
        assert_eq!(titles(&s), ["t3", "t4", "finished"]);
    }

    #[test]
    fn toasts_with_an_action_or_an_error_stay_longer() {
        let mut t = Toasts::default();
        assert_eq!(t.push(Toast::new("a", Tone::Info)).1, 4500);
        assert_eq!(t.push(Toast::new("b", Tone::Error)).1, 6000);
        assert_eq!(t.push(Toast { action: Some("Undo".into()), ..Toast::new("c", Tone::Success) }).1, 6000);
    }

    #[test]
    fn settings_merge_over_the_defaults() {
        let s = Settings::default().merged(&json!({ "density": "compact" }));
        assert_eq!((s.density, s.theme, s.haptics), (Density::Compact, Theme::System, true));
        let saved = serde_json::to_value(&s).unwrap();
        assert_eq!((saved["density"].as_str(), saved["haptics"].as_bool()), (Some("compact"), Some(true)));
        assert_eq!(Settings::load(r#"{"notifyDone":false,"unknown":1}"#), Settings { notify_done: false, ..Settings::default() });
        assert_eq!(Settings::load("not json"), Settings::default());
    }

    fn moved(s: &mut Store, host: &str, ctx: &Context) -> Update {
        s.apply(&ServerMsg::PageNotify { host: host.into(), slug: "robot".into(), title: "Robot".into(), body: "3 of 12 done · Payment sheet".into() }, None, ctx, NOW)
    }

    #[test]
    fn a_pages_board_moving_on_toasts_with_a_link_to_the_page_unless_that_page_is_open() {
        let mut s = Store::default();
        moved(&mut s, "hub:h1/box", &HOME);
        assert_eq!(toasts(&s), [("Robot".into(), Some("3 of 12 done · Payment sheet".into()), Some("/pages/hub%3Ah1%2Fbox/robot".into()), Tone::Success)]);
        assert!(s.activity.is_empty());
        s.toasts.list.clear();
        moved(&mut s, "laptop", &Context { route: "/pages/laptop/robot", ..HOME });
        assert!(s.toasts.list.is_empty());
    }

    #[test]
    fn page_notices_can_be_turned_off() {
        let mut s = Store::new(Settings { notify_pages: false, ..Settings::default() });
        assert!(moved(&mut s, "laptop", &HOME).effects.is_empty());
        assert!(s.toasts.list.is_empty());
    }

    #[test]
    fn in_the_phone_apps_background_one_native_notification_per_page_that_opens_it() {
        let mut s = Store::default();
        let u = moved(&mut s, "laptop", &PHONE_HIDDEN);
        assert_eq!(
            u.effects,
            [Effect::NativeNotify {
                title: "Robot".into(),
                body: "3 of 12 done · Payment sheet".into(),
                href: "/pages/laptop/robot".into(),
                tag: "page:laptop/robot".into(),
                kind: NoticeKind::Page
            }]
        );
        assert!(s.toasts.list.is_empty());
    }

    fn note(s: &mut Store, kind: NotifyKind, title: &str, ctx: &Context, now: u64) -> Update {
        s.apply(&ServerMsg::Notify { host: "box".into(), pane: "%3".into(), kind, title: title.into(), body: "b".into(), agent: Some(AgentKind::Claude) }, None, ctx, now)
    }

    #[test]
    fn agent_events_go_in_the_activity_list_and_toast_with_a_link_to_the_session() {
        let mut s = Store::default();
        let u = note(&mut s, NotifyKind::Waiting, "Claude needs you", &HOME, NOW);
        let a = &s.activity[0];
        assert_eq!((a.host.as_str(), a.pane.as_str(), a.kind), ("box", "%3", ActivityKind::Waiting));
        assert_eq!(toasts(&s), [("Claude needs you".into(), Some("b".into()), Some("/m/box/p/3".into()), Tone::Warn)]);
        assert_eq!(u.effects, [Effect::Toast { id: 1, ms: 4500 }, Effect::Haptic(Haptic::Success)]);
    }

    #[test]
    fn machine_events_toast_but_are_not_activity() {
        let mut s = Store::default();
        let u = note(&mut s, NotifyKind::Offline, "box is offline", &HOME, NOW);
        assert!(s.activity.is_empty());
        assert_eq!((s.toasts.list[0].tone, s.toasts.list[0].href.as_deref()), (Tone::Warn, Some("/machines/box")));
        assert_eq!(u.effects[1], Effect::Haptic(Haptic::Warning));
    }

    #[test]
    fn no_toast_for_the_conversation_on_screen() {
        let mut s = Store::default();
        note(&mut s, NotifyKind::Done, "done", &Context { route: "/m/box/p/3", ..HOME }, NOW);
        assert!(s.toasts.list.is_empty());
        assert_eq!(s.activity.len(), 1);
    }

    #[test]
    fn finished_notices_can_be_turned_off_and_still_go_in_the_activity_list() {
        let mut s = Store::new(Settings { notify_done: false, ..Settings::default() });
        note(&mut s, NotifyKind::Done, "done", &HOME, NOW);
        assert!(s.toasts.list.is_empty());
        assert_eq!(s.activity.len(), 1);
        note(&mut s, NotifyKind::Waiting, "asks", &HOME, NOW);
        assert_eq!(s.toasts.list.len(), 1);
    }

    #[test]
    fn the_activity_list_keeps_the_latest_30() {
        let mut s = Store::default();
        for i in 0..35 {
            note(&mut s, NotifyKind::Done, &format!("n{i}"), &HOME, NOW);
        }
        assert_eq!(s.activity.len(), 30);
        assert_eq!(s.activity[0].title, "n34");
    }

    #[test]
    fn the_same_prompt_buzzes_once_within_ten_seconds() {
        let mut s = Store::default();
        let buzzes = |u: &Update| u.effects.iter().filter(|e| matches!(e, Effect::Haptic(_))).count();
        assert_eq!(buzzes(&note(&mut s, NotifyKind::Waiting, "a", &HOME, NOW)), 1);
        assert_eq!(buzzes(&note(&mut s, NotifyKind::Waiting, "a", &HOME, NOW + 5_000)), 0);
        assert_eq!(buzzes(&note(&mut s, NotifyKind::Done, "a", &HOME, NOW + 5_000)), 1);
        assert_eq!(buzzes(&note(&mut s, NotifyKind::Waiting, "a", &HOME, NOW + 10_001)), 1);
    }

    #[test]
    fn in_the_phone_apps_background_one_native_notification_per_session() {
        let mut s = Store::default();
        let u = note(&mut s, NotifyKind::Waiting, "Claude needs you", &PHONE_HIDDEN, NOW);
        let off = s.apply(
            &ServerMsg::Notify { host: "box".into(), pane: "".into(), kind: NotifyKind::Offline, title: "box is offline".into(), body: "b".into(), agent: None },
            None,
            &PHONE_HIDDEN,
            NOW,
        );
        let native = |u: &Update| u.effects.iter().filter(|e| matches!(e, Effect::NativeNotify { .. })).cloned().collect::<Vec<_>>();
        assert_eq!(
            native(&u),
            [Effect::NativeNotify { title: "Claude needs you".into(), body: "b".into(), href: "/m/box/p/3".into(), tag: "box:%3".into(), kind: NoticeKind::Waiting }]
        );
        assert_eq!(
            native(&off),
            [Effect::NativeNotify { title: "box is offline".into(), body: "b".into(), href: "/machines/box".into(), tag: "machine:box".into(), kind: NoticeKind::Offline }]
        );
        assert_eq!(notify_tag("box", Some("%3")), "box:%3");
    }

    fn box_with(state: &str, status: &str) -> ServerMsg {
        overview(vec![host("box", vec![agent_pane("%3", "claude", json!({ "agent": { "kind": "claude", "pid": 1, "state": state } }))], json!({ "status": status }))])
    }
    fn cleared(u: &Update) -> Vec<String> {
        u.effects
            .iter()
            .filter_map(|e| match e {
                Effect::ClearNotification { tag } => Some(tag.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_question_answered_somewhere_else_takes_its_notification_with_it() {
        let mut s = Store::default();
        note(&mut s, NotifyKind::Waiting, "asks", &PHONE_HIDDEN, NOW);
        assert!(cleared(&s.apply(&box_with("waiting", "online"), None, &PHONE_HIDDEN, NOW)).is_empty());
        // still asking, or the machine went quiet: it stays
        assert!(cleared(&s.apply(&box_with("working", "error"), None, &PHONE_HIDDEN, NOW)).is_empty());
        // answered on the computer
        assert_eq!(cleared(&s.apply(&box_with("working", "online"), None, &PHONE_HIDDEN, NOW)), ["box:%3"]);
        assert!(cleared(&s.apply(&box_with("idle", "online"), None, &PHONE_HIDDEN, NOW)).is_empty());
        // a finished notice replaced it: nothing to take back
        note(&mut s, NotifyKind::Waiting, "asks", &PHONE_HIDDEN, NOW);
        note(&mut s, NotifyKind::Done, "done", &PHONE_HIDDEN, NOW);
        assert!(cleared(&s.apply(&box_with("idle", "online"), None, &PHONE_HIDDEN, NOW)).is_empty());
    }

    #[test]
    fn in_a_browser_tab_an_answered_questions_notification_is_closed_too_and_a_newer_one_replaces_the_old() {
        let mut s = Store::default();
        let a = note(&mut s, NotifyKind::Waiting, "first question", &TAB_HIDDEN, NOW);
        let b = note(&mut s, NotifyKind::Waiting, "second question", &TAB_HIDDEN, NOW);
        let browser = |u: &Update| u.effects.iter().find_map(|e| if let Effect::BrowserNotify { title, tag, .. } = e { Some((title.clone(), tag.clone())) } else { None });
        // the same tag: the app closes the first before showing the second
        assert_eq!(browser(&a), Some(("first question".into(), "box:%3".into())));
        assert_eq!(browser(&b), Some(("second question".into(), "box:%3".into())));
        assert!(cleared(&s.apply(&box_with("waiting", "online"), None, &TAB_HIDDEN, NOW)).is_empty());
        assert_eq!(cleared(&s.apply(&box_with("working", "online"), None, &TAB_HIDDEN, NOW)), ["box:%3"]);
    }

    #[test]
    fn signing_out_takes_the_copies_with_it_and_a_forgetful_hub_says_so() {
        let mut s = Store::default();
        s.apply(&overview(vec![bare("a")]), None, &HOME, NOW);
        s.chat("a:%1");
        s.sign_out();
        assert!(s.overview.hosts.is_empty() && !s.overview.loaded && s.chats.is_empty());
        s.hub_forgot("studio");
        assert_eq!(toasts(&s), [("studio forgot this phone".into(), Some("Pair it again from the hub to see its machines here.".into()), None, Tone::Warn)]);
    }

    #[test]
    fn each_sources_organize_is_kept_and_read_as_one() {
        let mut s = Store::default();
        let org = |pane: &str| -> Organize {
            serde_json::from_value(json!({ "v": 1, "sessions": [{ "id": format!("s-{pane}"), "host": "a", "pane": pane, "live": true, "pinned": 0 }] })).unwrap()
        };
        let msg = |o: Option<Organize>| ServerMsg::Overview { hosts: vec![bare("a")], organize: o.map(Box::new) };
        let u = s.apply(&msg(Some(org("%1"))), None, &HOME, NOW);
        assert_eq!(u.organize.unwrap().sessions.len(), 1);
        let u = s.apply(&msg(Some(org("%1"))), None, &HOME, NOW);
        assert!(u.organize.is_none(), "unchanged");
        let u = s.apply(&msg(Some(org("%2"))), Some("hub:x"), &HOME, NOW);
        assert_eq!(u.organize.unwrap().sessions.iter().map(|m| m.pane.as_str()).collect::<Vec<_>>(), ["%1", "%2"]);
        // an older hub sends none: its part goes
        let u = s.apply(&msg(None), Some("hub:x"), &HOME, NOW);
        assert_eq!(u.organize.unwrap().sessions.len(), 1);
        assert_eq!(s.overview.organize().sessions[0].pane, "%1");
    }
}
