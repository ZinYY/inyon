//! Notifications while the app is off screen (the Android build, from 1.0). The core's own thread
//! posts them as it hears each notice, so they arrive even while Android has frozen the window:
//! what apps/web/src/lib/store.ts's `notify` and `pageNotify` do for the Capacitor app.
//!
//! One notification per session (or machine, or page): the next replaces it. A question waits a
//! moment for the overview that carries its answers; one answered meanwhile gets none, and one
//! answered somewhere else takes its notification away.
//!
//! A question with one to three answers gets them as buttons (Kotlin's Notifications, which asks
//! for a fingerprint or the PIN on the lock screen before any answer that isn't a no). A tapped
//! answer comes back here ([`answer`]) and is sent as notifyAnswer.ts sends it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use inyon_chat_core::js::encode_uri_component;
use inyon_protocol::{AgentState, HostState, HostStatus, NotifyKind, ServerMsg};
use serde::{Deserialize, Serialize};

/// How long a question's notice waits for the overview with its answers (the web app's ANSWERS_WAIT_MS).
pub const ANSWERS_WAIT_MS: u64 = 400;

/// An answer a question's notification offers as a button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub n: u32,
    pub label: String,
}

/// One notification, as the Kotlin side posts it (Notifications.post).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub href: String,
    pub tag: String,
    /// waiting, done, offline, online or page: the channel and the buttons
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
}

/// Where notifications go: Android's shade, or a test's list.
pub trait Shade {
    fn post(&self, n: &Notice) -> bool;
    fn clear(&self, tag: &str);
}

/// What the person chose in Settings (`notifyDone`, `notifyPages`); both on unless turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wants {
    pub done: bool,
    pub pages: bool,
}

impl Wants {
    /// From the app's saved settings (`inyon.settings.v2`).
    pub fn from_settings(saved: Option<&str>) -> Wants {
        let v: serde_json::Value = saved.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        let on = |k: &str| v.get(k).and_then(serde_json::Value::as_bool).unwrap_or(true);
        Wants { done: on("notifyDone"), pages: on("notifyPages") }
    }
}

/// A session's (or a machine's) tag: its next notification replaces this one.
pub fn tag_of(host: &str, pane: Option<&str>) -> String {
    match pane {
        Some(p) => format!("{host}:{p}"),
        None => format!("machine:{host}"),
    }
}

/// The in-app path a tap opens, as the web app writes it.
pub fn href_of(host: &str, pane: Option<&str>) -> String {
    match pane {
        Some(p) => format!("/m/{}/p/{}", encode_uri_component(host), p.trim_start_matches('%')),
        None => format!("/machines/{}", encode_uri_component(host)),
    }
}

fn kind_name(k: NotifyKind) -> &'static str {
    match k {
        NotifyKind::Waiting => "waiting",
        NotifyKind::Done => "done",
        NotifyKind::Offline => "offline",
        NotifyKind::Online => "online",
    }
}

/// The answers a waiting session offers, when there are one to three of them (Android shows three
/// buttons at most); none otherwise.
pub fn choices_for(hosts: &[HostState], host: &str, pane: &str) -> Vec<Choice> {
    let agent = hosts
        .iter()
        .find(|h| h.config.id == host)
        .and_then(|h| h.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes).find(|p| p.id == pane))
        .and_then(|p| p.agent.as_ref());
    match agent {
        Some(a) if a.state == AgentState::Waiting => match a.options.as_deref() {
            Some(o) if (1..=3).contains(&o.len()) => o.iter().map(|o| Choice { n: o.n, label: o.label.clone() }).collect(),
            _ => vec![],
        },
        _ => vec![],
    }
}

/// How old a tap may be when it's sent (the web app's ANSWER_TTL_MS). Android may hold a tap while
/// the app is frozen; Kotlin's AnswerService gives up after 15 s and says the answer wasn't sent,
/// so one older than this is dropped rather than sent late.
pub const ANSWER_TTL_MS: f64 = 12_000.0;

