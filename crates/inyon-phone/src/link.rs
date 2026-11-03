//! Port of the TypeScript phone core's link.ts (removed in 0.23): the phone's side of `inyon link`. One end-to-end encrypted
//! channel per paired laptop, carrying the same commands a hub sends to inyon. It tries the
//! addresses the laptop gave last (its LAN address first, then its tunnel), looks up its current
//! ones by its id when those fail (a tunnel's address changes every time it restarts), then its
//! relays. A lost connection comes back on its own, and at once when the app returns to the
//! foreground.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use futures_util::future::{join_all, LocalBoxFuture};
use inyon_core::clock::now_ms;
use inyon_core::hosts::node_link::{self, NodeSocket, Socket};
use inyon_core::platform::fetch::{fetch, FetchRequest};
use inyon_core::platform::rt::{set_timeout, spawn, Deferred, Timer};
use inyon_core::platform::store::kv;
use inyon_link::pkarr::{newest, relay_url, Record};
use inyon_link::{is_machine_id, noise_key_of, phone_path, relay_ws, short_code_identity, short_secret, Channel, KeyPair, LinkError, PairInfo, ShortCode, PKARR_RELAYS};
use inyon_protocol::HostConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const DIRECT_MS: u64 = 4000;
const PING_MS: u64 = 20_000;
const PONG_MS: u64 = 8000;
/// how long a removed computer is told to forget the phone before the phone stops trying
pub const UNPAIR_GIVE_UP_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// how long a connection has to stay up before its backoff starts from the first step again
const STABLE_MS: u64 = 30_000;

/// `backoff`: a random wait up to min(cap, base·2^attempt), so phones don't all retry at once.
pub fn backoff(attempt: u32, base: u64, cap: u64) -> u64 {
    let r = u64::from_le_bytes(inyon_api::http::random_bytes(8).try_into().unwrap_or([0; 8])) as f64 / (u64::MAX as f64 + 1.0);
    (r * cap.min(base.saturating_mul(1 << attempt.min(30))) as f64) as u64
}

fn attempt_after(attempt: u32, opened_at: Option<u64>) -> u32 {
    if opened_at.is_some_and(|o| now_ms().saturating_sub(o) >= STABLE_MS) {
        0
    } else {
        attempt
    }
}

// ---------- the wire ----------

pub enum WireEvent {
    Message(Vec<u8>),
    Close(u16, String),
}

/// A WebSocket: whole binary messages, in order.
pub trait Wire {
    fn send(&self, data: &[u8]);
    fn close(&self, code: u16, reason: &str);
}

pub type OnWire = Rc<dyn Fn(WireEvent)>;

/// Opens WebSockets (the browser's, or a test's). What's sent before one opens waits for it.
pub trait Dialer {
    fn dial(&self, url: &str, on: OnWire) -> Result<Rc<dyn Wire>, String>;
}

type OnMessage = Rc<dyn Fn(Value)>;
type OnClose = Rc<dyn Fn(u16, String)>;

/// An encrypted JSON channel to a machine, over one wire.
pub struct LinkChannel {
    wire: Rc<dyn Wire>,
    channel: RefCell<Channel>,
    closed: Cell<bool>,
    on_message: RefCell<Option<OnMessage>>,
    on_close: RefCell<Option<OnClose>>,
}

impl LinkChannel {
    pub fn send(&self, msg: &Value) {
        if self.closed.get() {
            return;
        }
        let Ok(frames) = self.channel.borrow_mut().seal(inyon_core::js::stringify(msg).as_bytes()) else { return };
        for f in frames {
            self.wire.send(&f);
        }
    }

    pub fn close(&self, code: u16, reason: &str) {
        if self.closed.replace(true) {
            return;
        }
        self.wire.close(code, reason);
        let f = self.on_close.borrow().clone();
        if let Some(f) = f {
            f(code, reason.to_owned());
        }
    }

