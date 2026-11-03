//! The hub's WebSockets: `/ws`, where the app gets live updates (core.ts's clients), `/ws2`, the
//! same with protocol v2 (streams that resume from an epoch and seq, large frames compressed; see
//! inyon-api's streams.rs), and `/node`, where paired machines keep their connection
//! (hosts/node-link). The upgrade is checked
//! and answered on the core thread like any request; the socket itself then runs as a task on the
//! same thread, handing each message to the core in the order it came, and sending what the core
//! queues for it.
//!
//! As Bun's server did: a client that falls 4 MB behind is closed rather than silently losing
//! messages (the app reconnects and gets a fresh copy of what it watches), and a connection that
//! says nothing for 120 seconds is closed, with pings in between to keep a quiet one open.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use axum::http::{header, HeaderValue, Method, Response, StatusCode};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use inyon_core::clock::now_ms;
use inyon_core::hosts::node_link::{self, NodeSocket};
use inyon_core::hosts::registry::{registry, token_expired};
use inyon_core::hosts::removed_nodes::was_removed;
use inyon_core::platform::rt::sleep;
use inyon_protocol::HostKind;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::auth::{hold_socket, is_authed, Closer};
use crate::clients::{add, client_message, close_client, new_client, new_client2, wire};
use crate::http::{same, sha256, text, Req, Resp};
use crate::phones::{has_bearer, hold_phone_socket, phone_for, redeem_ticket};
use crate::server::same_origin;
use inyon_api::streams::{client_message2, Wire};

/// how far behind a client may fall before it's closed
const BACKPRESSURE: usize = 4 * 1024 * 1024;
/// a connection that says nothing for this long is closed
const IDLE: Duration = Duration::from_secs(120);
/// Bun's largest message
pub(crate) const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// How long a socket waits for a peer before giving up on it.
#[derive(Clone, Copy)]
struct Limits {
    /// nothing received for this long ends the socket, whatever is still being written
    idle: Duration,
    /// each step of closing: finishing the write in progress, the close frame, the TCP close
    close_wait: Duration,
}

const LIMITS: Limits = Limits { idle: IDLE, close_wait: Duration::from_secs(5) };

/// The core's side of one socket: what it sends goes into the socket task's queue.
pub struct Socket {
    tx: mpsc::UnboundedSender<Wire>,
    /// a close goes on a channel of its own, so it never waits behind queued text
    close_tx: mpsc::UnboundedSender<(u16, String)>,
    queued: Rc<Cell<usize>>,
    closed: Rc<Cell<bool>>,
}

/// The socket task's side of a [`Socket`].
struct Ends {
    rx: mpsc::UnboundedReceiver<Wire>,
    close_rx: mpsc::UnboundedReceiver<(u16, String)>,
    queued: Rc<Cell<usize>>,
    closed: Rc<Cell<bool>>,
}

impl Socket {
    fn new() -> (Rc<Socket>, Ends) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = mpsc::unbounded_channel();
        let (queued, closed) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(false)));
        (Rc::new(Socket { tx, close_tx, queued: queued.clone(), closed: closed.clone() }), Ends { rx, close_rx, queued, closed })
    }

    /// Queues a message. As uWS does, the limit is checked against what is still waiting to be
    /// written before this one is added, so a single large message always goes out.
    pub fn send_text(&self, s: String) {
        self.send(Wire::Text(s));
    }

    /// Queues a text or binary message (binary: a `/ws2` frame, compressed).
    pub fn send(&self, w: Wire) {
        if self.closed.get() {
            return;
        }
        if self.queued.get() > BACKPRESSURE {
            return self.close_with(1009, "Too far behind");
        }
        let n = match &w {
            Wire::Text(s) => s.len(),
            Wire::Binary(b) => b.len(),
        };
        self.queued.set(self.queued.get() + n);
        let _ = self.tx.send(w);
    }

    /// Closes the socket: from now on nothing the peer sends is acted on, and queued text is
    /// dropped rather than sent first.
    pub fn close_with(&self, code: u16, reason: &str) {
        if self.closed.replace(true) {
            return;
        }
        let _ = self.close_tx.send((code, reason.to_owned()));
    }
}