/// An answer tapped on a question's notification (Kotlin's NotificationActions).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tap {
    pub host: String,
    pub pane: String,
    pub option: u32,
    pub label: String,
    pub tag: String,
    /// when it was tapped, in ms since the epoch
    pub at: f64,
}

/// What becomes of a tap.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// too old: Kotlin has already said it wasn't sent
    Late,
    /// the session no longer asks that question with that answer at that number
    Changed,
    Send,
}

/// Whether a tap may be sent: only while the session still asks that question with that answer at
/// that number, so a tap that comes late never lands on the next question.
pub fn judge(tap: &Tap, now: f64, hosts: &[HostState]) -> Verdict {
    if now - tap.at > ANSWER_TTL_MS {
        return Verdict::Late;
    }
    let agent = hosts
        .iter()
        .find(|h| h.config.id == tap.host)
        .and_then(|h| h.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes).find(|p| p.id == tap.pane))
        .and_then(|p| p.agent.as_ref());
    let asks = agent.is_some_and(|a| a.state == AgentState::Waiting && a.options.as_deref().unwrap_or_default().iter().any(|o| o.n == tap.option && o.label == tap.label));
    if asks {
        Verdict::Send
    } else {
        Verdict::Changed
    }
}

/// What the notifier remembers between notices.
pub struct Notifier {
    shade: Rc<dyn Shade>,
    /// sessions with a "needs you" notification up (or about to be), by tag: the host, the pane
    /// and which question it is
    asking: RefCell<HashMap<String, (String, String, u64)>>,
    /// the last question's number
    asked: Cell<u64>,
    /// the question each tag's notification shows now, and when it was posted: a tap from before
    /// that was on an older notification
    shown: RefCell<HashMap<String, (u64, f64)>>,
    /// ms since the epoch
    clock: Box<dyn Fn() -> f64>,
}

/// Which question a tap was on, from what the notifier posted.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Whose {
    /// the question the session asks now, by its number
    Live(u64),
    /// one since replaced by a newer question (shown, or about to be)
    Older,
    /// nothing to go by (the app started again since it was posted)
    Unknown,
}

/// The wall clock, in ms since the epoch (Kotlin stamps taps with System.currentTimeMillis).
pub fn wall_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

/// Kotlin's AnswerService says an answer wasn't sent 15 s after the tap; the send stops a second
/// before, so what it says stays true.
pub const ANSWER_SEND_BY_MS: f64 = 14_000.0;

/// How long a send that starts now may take, for a tap at `at`.
pub fn send_window_ms(at: f64, now: f64) -> u64 {
    (ANSWER_SEND_BY_MS - (now - at)).max(0.0) as u64
}

/// What to do once the overview with a question's answers has had time to arrive.
pub struct Later {
    pub tag: String,
    pub notice: Notice,
    /// which question: one asked again since replaces this one
    pub seq: u64,
}

impl Notifier {
    pub fn new(shade: Rc<dyn Shade>) -> Rc<Notifier> {
        Notifier::with_clock(shade, wall_ms)
    }

    pub fn with_clock(shade: Rc<dyn Shade>, clock: impl Fn() -> f64 + 'static) -> Rc<Notifier> {
        Rc::new(Notifier { shade, asking: RefCell::default(), asked: Cell::new(0), shown: RefCell::default(), clock: Box::new(clock) })
    }