    pub fn on_message(&self, f: impl Fn(Value) + 'static) {
        *self.on_message.borrow_mut() = Some(Rc::new(f));
    }

    pub fn on_close(&self, f: Option<OnClose>) {
        *self.on_close.borrow_mut() = f;
    }

    fn frame(&self, data: &[u8]) {
        if self.closed.get() {
            return;
        }
        let opened = self.channel.borrow_mut().open(data);
        match opened {
            // a forged or out-of-order message: the session can't be trusted any more
            Err(e) => self.close(e.close_code(), e.reason()),
            Ok(None) => {}
            Ok(Some(body)) => {
                let (Ok(msg), Some(f)) = (serde_json::from_slice::<Value>(&body), self.on_message.borrow().clone()) else { return };
                f(msg);
            }
        }
    }

    fn ended(&self, code: u16, reason: String) {
        if self.closed.replace(true) {
            return;
        }
        let f = self.on_close.borrow().clone();
        if let Some(f) = f {
            f(code, reason);
        }
    }
}

/// Where a wire's events go: the handshake's answer first, then the channel.
enum Route {
    Handshake(Deferred<Result<Vec<u8>, LinkError>>, Vec<Vec<u8>>),
    Open(Weak<LinkChannel>),
    Done,
}

/// One address that answered: its channel, the machine's welcome, and the addresses tried.
#[derive(Clone)]
pub struct Reached {
    pub channel: Rc<LinkChannel>,
    pub welcome: Value,
    pub url: String,
    pub urls: Vec<String>,
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| u.port().map_or_else(|| h.to_owned(), |p| format!("{h}:{p}")))).unwrap_or_else(|| url.to_owned())
}

/// What only the real computer can answer: a refusal needs a message 2 that decrypts. BAD_KEY says
/// only that whatever answered at that address wasn't the computer (another device on a café's
/// network holding the LAN address the computer had at home, say), so the next address is tried.
fn is_final(e: &LinkError) -> bool {
    matches!(e, LinkError::NotPaired(_) | LinkError::Refused(_))
}

pub type Resolve = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Option<Record>>>;

/// What a machine (or a short pairing code) last published, from the pkarr relays.
pub fn pkarr_resolve() -> Resolve {
    Rc::new(|id| {
        Box::pin(async move {
            let got = join_all(PKARR_RELAYS.iter().map(|r| fetch(FetchRequest::get(relay_url(r, &id), 10_000)))).await;
            let bodies: Vec<Vec<u8>> = got.into_iter().filter_map(Result::ok).filter(|r| r.ok()).map(|r| r.body).collect();
            newest(&id, bodies.iter().map(Vec::as_slice)).map(|p| p.record)
        })
    })
}

/// How the phone reaches computers: its sockets, the pkarr lookup, and whether this page may open
/// plain ws:// addresses.
#[derive(Clone)]
pub struct Reach {
    pub dialer: Rc<dyn Dialer>,
    pub resolve: Resolve,
    /// A web page served over https can't open plain ws:// (the LAN and private network
    /// addresses), and newer Chrome asks about "devices on your local network" before it refuses,
    /// so a browser skips them. The Android app allows them.
    pub plain_ws: bool,
}

impl Reach {
    /// What a short pairing code stands for: the computer that published under it.
    pub async fn short_code(&self, c: &ShortCode) -> Option<PairInfo> {
        let rec = (self.resolve)(short_code_identity(c).id).await?;
        let machine = rec.machine.filter(|m| is_machine_id(m))?;
        Some(PairInfo { id: machine, secret: short_secret(c), name: rec.name.filter(|n| !n.is_empty()).unwrap_or_else(|| "Computer".into()), urls: rec.urls, relays: rec.relays })
    }

    fn usable(&self, urls: &[String]) -> Vec<String> {
        urls.iter().filter(|u| self.plain_ws || !u.starts_with("ws://")).cloned().collect()
    }