impl NodeSocket for Socket {
    fn send(&self, data: &str) {
        self.send_text(data.to_owned());
    }
    fn close(&self, code: u16, reason: &str) {
        self.close_with(code, reason);
    }
}

/// The 101 answer to a well-formed WebSocket request, or None (Bun's `upgrade` returning false).
fn handshake(req: &Req) -> Option<Resp> {
    let has = |name: &str, token: &str| req.header(name).is_some_and(|v| v.split(',').any(|p| p.trim().eq_ignore_ascii_case(token)));
    if req.method != Method::GET || !has("upgrade", "websocket") || !has("connection", "upgrade") || req.header("sec-websocket-version").as_deref() != Some("13") {
        return None;
    }
    let key = req.header("sec-websocket-key").filter(|k| !k.is_empty())?;
    let mut r = Response::new(crate::http::Body::empty());
    *r.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    r.headers_mut().insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    r.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    r.headers_mut().insert(header::SEC_WEBSOCKET_ACCEPT, HeaderValue::from_str(&derive_accept_key(key.as_bytes())).ok()?);
    Some(r)
}

/// Runs one socket once the upgrade is through.
async fn pump(on: hyper::upgrade::OnUpgrade, ends: Ends, on_open: impl FnOnce(), on_text: impl Fn(&str), on_close: impl FnOnce(bool)) {
    match on.await {
        Ok(upgraded) => run(TokioIo::new(upgraded), ends, LIMITS, on_open, on_text, on_close).await,
        Err(_) => on_close(true),
    }
}

type Sink<IO> = SplitSink<WebSocketStream<IO>, Message>;
/// a write in progress: it hands the sink back with whether it worked
type Writing<IO> = Pin<Box<dyn Future<Output = (Sink<IO>, bool)>>>;

/// Writes one message. Its `n` bytes stop counting against the limit once the socket has taken
/// the message, before the flush: uWS counts only what the socket hasn't sent yet, and the closest
/// tungstenite comes is the one message it holds while it writes it out. The next one waits for
/// the flush, so that's never more than one.
fn write<IO: AsyncRead + AsyncWrite + Unpin + 'static>(mut sink: Sink<IO>, m: Message, n: usize, queued: Rc<Cell<usize>>) -> Writing<IO> {
    Box::pin(async move {
        let fed = sink.feed(m).await.is_ok();
        queued.set(queued.get().saturating_sub(n));
        let ok = fed && sink.flush().await.is_ok();
        (sink, ok)
    })
}