    /// A notice from the core. Posted at once, unless it's a question: that one comes back as
    /// [`Later`], for [`Notifier::ask`] once its answers are known.
    pub fn heard(&self, msg: &ServerMsg, visible: bool, wants: Wants) -> Option<Later> {
        match msg {
            ServerMsg::Notify { host, pane, kind, title, body, .. } => {
                let machine = matches!(kind, NotifyKind::Offline | NotifyKind::Online);
                let pane = (!machine).then_some(pane.as_str());
                let tag = tag_of(host, pane);
                // any notice from the session ends the question it had, posted or not (a new question
                // takes its place below)
                let was_asking = self.asking.borrow_mut().remove(&tag).is_some();
                // on screen, the app shows it itself
                if (*kind == NotifyKind::Done && !wants.done) || visible {
                    if was_asking {
                        self.shown.borrow_mut().remove(&tag);
                        self.shade.clear(&tag);
                    }
                    return None;
                }
                let notice = Notice {
                    title: title.clone(),
                    body: body.clone(),
                    href: href_of(host, pane),
                    tag: tag.clone(),
                    kind: kind_name(*kind).into(),
                    host: pane.map(|_| host.clone()),
                    pane: pane.map(str::to_owned),
                    options: vec![],
                };
                if *kind != NotifyKind::Waiting {
                    self.shown.borrow_mut().remove(&tag);
                    self.shade.post(&notice);
                    return None;
                }
                let seq = self.asked.get() + 1;
                self.asked.set(seq);
                self.asking.borrow_mut().insert(tag.clone(), (host.clone(), pane.unwrap_or_default().to_owned(), seq));
                Some(Later { tag, notice, seq })
            }
            ServerMsg::PageNotify { host, slug, title, body } => {
                if !wants.pages || visible {
                    return None;
                }
                let tag = format!("page:{host}/{slug}");
                let href = format!("/pages/{}/{slug}", encode_uri_component(host));
                self.shade.post(&Notice { title: title.clone(), body: body.clone(), href, tag, kind: "page".into(), host: None, pane: None, options: vec![] });
                None
            }
            _ => None,
        }
    }

    /// Posts a question's notification with the answers the overview now shows, unless it was
    /// answered meanwhile or asked again (the newer one posts its own).
    pub fn ask(&self, later: Later, hosts: &[HostState]) {
        let Some((host, pane, seq)) = self.asking.borrow().get(&later.tag).cloned() else { return };
        if seq != later.seq {
            return;
        }
        let mut notice = later.notice;
        notice.options = choices_for(hosts, &host, &pane);
        self.shown.borrow_mut().insert(later.tag.clone(), (seq, (self.clock)()));
        self.shade.post(&notice);
    }

    fn whose(&self, tap: &Tap) -> Whose {
        let asks = self.asking.borrow().get(&tap.tag).map(|a| a.2);
        match (self.shown.borrow().get(&tap.tag).copied(), asks) {
            (Some((seq, at)), Some(now)) if seq == now && tap.at >= at => Whose::Live(seq),
            (None, None) => Whose::Unknown,
            // tapped before the notification it was on was replaced, or a newer question is
            // waiting for its answers
            (_, Some(_)) => Whose::Older,
            // the question it was on ended, and nothing replaced its notification
            (Some(_), None) => Whose::Unknown,
        }
    }

    /// Whether the session still asks the question a tap was on (or there's nothing to tell).
    fn still(&self, tag: &str, whose: Whose) -> bool {
        let asks = self.asking.borrow().get(tag).map(|a| a.2);
        match whose {
            Whose::Live(seq) => asks == Some(seq),
            Whose::Unknown => asks.is_none(),
            Whose::Older => false,
        }
    }

    /// The tag's question is over: its notification goes.
    fn settle(&self, tag: &str) {
        self.asking.borrow_mut().remove(tag);
        self.shown.borrow_mut().remove(tag);
        self.shade.clear(tag);
    }