    /// One address: the handshake, within `ms`.
    async fn via(&self, url: &str, ms: u64, noise: &KeyPair, machine_key: &[u8; 32], hello: &[u8]) -> Result<Reached, LinkError> {
        let first = Deferred::new();
        let route = Rc::new(RefCell::new(Route::Handshake(first.clone(), Vec::new())));
        let r = route.clone();
        let on: OnWire = Rc::new(move |e| {
            let open = match &mut *r.borrow_mut() {
                Route::Handshake(d, early) => {
                    match e {
                        // the first message is the answer; anything after it waits for the channel
                        WireEvent::Message(m) if !d.is_settled() => d.resolve(Ok(m)),
                        WireEvent::Message(m) => early.push(m),
                        WireEvent::Close(code, reason) => d.resolve(Err(LinkError::Closed(if reason.is_empty() { format!("Connection closed ({code})") } else { reason }))),
                    }
                    return;
                }
                Route::Open(c) => c.upgrade(),
                Route::Done => None,
            };
            match (open, e) {
                (Some(c), WireEvent::Message(m)) => c.frame(&m),
                (Some(c), WireEvent::Close(code, reason)) => c.ended(code, reason),
                _ => {}
            }
        });
        let wire = self.dialer.dial(url, on).map_err(LinkError::Closed)?;
        let timer = {
            let (first, wire, host) = (first.clone(), wire.clone(), host_of(url));
            set_timeout(ms, move || {
                wire.close(1000, "");
                first.resolve(Err(LinkError::Timeout(format!("{host} didn't answer"))));
            })
        };
        let (connecting, msg1) = inyon_link::connect(noise, machine_key, hello).map_err(|e| LinkError::Closed(e.to_string()))?;
        wire.send(&msg1);
        let reply = first.wait().await;
        timer.clear();
        let done = |wire: &Rc<dyn Wire>, code: u16, reason: &str| {
            *route.borrow_mut() = Route::Done;
            wire.close(code, reason);
        };
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                *route.borrow_mut() = Route::Done;
                return Err(e);
            }
        };
        match connecting.finish(&reply) {
            Ok((channel, welcome)) => {
                let ch = Rc::new(LinkChannel { wire, channel: RefCell::new(channel), closed: Cell::new(false), on_message: RefCell::default(), on_close: RefCell::default() });
                let early = match std::mem::replace(&mut *route.borrow_mut(), Route::Open(Rc::downgrade(&ch))) {
                    Route::Handshake(_, early) => early,
                    _ => vec![],
                };
                for m in early {
                    ch.frame(&m);
                }
                Ok(Reached { channel: ch, welcome, url: url.to_owned(), urls: vec![] })
            }
            Err(e) => {
                match e {
                    LinkError::BadKey(_) => done(&wire, 4401, "bad handshake"),
                    _ => done(&wire, 4403, "refused"),
                }
                Err(e)
            }
        }
    }

    /// The first address to answer wins; the others are closed, even ones that answer later.
    async fn first_of(&self, urls: Vec<String>, ms: u64, noise: &KeyPair, machine_key: [u8; 32], hello: Rc<[u8]>, others: Rc<RefCell<Vec<String>>>) -> Result<Reached, LinkError> {
        if urls.is_empty() {
            return Err(LinkError::Closed("No address to try".into()));
        }
        let out: Deferred<Result<Reached, LinkError>> = Deferred::new();
        let state = Rc::new(RefCell::new((false, Vec::<LinkError>::new())));
        let n = urls.len();
        for u in urls {
            let (me, out, state, noise, hello, others) = (self.clone(), out.clone(), state.clone(), noise.clone(), hello.clone(), others.clone());
            spawn(async move {
                let got = me.via(&u, ms, &noise, &machine_key, &hello).await;
                let mut s = state.borrow_mut();
                match got {
                    Ok(r) if s.0 => r.channel.close(1000, "another address answered first"),
                    Ok(r) => {
                        s.0 = true;
                        drop(s);
                        out.resolve(Ok(r));
                    }
                    Err(e) => {
                        // only where it happened: another address of the batch may just have timed out
                        if matches!(e, LinkError::BadKey(_)) && !others.borrow().contains(&host_of(&u)) {
                            others.borrow_mut().push(host_of(&u));
                        }
                        s.1.push(e);
                        if s.0 || s.1.len() < n {
                            return;
                        }
                        // a refusal from the computer says more than a timeout, and more than something else answering
                        let errors = std::mem::take(&mut s.1);
                        drop(s);
                        let pick = errors.iter().find(|e| is_final(e)).or_else(|| errors.iter().find(|e| matches!(e, LinkError::BadKey(_)))).unwrap_or(&errors[0]).clone();
                        out.resolve(Err(pick));
                    }
                }
            });
        }
        out.wait().await
    }

    /// Opens a channel through the first address that answers; LAN and tunnel race, relays come last.
    pub async fn reach(&self, noise: &KeyPair, id: &str, urls: &[String], relays: &[String], hello: &Value) -> Result<Reached, LinkError> {
        let machine_key = noise_key_of(id).ok_or_else(|| LinkError::BadKey("That isn't a computer's id".into()))?;
        let hello: Rc<[u8]> = inyon_core::js::stringify(hello).into_bytes().into();
        // addresses where something other than the computer answered, said as a detail at the end
        let others = Rc::new(RefCell::new(Vec::new()));
        let fail = |e: LinkError| if is_final(&e) { Err(e) } else { Ok(()) };
        match self.first_of(self.usable(urls), DIRECT_MS, noise, machine_key, hello.clone(), others.clone()).await {
            Ok(r) => return Ok(Reached { urls: urls.to_vec(), ..r }),
            Err(e) => fail(e)?,
        }
        let found = (self.resolve)(id.to_owned()).await;
        let found_urls = found.as_ref().map(|f| f.urls.clone());
        let fresh: Vec<String> = self.usable(&found_urls.clone().unwrap_or_default()).into_iter().filter(|u| !urls.contains(u)).collect();
        let mut all_relays: Vec<String> = Vec::new();
        for r in found.iter().flat_map(|f| f.relays.iter()).chain(relays) {
            if !all_relays.contains(r) {
                all_relays.push(r.clone());
            }
        }
        if !fresh.is_empty() {
            match self.first_of(fresh, DIRECT_MS * 2, noise, machine_key, hello.clone(), others.clone()).await {
                Ok(r) => return Ok(Reached { urls: found_urls.clone().unwrap_or_default(), ..r }),
                Err(e) => fail(e)?,
            }
        }
        if !all_relays.is_empty() {
            let via_relays = all_relays.iter().map(|r| relay_ws(r, &phone_path(id))).collect();
            match self.first_of(via_relays, DIRECT_MS * 2, noise, machine_key, hello, others.clone()).await {
                Ok(r) => return Ok(Reached { urls: found_urls.clone().unwrap_or_else(|| urls.to_vec()), ..r }),
                Err(e) => fail(e)?,
            }
        }
        let others = others.borrow();
        let elsewhere = if others.is_empty() { String::new() } else { format!(" (something else answered at {})", others.join(", ")) };
        Err(LinkError::Timeout(format!(
            "{}{elsewhere}",
            if found.is_some() {
                "The computer isn't answering at its addresses. Is `inyon link` running there?"
            } else {
                "Couldn't find the computer online. Is `inyon link` running there? It needs to be on and connected to the internet."
            }
        )))
    }
}