/// Frames in go to `on_text`, the queue goes out, until either side ends it. A write is one branch
/// among the others, never awaited in place: a peer that stops reading can hold up its own
/// output, but not the idle limit, a close, or `on_close`, which hears whether the connection
/// dropped (true) rather than being closed by either side.
async fn run<IO: AsyncRead + AsyncWrite + Unpin + 'static>(io: IO, mut ends: Ends, limits: Limits, on_open: impl FnOnce(), on_text: impl Fn(&str), on_close: impl FnOnce(bool)) {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_MESSAGE);
    config.max_frame_size = Some(MAX_MESSAGE);
    let ws = WebSocketStream::from_raw_socket(io, Role::Server, Some(config)).await;
    let (sink, mut stream) = ws.split();
    on_open();
    let mut sink = Some(sink);
    let mut writing: Option<Writing<IO>> = None;
    let mut close: Option<(u16, String)> = None;
    let mut goodbye = false;
    let mut last = Instant::now();
    let mut tick = tokio::time::interval(limits.idle / 4);
    tick.tick().await;
    loop {
        tokio::select! {
            c = ends.close_rx.recv() => {
                close = c;
                goodbye = true;
                break;
            }
            _ = tokio::time::sleep_until(last + limits.idle) => break,
            got = stream.next() => match got {
                // a socket the hub has closed (sign-out, a phone removed, too far behind) acts
                // on nothing more from its peer, even while its close is still on the way
                Some(Ok(Message::Text(t))) => {
                    last = Instant::now();
                    if !ends.closed.get() {
                        on_text(t.as_str());
                    }
                }
                Some(Ok(Message::Binary(b))) => {
                    last = Instant::now();
                    if !ends.closed.get() {
                        on_text(&String::from_utf8_lossy(&b));
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => last = Instant::now(),
                Some(Ok(Message::Close(_))) => {
                    goodbye = true;
                    break;
                }
                Some(Err(_)) | None => break,
            },
            done = async { writing.as_mut().expect("a write in progress").await }, if writing.is_some() => {
                writing = None;
                let (s, ok) = done;
                sink = Some(s);
                if !ok {
                    break;
                }
            }
            out = ends.rx.recv(), if writing.is_none() => match out {
                Some(w) => {
                    let (n, m) = match w {
                        Wire::Text(s) => (s.len(), Message::text(s)),
                        Wire::Binary(b) => (b.len(), Message::binary(b)),
                    };
                    writing = sink.take().map(|k| write(k, m, n, ends.queued.clone()));
                }
                None => break,
            },
            _ = tick.tick(), if writing.is_none() => {
                if last.elapsed() >= limits.idle / 4 {
                    writing = sink.take().map(|k| write(k, Message::Ping(Vec::new().into()), 0, ends.queued.clone()));
                }
            }
        }
    }
    ends.closed.set(true);
    // the clients list, terminals and machine links let go now, not after the goodbyes below
    on_close(!goodbye);
    if let Some((code, reason)) = close {
        // the write in progress gets a moment to finish so the close frame can follow it
        if let Some(w) = writing.take() {
            if let Ok((s, true)) = tokio::time::timeout(limits.close_wait, w).await {
                sink = Some(s);
            }
        }
        if let Some(s) = sink.as_mut() {
            let frame = Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() }));
            if let Ok(Ok(())) = tokio::time::timeout(limits.close_wait, s.send(frame)).await {
                // the other side's close reply, briefly
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    while let Some(Ok(m)) = stream.next().await {
                        if m.is_close() {
                            break;
                        }
                    }
                })
                .await;
            }
        }
    }
    // a write still stuck here is dropped with the connection
    if let Some(mut s) = sink {
        let _ = tokio::time::timeout(limits.close_wait, s.close()).await;
    }
}

/// `/ws`: the app. A phone connects with a ticket (a page can't set a header on a WebSocket) or its
/// bearer; either one is judged alone, never a cookie sent along with it.
pub fn app_upgrade(req: Req) -> Resp {
    app_socket(req, false)
}

/// `/ws2`: the app on protocol v2, signed in the same way. Companion apps stay on `/ws`.
pub fn app2_upgrade(req: Req) -> Resp {
    app_socket(req, true)
}