    /// Sends a tapped answer with `send` (host, pane, option; an error says why it failed) when
    /// [`judge`] allows it. The notification goes once the agent has it, or says why it didn't.
    pub async fn answer<F, Fut>(&self, tap: Tap, now: f64, hosts: &[HostState], send: F)
    where
        F: FnOnce(String, String, u32) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        let href = href_of(&tap.host, Some(&tap.pane));
        let say = |title: &str, body: String, kind: &str| {
            self.shade.post(&Notice { title: title.into(), body, href: href.clone(), tag: tap.tag.clone(), kind: kind.into(), host: None, pane: None, options: vec![] });
        };
        let whose = self.whose(&tap);
        // a newer question took its place: that one's notification is what the shade shows (or
        // soon will), and a word about this tap would replace it
        if whose == Whose::Older {
            return;
        }
        match judge(&tap, now, hosts) {
            Verdict::Late => {}
            Verdict::Changed => {
                self.asking.borrow_mut().remove(&tap.tag);
                self.shown.borrow_mut().remove(&tap.tag);
                say("That question changed", format!("“{}” wasn't sent. Open the session to see what it asks now.", tap.label), "done");
            }
            Verdict::Send => {
                let sent = send(tap.host.clone(), tap.pane.clone(), tap.option).await;
                // the session may have asked something new meanwhile; its notification stays
                if !self.still(&tap.tag, whose) {
                    return;
                }
                match sent {
                    Ok(()) => self.settle(&tap.tag),
                    Err(why) => say("Couldn't send your answer", why, "waiting"),
                }
            }
        }
    }

    /// A question answered somewhere else (on the computer, say) takes its notification with it,
    /// so what's left in the shade still needs you.
    pub fn overview(&self, hosts: &[HostState]) {
        let answered: Vec<String> = self
            .asking
            .borrow()
            .iter()
            .filter(|(_, (host, pane, _))| {
                // a machine that went quiet says nothing about the question
                let Some(h) = hosts.iter().find(|h| &h.config.id == host).filter(|h| h.status == HostStatus::Online) else { return false };
                let p = h.sessions.iter().flat_map(|s| &s.windows).flat_map(|w| &w.panes).find(|p| &p.id == pane);
                p.and_then(|p| p.agent.as_ref()).is_none_or(|a| a.state != AgentState::Waiting)
            })
            .map(|(tag, _)| tag.clone())
            .collect();
        for tag in answered {
            self.settle(&tag);
        }
    }
}

thread_local! {
    /// the notifier [`start`] made, on the core's thread
    static NOTIFIER: RefCell<Option<Rc<Notifier>>> = const { RefCell::new(None) };
}

/// Sends an answer tapped on a notification, from the core's thread; false when the core isn't
/// running, and the notification then says to open the app.
pub fn answer(tap: Tap) -> bool {
    let Some(phone) = crate::phone::handle() else { return false };
    phone.spawn(move || async move {
        use inyon_core::hosts::registry::registry;
        let Some(n) = NOTIFIER.with(|n| n.borrow().clone()) else { return };
        let (now, at) = (wall_ms(), tap.at);
        n.answer(tap, now, &registry().overview(), |host, pane, option| async move {
            let body = crate::api::Answer { option, text: None, question: None };
            let send = crate::api::answer(&host, &pane, &body);
            // past Kotlin's 15 s it would say "not sent" while the answer still arrived
            match tokio::time::timeout(std::time::Duration::from_millis(send_window_ms(at, wall_ms())), send).await {
                Ok(r) => r.map_err(|e| e.message),
                Err(_) => Err("It took too long to reach the agent, so it may not have arrived. Open the session to check.".into()),
            }
        })
        .await;
    });
    true
}

/// Starts posting notifications from the core's thread into `shade` (made there).
pub fn start(phone: &inyon_phone::native::PhoneHandle, shade: impl FnOnce() -> Rc<dyn Shade> + Send + 'static) {
    use inyon_core::hosts::registry::{registry, RegistryEvent};
    use inyon_core::platform::rt::set_timeout;
    phone.spawn(move || async move {
        let n = Notifier::new(shade());
        NOTIFIER.with(|cell| *cell.borrow_mut() = Some(n.clone()));
        let heard = n.clone();
        inyon_api::clients::on_notice(move |msg| {
            let wants = Wants::from_settings(crate::platform::load("inyon.settings.v2").as_deref());
            if let Some(later) = heard.heard(msg, crate::phone::is_visible(), wants) {
                let n = heard.clone();
                set_timeout(ANSWERS_WAIT_MS, move || n.ask(later, &registry().overview()));
            }
        });
        let _ = registry().on(move |e| {
            if matches!(e, RegistryEvent::Overview) {
                n.overview(&registry().overview());
            }
        });
    });
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
