//! The one WebSocket to the hub (`/ws2`, protocol v2), run as apps/web/src/lib/socket.ts runs
//! `/ws`: `hello` on open (the app then follows its streams from where it left them), a ping every
//! 25 s, a socket that went quiet for 55 s is dead, reconnect with backoff, and come back at once
//! when the page is shown again or the network returns. After 15 s in the background it asks for
//! no frames but an overview catch-up when an agent's state changes; streams stay followed.

/// What the socket tells the app.
#[derive(Debug)]
#[cfg_attr(not(any(target_arch = "wasm32", feature = "phone")), allow(dead_code))]
pub enum Event {
    /// connected (`hello` is sent): follow the streams again
    Up,
    /// the connection closed, or the try failed; the app logs why and checks whether the session
    /// ended with it
    Down(Dropped),
    /// a message: text, or a compressed frame
    Message { text: Option<String>, binary: Option<Vec<u8>> },
}

/// How a connection ended: the close code and reason the browser reported, or that it went
/// quiet and was given up on.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub struct Dropped {
    pub code: Option<u16>,
    pub reason: String,
    pub timeout: bool,
}

/// Where the app's messages to the hub go. While the socket is down they are dropped: what
/// matters is sent again on the next `Up` (the streams), or by the person (a draft, keys).
#[derive(Clone)]
pub struct Outbox(
    #[cfg(target_arch = "wasm32")] futures_channel::mpsc::UnboundedSender<imp::Raw>,
    // the phone core in the app (crate::phone) takes the messages as they are
    #[cfg(all(not(target_arch = "wasm32"), feature = "phone"))] futures_channel::mpsc::UnboundedSender<String>,
);

impl Outbox {
    #[cfg_attr(not(any(target_arch = "wasm32", feature = "phone")), allow(unused_variables))]
    pub fn send(&self, json: String) {
        #[cfg(target_arch = "wasm32")]
        let _ = self.0.unbounded_send(imp::Raw::Send(json));
        #[cfg(all(not(target_arch = "wasm32"), feature = "phone"))]
        let _ = self.0.unbounded_send(json);
    }
}

/// The outbox, and the socket's end of it for [`run`].
pub fn channel() -> (Outbox, Inbox) {
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, rx) = futures_channel::mpsc::unbounded();
        (Outbox(tx.clone()), Inbox { tx, rx })
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "phone"))]
    {
        let (tx, rx) = futures_channel::mpsc::unbounded();
        (Outbox(tx), Inbox(rx))
    }
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "phone")))]
    (Outbox(), Inbox)
}

#[cfg(target_arch = "wasm32")]
pub use imp::{run, Inbox};

#[cfg(all(not(target_arch = "wasm32"), feature = "phone"))]
pub struct Inbox(futures_channel::mpsc::UnboundedReceiver<String>);

/// The socket to the phone core in the app, in place of the hub's `/ws2`.
#[cfg(all(not(target_arch = "wasm32"), feature = "phone"))]
pub async fn run(inbox: Inbox, on: impl FnMut(Event)) {
    crate::phone::run(inbox.0, on).await
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "phone")))]
pub struct Inbox;