fn app_socket(mut req: Req, v2: bool) -> Resp {
    let ticket = req.query_get("ticket");
    // a companion app: its bearer, and its contract in a header or (for a client that can't set one) ?contract=
    if ticket.is_none() && !v2 {
        if let Some(c) = crate::companions::companion_for(&req) {
            return companion_upgrade(req, c);
        }
    }
    let as_phone = ticket.is_some() || has_bearer(&req);
    let phone = match &ticket {
        Some(t) => redeem_ticket(t, now_ms()),
        None if as_phone => phone_for(&req),
        None => None,
    };
    if as_phone && phone.is_none() {
        return text(401, "Not signed in");
    }
    if !as_phone && !same_origin(&req) {
        return text(403, "Cross-site connection refused");
    }
    if !as_phone && !is_authed(&req) {
        return text(401, "Not signed in");
    }
    let (Some(res), Some(on)) = (handshake(&req), req.extensions.remove::<hyper::upgrade::OnUpgrade>()) else { return text(400, "Upgrade failed") };
    let (sock, ends) = Socket::new();
    let s = sock.clone();
    let client = if v2 {
        new_client2(move |w| s.send(w))
    } else {
        // an app from before the newer agents gets them as plain terminals
        let catalog = req.catalog();
        new_client(move |msg| s.send_text(wire(&inyon_protocol::compat::msg_for(msg, catalog))))
    };
    let s = sock.clone();
    let closer: Closer = Rc::new(move |code, reason| s.close_with(code, reason));
    let release: Box<dyn Fn()> = match &phone {
        Some(p) => Box::new(hold_phone_socket(&p.id, closer)),
        None => Box::new(hold_socket(&req, closer)),
    };
    let (c1, c2, c3) = (client.clone(), client.clone(), client);
    tokio::task::spawn_local(pump(
        on,
        ends,
        move || add(&c1),
        move |t| if v2 { client_message2(&c2, t) } else { client_message(&c2, t) },
        move |_| {
            release();
            close_client(&c3);
        },
    ));
    res
}

/// `/ws` for a companion app: only what its scopes allow goes out, and a frame it may not send
/// ends the connection.
fn companion_upgrade(mut req: Req, c: crate::companions::CompanionRecord) -> Resp {
    if let Some(refused) = crate::companions::contract_refusal(&req, req.query_get("contract")) {
        return refused;
    }
    let (Some(res), Some(on)) = (handshake(&req), req.extensions.remove::<hyper::upgrade::OnUpgrade>()) else { return text(400, "Upgrade failed") };
    let (sock, ends) = Socket::new();
    let scopes = Rc::new(c.scopes.clone());
    let (s, sc) = (sock.clone(), scopes.clone());
    let catalog = req.catalog();
    let client = new_client(move |msg| {
        if let Some(v) = crate::companions::companion_frame(&inyon_protocol::compat::msg_for(msg, catalog), &sc) {
            s.send_text(inyon_core::js::stringify(&v));
        }
    });
    let s = sock.clone();
    let closer: Closer = Rc::new(move |code, reason| s.close_with(code, reason));
    let release = crate::companions::hold_companion_socket(&c.id, closer);
    let (c1, c2, c3) = (client.clone(), client.clone(), client);
    let s = sock;
    tokio::task::spawn_local(pump(
        on,
        ends,
        move || add(&c1),
        move |t| match crate::companions::inbound_refusal(t, &scopes) {
            // a companion may only watch: anything else ends its connection
            Some(why) => s.close_with(4403, &why),
            None => client_message(&c2, t),
        },
        move |_| {
            release();
            close_client(&c3);
        },
    ));
    res
}