/// Where to try next time: the laptop's own list, with the direct address that just worked first
/// (a tunnel's address changes, so only a direct one is worth keeping when the laptop didn't list it).
pub fn addresses(worked: &str, listed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if worked.starts_with("ws://") || listed.iter().any(|l| l == worked) {
        out.push(worked.to_owned());
    }
    for l in listed {
        if !out.contains(l) {
            out.push(l.clone());
        }
    }
    out
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// A computer removed on the phone that hasn't heard yet that it should stop trusting the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUnpair {
    pub machine_id: String,
    pub name: String,
    pub urls: Vec<String>,
    pub relays: Vec<String>,
    pub at: u64,
}

// kept in the store, so they outlive the app being closed
const PENDING: &str = "pending-unpairs";

fn pending_list() -> Vec<PendingUnpair> {
    kv::get(PENDING).unwrap_or_default()
}

fn save_pending(list: &[PendingUnpair]) {
    if list.is_empty() {
        kv::del(PENDING);
    } else {
        let _ = kv::set(PENDING, list);
    }
}

fn is_pending(machine_id: &str) -> bool {
    pending_list().iter().any(|p| p.machine_id == machine_id)
}

#[derive(Default)]
struct Conn {
    config: Option<HostConfig>,
    channel: Option<Rc<LinkChannel>>,
    connecting: bool,
    attempt: u32,
    /// when the open channel came up, so a backoff only starts over after it stayed up a while
    opened_at: Option<u64>,
    timer: Option<Timer>,
    ping: Option<Timer>,
    awaiting_pong: Option<Timer>,
    stopped: bool,
}