#[cfg(all(not(target_arch = "wasm32"), not(feature = "phone")))]
pub async fn run(_inbox: Inbox, _on: impl FnMut(Event)) {
    // only the browser and phone builds have something to reach so far
    std::future::pending::<()>().await
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::Event;
    use crate::platform::{now, random};
    use crate::state::{attempt_after, backoff, hello, wire};
    use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
    use futures_util::future::{select, Either};
    use futures_util::StreamExt;
    use gloo_timers::future::TimeoutFuture;
    use inyon_protocol::{compat::CATALOG, ClientMsg};
    use wasm_bindgen::{closure::Closure, JsCast};

    const PING_MS: f64 = 25_000.0;
    const DEAD_MS: f64 = 55_000.0;
    const QUIET_AFTER_MS: f64 = 15_000.0;

    pub enum Raw {
        Open(u32),
        Message(u32, Option<String>, Option<Vec<u8>>),
        Close(u32, Option<u16>, String),
        /// the page was shown (true) or hidden
        Visible(bool),
        Online,
        /// a message from the app for the hub
        Send(String),
    }

    /// The socket's end of the outbox, and a way to post its own events into the same queue.
    pub struct Inbox {
        pub(super) tx: UnboundedSender<Raw>,
        pub(super) rx: UnboundedReceiver<Raw>,
    }

    /// The open socket and its handlers; dropping it closes the socket without hearing back.
    struct Conn {
        ws: web_sys::WebSocket,
        _handlers: Vec<Closure<dyn FnMut(web_sys::Event)>>,
    }

    impl Drop for Conn {
        fn drop(&mut self) {
            self.ws.set_onopen(None);
            self.ws.set_onmessage(None);
            self.ws.set_onclose(None);
            let _ = self.ws.close();
        }
    }

    fn url() -> String {
        let loc = web_sys::window().map(|w| w.location());
        let https = loc.as_ref().and_then(|l| l.protocol().ok()).is_some_and(|p| p == "https:");
        let host = loc.and_then(|l| l.host().ok()).unwrap_or_default();
        format!("{}://{host}/ws2?catalog={CATALOG}", if https { "wss" } else { "ws" })
    }

    fn open(id: u32, tx: &UnboundedSender<Raw>) -> Option<Conn> {
        let ws = web_sys::WebSocket::new(&url()).ok()?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let on = |f: Box<dyn FnMut(web_sys::Event)>| Closure::wrap(f);
        let t = tx.clone();
        let opened = on(Box::new(move |_| drop(t.unbounded_send(Raw::Open(id)))));
        let t = tx.clone();
        let message = on(Box::new(move |e: web_sys::Event| {
            let Some(data) = e.dyn_ref::<web_sys::MessageEvent>().map(|m| m.data()) else { return };
            if let Some(text) = data.as_string() {
                let _ = t.unbounded_send(Raw::Message(id, Some(text), None));
            } else if let Some(buf) = data.dyn_ref::<js_sys::ArrayBuffer>() {
                let _ = t.unbounded_send(Raw::Message(id, None, Some(js_sys::Uint8Array::new(buf).to_vec())));
            }
        }));
        let t = tx.clone();
        let closed = on(Box::new(move |e: web_sys::Event| {
            let c = e.dyn_ref::<web_sys::CloseEvent>();
            let _ = t.unbounded_send(Raw::Close(id, c.map(|c| c.code()), c.map(|c| c.reason()).unwrap_or_default()));
        }));
        ws.set_onopen(Some(opened.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(closed.as_ref().unchecked_ref()));
        Some(Conn { ws, _handlers: vec![opened, message, closed] })
    }

    fn visible() -> bool {
        web_sys::window().and_then(|w| w.document()).is_none_or(|d| d.visibility_state() != web_sys::VisibilityState::Hidden)
    }

    /// Page visibility and the network coming back, while the socket runs; dropping it takes the
    /// listeners off again, since a closure the page still calls must not be freed.
    struct Listeners {
        vis: Closure<dyn FnMut(web_sys::Event)>,
        online: Closure<dyn FnMut(web_sys::Event)>,
    }

    impl Listeners {
        fn new(tx: &UnboundedSender<Raw>) -> Self {
            let t = tx.clone();
            let vis = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| drop(t.unbounded_send(Raw::Visible(visible()))));
            let t = tx.clone();
            let online = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| drop(t.unbounded_send(Raw::Online)));
            if let Some(win) = web_sys::window() {
                if let Some(doc) = win.document() {
                    let _ = doc.add_event_listener_with_callback("visibilitychange", vis.as_ref().unchecked_ref());
                }
                let _ = win.add_event_listener_with_callback("online", online.as_ref().unchecked_ref());
            }
            Listeners { vis, online }
        }
    }

    impl Drop for Listeners {
        fn drop(&mut self) {
            if let Some(win) = web_sys::window() {
                if let Some(doc) = win.document() {
                    let _ = doc.remove_event_listener_with_callback("visibilitychange", self.vis.as_ref().unchecked_ref());
                }
                let _ = win.remove_event_listener_with_callback("online", self.online.as_ref().unchecked_ref());
            }
        }
    }

    fn send(conn: &Conn, msg: &ClientMsg) {
        send_raw(conn, &wire(msg));
    }

    fn send_raw(conn: &Conn, text: &str) {
        if conn.ws.ready_state() == web_sys::WebSocket::OPEN {
            let _ = conn.ws.send_with_str(text);
        }
    }

    /// The next event, or None once `ms` passed without one.
    async fn next_within(rx: &mut UnboundedReceiver<Raw>, ms: f64) -> Option<Raw> {
        let wait = TimeoutFuture::new(ms.clamp(0.0, 2_147_483_000.0) as u32);
        match select(rx.next(), wait).await {
            Either::Left((ev, _)) => ev,
            Either::Right(_) => None,
        }
    }

    /// Runs until the task is dropped (the app signs out or closes).
    pub async fn run(inbox: Inbox, mut on: impl FnMut(Event)) {
        let Inbox { tx, mut rx } = inbox;
        let _listeners = Listeners::new(&tx);
        let mut id = 0u32;
        let mut retry = 0u32;
        let mut quiet = false;
        let mut hidden_since = if visible() { None } else { Some(now()) };
        loop {
            id += 1;
            let conn = open(id, &tx);
            let mut opened_at: Option<f64> = None;
            let mut heard = now();
            let mut next_ping = now() + PING_MS;
            let mut ended = super::Dropped::default();
            // one connection, until it closes or goes quiet
            while let Some(conn) = &conn {
                let quiet_at = hidden_since.filter(|_| !quiet).map(|t| t + QUIET_AFTER_MS);
                let deadline = quiet_at.map_or(next_ping, |q| q.min(next_ping));
                match next_within(&mut rx, deadline - now()).await {
                    Some(Raw::Open(i)) if i == id => {
                        opened_at = Some(now());
                        heard = now();
                        next_ping = now() + PING_MS;
                        send_raw(conn, &hello());
                        on(Event::Up);
                        if quiet {
                            send(conn, &ClientMsg::Quiet { on: true });
                        }
                    }
                    Some(Raw::Message(i, text, binary)) if i == id => {
                        heard = now();
                        on(Event::Message { text, binary });
                    }
                    Some(Raw::Send(text)) => {
                        if opened_at.is_some() {
                            send_raw(conn, &text);
                        }
                    }
                    Some(Raw::Close(i, code, reason)) if i == id => {
                        ended = super::Dropped { code, reason, timeout: false };
                        break;
                    }
                    Some(Raw::Visible(true)) | Some(Raw::Online) => {
                        hidden_since = None;
                        if quiet {
                            quiet = false;
                            send(conn, &ClientMsg::Quiet { on: false });
                        }
                        // a phone drops sockets in the background: check on this one now
                        if opened_at.is_some() && now() - heard > 30_000.0 {
                            ended.timeout = true;
                            break;
                        }
                        send(conn, &ClientMsg::Ping);
                    }
                    Some(Raw::Visible(false)) => hidden_since = Some(now()),
                    // an older connection's last words
                    Some(_) => {}
                    None => {
                        if now() - heard > DEAD_MS {
                            ended.timeout = true;
                            break;
                        }
                        if now() >= next_ping {
                            send(conn, &ClientMsg::Ping);
                            next_ping = now() + PING_MS;
                        }
                        if quiet_at.is_some_and(|q| now() >= q) {
                            quiet = true;
                            send(conn, &ClientMsg::Quiet { on: true });
                        }
                    }
                }
            }
            drop(conn);
            on(Event::Down(ended));
            retry = attempt_after(retry, opened_at, now());
            let wait = backoff(retry, 400.0, 8000.0, random());
            retry += 1;
            // the wait ends early when the page comes back or the network returns
            let until = now() + wait;
            while now() < until {
                match next_within(&mut rx, until - now()).await {
                    Some(Raw::Visible(true)) | Some(Raw::Online) => {
                        hidden_since = None;
                        retry = 0;
                        break;
                    }
                    Some(Raw::Visible(false)) => hidden_since = Some(now()),
                    _ => {}
                }
            }
        }
    }
}