/// `/node`: machines running inyon authenticate with their pairing token.
pub async fn node_upgrade(mut req: Req) -> Resp {
    let auth = req.header("authorization").unwrap_or_default();
    let token = inyon_core::re!(r"(?i)^Bearer\s+").replace(&auth, "").into_owned();
    let hash = if token.is_empty() { String::new() } else { sha256(&token) };
    let cfg = (!hash.is_empty())
        .then(|| registry().configs().into_iter().find(|c| c.kind == HostKind::Node && c.token_hash.as_deref().is_some_and(|h| !h.is_empty() && same(h, &hash))))
        .flatten();
    let cfg = match cfg {
        Some(c) if !token_expired(&c, now_ms()) => c,
        other => {
            sleep(500).await;
            // 410: the machine was removed here and should stop calling; it takes its service down
            if other.is_none() && was_removed(&hash) {
                return text(410, "This machine was removed from the hub");
            }
            return text(403, if other.is_some() { "This pairing command expired; get a new one from the hub" } else { "Unknown token" });
        }
    };
    let (Some(res), Some(on)) = (handshake(&req), req.extensions.remove::<hyper::upgrade::OnUpgrade>()) else { return text(400, "Upgrade failed") };
    let (sock, ends) = Socket::new();
    let id = cfg.id.clone();
    let (a, b, c) = (id.clone(), id.clone(), id.clone());
    let as_node: node_link::Socket = sock;
    let (s1, s2) = (as_node.clone(), as_node);
    let attached = Rc::new(RefCell::new(false));
    let at = attached.clone();
    tokio::task::spawn_local(pump(
        on,
        ends,
        move || {
            *at.borrow_mut() = true;
            node_link::attach(&a, s1);
        },
        move |t| node_link::message(&b, t),
        move |dropped| {
            if *attached.borrow() {
                if dropped {
                    node_link::lost(&c, &s2);
                } else {
                    node_link::detach(&c, &s2);
                }
            }
        },
    ));
    let _ = registry().mark_paired(&id);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_large_message_goes_out_and_only_a_queue_already_past_the_limit_closes() {
        let (sock, _ends) = Socket::new();
        sock.send_text("x".repeat(BACKPRESSURE + 1));
        assert!(!sock.closed.get(), "a single message over the limit is sent, as uWS sends it");
        sock.send_text("y".into());
        assert!(sock.closed.get(), "the next one finds the queue past the limit");
    }

    /// The hub's end of a socket over an in-memory pipe whose buffers hold a few KB, so a peer that
    /// doesn't read stalls the hub's writes as a full TCP send buffer would.
    struct Rig {
        sock: Rc<Socket>,
        got: Rc<RefCell<Vec<String>>>,
        closed_at: Rc<Cell<Option<std::time::Instant>>>,
        peer: WebSocketStream<tokio::io::DuplexStream>,
    }

    async fn rig(limits: Limits) -> Rig {
        let (hub_io, peer_io) = tokio::io::duplex(4096);
        let (sock, ends) = Socket::new();
        let got = Rc::new(RefCell::new(vec![]));
        let closed_at = Rc::new(Cell::new(None));
        let (g, c) = (got.clone(), closed_at.clone());
        tokio::task::spawn_local(run(hub_io, ends, limits, || {}, move |t| g.borrow_mut().push(t.to_owned()), move |_| c.set(Some(std::time::Instant::now()))));
        let peer = WebSocketStream::from_raw_socket(peer_io, Role::Client, None).await;
        Rig { sock, got, closed_at, peer }
    }

    /// queues far more than the pipe holds, for a peer that never reads it
    fn flood(sock: &Socket) {
        for _ in 0..64 {
            sock.send_text("z".repeat(32 * 1024));
        }
    }

    async fn until(what: impl Fn() -> bool, within: Duration) -> bool {
        let end = std::time::Instant::now() + within;
        while std::time::Instant::now() < end {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        what()
    }

    #[test]
    fn a_ws2_frame_goes_out_compressed_as_binary_and_text_stays_text() {
        crate::test_util::block(async {
            let mut r = rig(Limits { idle: Duration::from_secs(30), close_wait: Duration::from_millis(200) }).await;
            let json = format!(r#"{{"t":"snapshot","stream":"h:%1","epoch":1,"seq":1,"rows":{{"items":[{}]}}}}"#, r#"{"kind":"user"}"#.repeat(100).replace("}{", "},{"));
            r.sock.send(Wire::Binary(inyon_api::streams::deflate(json.as_bytes())));
            r.sock.send_text(r#"{"t":"pong"}"#.into());
            let next = |m: Option<Result<Message, _>>| m.expect("a message").expect("read");
            let Message::Binary(b) = next(r.peer.next().await) else { panic!("binary first") };
            assert_eq!(miniz_oxide::inflate::decompress_to_vec(&b).unwrap(), json.as_bytes());
            assert!(b.len() < json.len() / 4);
            assert!(matches!(next(r.peer.next().await), Message::Text(t) if t.as_str() == r#"{"t":"pong"}"#));
        });
    }

    #[test]
    fn a_closed_socket_acts_on_nothing_more_and_lets_go_while_its_peer_stops_reading() {
        crate::test_util::block(async {
            let mut r = rig(Limits { idle: Duration::from_secs(30), close_wait: Duration::from_millis(200) }).await;
            flood(&r.sock);
            r.peer.send(Message::text("before")).await.unwrap();
            let got = r.got.clone();
            assert!(until(|| got.borrow().len() == 1, Duration::from_secs(2)).await, "input is read even while output is stuck");
            // signed out (or the phone removed) with text still queued
            r.sock.close_with(4401, "Signed out");
            let _ = r.peer.send(Message::text("term.in after sign-out")).await;
            let closed_at = r.closed_at.clone();
            assert!(until(|| closed_at.get().is_some(), Duration::from_secs(2)).await, "on_close runs though the peer reads nothing");
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(*r.got.borrow(), ["before"]);
        });
    }

    #[test]
    fn from_the_moment_it_is_closed_a_socket_runs_no_text_or_binary_frame() {
        crate::test_util::block(async {
            let mut r = rig(Limits { idle: Duration::from_secs(30), close_wait: Duration::from_millis(200) }).await;
            r.peer.send(Message::text("before")).await.unwrap();
            let got = r.got.clone();
            assert!(until(|| got.borrow().len() == 1, Duration::from_secs(2)).await);
            // closed, its close not yet picked up by the socket task: only the frames are there
            // to read, so they reach the guard every time
            r.sock.closed.set(true);
            r.peer.send(Message::text("term.in")).await.unwrap();
            r.peer.send(Message::binary(b"term.in as binary".to_vec())).await.unwrap();
            r.peer.send(Message::Ping(Vec::new().into())).await.unwrap();
            // the ping's answer shows both frames before it were read
            let pong = tokio::time::timeout(Duration::from_secs(2), r.peer.next()).await;
            assert!(matches!(pong, Ok(Some(Ok(Message::Pong(_))))), "{pong:?}");
            assert_eq!(*r.got.borrow(), ["before"]);
        });
    }

    #[test]
    fn three_upload_pieces_to_a_slow_machine_go_out_and_a_fourth_finds_it_too_far_behind() {
        crate::test_util::block(async {
            let r = rig(Limits { idle: Duration::from_secs(30), close_wait: Duration::from_millis(200) }).await;
            // a 2 MB file in a host.exec frame, base64 and all; the machine reads nothing for now
            let piece = || "p".repeat(2_800_000);
            r.sock.send_text(piece());
            // the socket task takes it and starts writing; the core goes on meanwhile
            tokio::time::sleep(Duration::from_millis(100)).await;
            r.sock.send_text(piece());
            tokio::time::sleep(Duration::from_millis(100)).await;
            r.sock.send_text(piece());
            assert!(!r.sock.closed.get(), "the piece being written no longer counts, so 2.8 MB waits");
            r.sock.send_text(piece());
            assert!(r.sock.closed.get(), "5.6 MB waiting is past the limit");
        });
    }

    #[test]
    fn a_peer_that_stops_reading_and_says_nothing_is_dropped_at_the_idle_limit() {
        crate::test_util::block(async {
            let r = rig(Limits { idle: Duration::from_millis(300), close_wait: Duration::from_millis(200) }).await;
            flood(&r.sock);
            let closed_at = r.closed_at.clone();
            assert!(until(|| closed_at.get().is_some(), Duration::from_secs(3)).await, "the idle limit holds with a write stuck");
            // and the core stops queueing for it
            r.sock.send_text("late".into());
            assert!(r.sock.closed.get());
        });
    }
}