type ConnRef = Rc<RefCell<Conn>>;

#[derive(Default)]
struct UnpairTry {
    attempt: u32,
    sent: u32,
    timer: Option<Timer>,
    running: bool,
}

pub type Noise = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<KeyPair, String>>>;
pub type SaveAddresses = Rc<dyn Fn(&str, Vec<String>, Vec<String>)>;

struct Inner {
    reach: Reach,
    noise: Noise,
    name: String,
    save_addresses: SaveAddresses,
    conns: RefCell<HashMap<String, ConnRef>>,
    unpair_tries: RefCell<HashMap<String, Rc<RefCell<UnpairTry>>>>,
}

/// The node_link socket for one channel: what the core sends a machine, encrypted.
struct ChannelSocket(Weak<LinkChannel>);

impl NodeSocket for ChannelSocket {
    fn send(&self, data: &str) {
        if let (Some(c), Ok(msg)) = (self.0.upgrade(), serde_json::from_str::<Value>(data)) {
            c.send(&msg);
        }
    }
    fn close(&self, code: u16, reason: &str) {
        if let Some(c) = self.0.upgrade() {
            c.close(code, reason);
        }
    }
}

/// Keeps a channel open to each paired computer.
#[derive(Clone)]
pub struct Links(Rc<Inner>);

impl Links {
    pub fn new(reach: Reach, noise: Noise, name: String, save_addresses: SaveAddresses) -> Self {
        let links = Links(Rc::new(Inner { reach, noise, name, save_addresses, conns: RefCell::default(), unpair_tries: RefCell::default() }));
        // ones left from before the app was closed
        let me = links.clone();
        set_timeout(0, move || {
            for p in pending_list() {
                me.tell(p);
            }
        });
        links
    }

    fn hello(&self) -> Value {
        json!({ "name": self.0.name })
    }

    // ---------- removed computers ----------

    // Removing a computer tells it to forget the phone, but only a computer that can be reached
    // hears it. The rest are kept in the store and told the next time they answer, until one says
    // the phone isn't paired there (which is how a told computer answers).

    fn done(&self, machine_id: &str) {
        save_pending(&pending_list().into_iter().filter(|p| p.machine_id != machine_id).collect::<Vec<_>>());
        if let Some(t) = self.0.unpair_tries.borrow_mut().remove(machine_id) {
            if let Some(timer) = t.borrow_mut().timer.take() {
                timer.clear();
            }
        }
    }

    fn tell(&self, p: PendingUnpair) {
        // a computer before 0.17 lets the phone in and ignores `unpair`, so it never says the phone
        // isn't paired: give up on one after a month
        if now_ms().saturating_sub(p.at) > UNPAIR_GIVE_UP_MS {
            return self.done(&p.machine_id);
        }
        let t = self.0.unpair_tries.borrow_mut().entry(p.machine_id.clone()).or_default().clone();
        {
            let mut t = t.borrow_mut();
            if t.running {
                return;
            }
            t.running = true;
            if let Some(timer) = t.timer.take() {
                timer.clear();
            }
        }
        let me = self.clone();
        spawn(async move {
            let mut again = {
                let mut t = t.borrow_mut();
                t.attempt += 1;
                backoff(t.attempt - 1, 5000, 10 * 60_000)
            };
            let got = match (me.0.noise)().await {
                Ok(noise) => me.0.reach.reach(&noise, &p.machine_id, &p.urls, &p.relays, &me.hello()).await,
                Err(e) => Err(LinkError::Closed(e)),
            };
            match got {
                Ok(r) => {
                    // paired with it again meanwhile: that pairing stands
                    if !is_pending(&p.machine_id) {
                        t.borrow_mut().running = false;
                        return r.channel.close(1000, "paired again");
                    }
                    r.channel.send(&json!({ "op": "unpair" }));
                    // it forgets the phone in a moment; the first check comes soon after, later ones back off
                    let ch = r.channel.clone();
                    set_timeout(1000, move || ch.close(1000, "unpaired"));
                    let mut t = t.borrow_mut();
                    t.sent += 1;
                    if t.sent == 1 {
                        again = 3000;
                    }
                }
                Err(LinkError::NotPaired(_)) => {
                    t.borrow_mut().running = false;
                    return me.done(&p.machine_id);
                }
                Err(_) => {}
            }
            t.borrow_mut().running = false;
            if is_pending(&p.machine_id) {
                let (me2, p2) = (me.clone(), p.clone());
                t.borrow_mut().timer = Some(set_timeout(again, move || me2.tell(p2)));
            }
        });
    }

    /// The computers removed here that still trust the phone, as far as it knows.
    pub fn pending_unpairs(&self) -> Vec<PendingUnpair> {
        pending_list()
    }

    // ---------- connections ----------

    fn drop_channel(c: &ConnRef, why: &str) {
        let ch = {
            let mut c = c.borrow_mut();
            for t in [c.ping.take(), c.awaiting_pong.take()].into_iter().flatten() {
                t.clear();
            }
            c.channel.take()
        };
        if let Some(ch) = ch {
            ch.on_close(None);
            ch.close(1000, why);
        }
    }

    fn schedule(&self, c: &ConnRef, now: bool) {
        let mut cc = c.borrow_mut();
        if let Some(t) = cc.timer.take() {
            t.clear();
        }
        if cc.stopped {
            return;
        }
        let delay = if now {
            0
        } else {
            cc.attempt += 1;
            backoff(cc.attempt - 1, 1000, 30_000)
        };
        let (me, c2) = (self.clone(), c.clone());
        cc.timer = Some(set_timeout(delay, move || me.open(&c2)));
    }

    fn open(&self, c: &ConnRef) {
        {
            let mut cc = c.borrow_mut();
            if cc.connecting {
                return;
            }
            cc.connecting = true;
        }
        let (me, c) = (self.clone(), c.clone());
        spawn(async move {
            me.try_open(&c).await;
            c.borrow_mut().connecting = false;
        });
    }

    async fn try_open(&self, c: &ConnRef) {
        let Some(config) = c.borrow().config.clone() else { return };
        let id = config.machine_id.clone().unwrap_or_default();
        let got = match (self.0.noise)().await {
            Ok(noise) => self.0.reach.reach(&noise, &id, config.urls.as_deref().unwrap_or_default(), config.relays.as_deref().unwrap_or_default(), &self.hello()).await,
            Err(e) => Err(LinkError::Closed(e)),
        };
        let r = match got {
            Ok(r) => r,
            Err(err) => {
                // removed while this attempt was out: its failure belongs to a computer that's gone
                if c.borrow().stopped {
                    return;
                }
                // not paired any more: retrying won't help until it pairs again (coming back to the
                // app still tries once more, in case it was paired again meanwhile). Anything else,
                // an address that something else answered included, is tried again.
                if let LinkError::NotPaired(m) = &err {
                    node_link::fail(&config.id, m);
                } else {
                    node_link::fail(&config.id, &format!("Connecting to {}… ({})", config.name, err.message()));
                    self.schedule(c, false);
                }
                return;
            }
        };
        if c.borrow().stopped {
            return r.channel.close(1000, "");
        }
        let welcome = Some(&r.welcome);
        let listed = strings(welcome.and_then(|w| w.get("urls")));
        let urls = addresses(&r.url, if listed.is_empty() { &r.urls } else { &listed });
        let relays = strings(welcome.and_then(|w| w.get("relays")));
        if (&urls, &relays) != (&config.urls.clone().unwrap_or_default(), &config.relays.clone().unwrap_or_default()) {
            (self.0.save_addresses)(&config.id, urls, relays);
        }
        let host_id = config.id.clone();
        let socket: Socket = Rc::new(ChannelSocket(Rc::downgrade(&r.channel)));
        {
            let mut cc = c.borrow_mut();
            cc.opened_at = Some(now_ms());
            cc.channel = Some(r.channel.clone());
        }
        let (weak, h) = (Rc::downgrade(c), host_id.clone());
        r.channel.on_message(move |m| {
            if m.get("type").and_then(Value::as_str) == Some("pong") {
                // answered: the next check can send its own ping
                if let Some(t) = weak.upgrade().and_then(|c| c.borrow_mut().awaiting_pong.take()) {
                    t.clear();
                }
                return;
            }
            node_link::receive(&h, &m);
        });
        let (me, weak, h, s) = (self.clone(), Rc::downgrade(c), host_id.clone(), socket.clone());
        r.channel.on_close(Some(Rc::new(move |_, _| {
            let Some(c) = weak.upgrade() else { return };
            {
                let mut cc = c.borrow_mut();
                cc.attempt = attempt_after(cc.attempt, cc.opened_at);
                cc.opened_at = None;
                for t in [cc.ping.take(), cc.awaiting_pong.take()].into_iter().flatten() {
                    t.clear();
                }
                cc.channel = None;
            }
            node_link::detach(&h, &s);
            me.schedule(&c, false);
        })));
        self.arm_ping(c);
        node_link::attach(&host_id, socket);
        let mut hello = Map::new();
        hello.insert("type".into(), "hello".into());
        for k in ["version", "os"] {
            if let Some(v) = welcome.and_then(|w| w.get(k)).filter(|v| !v.is_null()) {
                hello.insert(k.into(), v.clone());
            }
        }
        node_link::receive(&host_id, &Value::Object(hello));
    }

    fn arm_ping(&self, c: &ConnRef) {
        let (me, weak) = (self.clone(), Rc::downgrade(c));
        let t = set_timeout(PING_MS, move || {
            let Some(c) = weak.upgrade() else { return };
            if c.borrow().channel.is_none() {
                return;
            }
            me.check(&c, PONG_MS);
            me.arm_ping(&c);
        });
        c.borrow_mut().ping = Some(t);
    }

    /// A ping the laptop must answer; a socket that died while the phone slept fails it fast.
    fn check(&self, c: &ConnRef, ms: u64) {
        let ch = {
            let cc = c.borrow();
            match (&cc.channel, &cc.awaiting_pong) {
                (Some(ch), None) => ch.clone(),
                _ => return,
            }
        };
        ch.send(&json!({ "op": "ping", "t": now_ms() }));
        let weak = Rc::downgrade(c);
        c.borrow_mut().awaiting_pong = Some(set_timeout(ms, move || {
            let Some(c) = weak.upgrade() else { return };
            let ch = {
                let mut cc = c.borrow_mut();
                cc.awaiting_pong = None;
                cc.channel.clone()
            };
            if let Some(ch) = ch {
                ch.close(4408, "no answer");
            }
        }));
    }

    /// Keeps this machine connected (called for every link machine in the list).
    pub fn ensure(&self, config: &HostConfig) {
        if let Some(c) = self.0.conns.borrow().get(&config.id) {
            c.borrow_mut().config = Some(config.clone());
            return;
        }
        let c: ConnRef = Rc::new(RefCell::new(Conn { config: Some(config.clone()), ..Default::default() }));
        self.0.conns.borrow_mut().insert(config.id.clone(), c.clone());
        self.open(&c);
    }

    /// Stops connecting to a removed computer and tells it to forget the phone. True when it was
    /// connected right then; otherwise it hears once it's back (see [`PendingUnpair`]).
    pub fn forget(&self, id: &str, config: Option<&HostConfig>) -> bool {
        let c = self.0.conns.borrow_mut().remove(id);
        let cfg = c.as_ref().and_then(|c| c.borrow().config.clone()).or_else(|| config.cloned());
        let connected = c.as_ref().is_some_and(|c| c.borrow().channel.is_some());
        if let Some(c) = &c {
            // the computer forgets the phone too, so its key stops working there (computers before
            // 0.17 ignore this)
            let ch = c.borrow().channel.clone();
            if let Some(ch) = ch {
                ch.send(&json!({ "op": "unpair" }));
            }
            {
                let mut cc = c.borrow_mut();
                cc.stopped = true;
                if let Some(t) = cc.timer.take() {
                    t.clear();
                }
            }
            Self::drop_channel(c, "removed");
        }
        // a computer added again under this id starts without the old one's error
        node_link::clear_error(id);
        if let Some(cfg) = cfg {
            if let Some(machine_id) = cfg.machine_id.clone() {
                let p = PendingUnpair {
                    machine_id: machine_id.clone(),
                    name: cfg.name.clone(),
                    urls: cfg.urls.clone().unwrap_or_default(),
                    relays: cfg.relays.clone().unwrap_or_default(),
                    at: now_ms(),
                };
                let mut list: Vec<PendingUnpair> = pending_list().into_iter().filter(|x| x.machine_id != machine_id).collect();
                list.push(p.clone());
                save_pending(&list);
                // even one that was connected is asked again, until it says the phone isn't paired there
                let me = self.clone();
                set_timeout(if connected { 1500 } else { 0 }, move || me.tell(p));
            }
        }
        connected
    }

    /// Back in the foreground: check every connection now instead of waiting out a backoff.
    pub fn resume(&self) {
        let conns: Vec<ConnRef> = self.0.conns.borrow().values().cloned().collect();
        for c in conns {
            if c.borrow().channel.is_some() {
                self.check(&c, 3000);
            } else {
                c.borrow_mut().attempt = 0;
                self.schedule(&c, true);
            }
        }
        // and tell removed computers again
        for p in pending_list() {
            if let Some(t) = self.0.unpair_tries.borrow().get(&p.machine_id) {
                t.borrow_mut().attempt = 0;
            }
            self.tell(p);
        }
    }

    /// First contact, with the code from the laptop's screen: the computer's welcome, with where to
    /// reach it next time.
    pub async fn pair(&self, info: &PairInfo) -> Result<Value, LinkError> {
        let noise = (self.0.noise)().await.map_err(LinkError::Closed)?;
        let r = self.0.reach.reach(&noise, &info.id, &info.urls, &info.relays, &json!({ "name": self.0.name, "pair": info.secret })).await?;
        r.channel.close(1000, "paired");
        // a computer removed earlier and paired again: it mustn't be told to forget this pairing
        if is_pending(&info.id) {
            self.done(&info.id);
        }
        let mut w = r.welcome.clone();
        let listed = strings(w.get("urls"));
        let urls = addresses(&r.url, if listed.is_empty() { &r.urls } else { &listed });
        if let Some(o) = w.as_object_mut() {
            o.insert("urls".into(), json!(urls));
        }
        Ok(w)
    }
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
