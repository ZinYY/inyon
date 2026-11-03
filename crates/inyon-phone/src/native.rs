//! The core outside a browser, as the Android app runs it (from 1.0): a thread of its own with
//! tokio's event loop (inyon-backends' core thread), its records as files, WebSockets and HTTPS of
//! its own, and the app's screens and requests handed to it from other threads.
//!
//! The thread lives as long as the process, not as long as a screen: Android may close the app's
//! window and open a new one while the process (and the connections to the computers) stays.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::LocalBoxFuture;
use futures_util::{SinkExt, StreamExt};
use http::{HeaderMap, HeaderValue, Method};
use inyon_api::body::Body;
use inyon_api::clients::{self, ClientRef};
use inyon_api::http::Req;
use inyon_api::streams::client_message2;
/// A frame for a screen: JSON text, or compressed once the screen asked for that.
pub use inyon_api::streams::Wire as Frame;
use inyon_backends::fs_store::FsStore;
pub use inyon_backends::runtime::CoreGone;
use inyon_backends::runtime::{core_thread, CoreHandle};
use inyon_core::platform::console;
use inyon_core::platform::fetch::{FetchRequest, FetchResponse, Fetcher};
use inyon_core::platform::rt::spawn;
use inyon_core::platform::store::{MemoryStore, Store};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::Connector;

use crate::link::{pkarr_resolve, Dialer, OnWire, Reach, Wire, WireEvent};
use crate::phone::{self, Phone};
use crate::ssh::SshPlugin;

/// Where log lines go: `true` for an error.
pub type Log = Arc<dyn Fn(bool, &str) + Send + Sync>;

/// What the app gives the core.
pub struct Options {
    /// where the records go, one JSON file per key
    pub dir: PathBuf,
    /// what paired computers list this phone as
    pub name: String,
    /// the SSH plugin, made on the core's thread, since it may hold what can't cross threads
    pub ssh: Box<dyn FnOnce() -> Rc<dyn SshPlugin> + Send>,
    /// the core's log lines (stderr when not given)
    pub log: Option<Log>,
    /// no requests to the web and no sockets: for tests
    pub offline: bool,
}

thread_local! {
    static PHONE: RefCell<Option<Rc<Phone>>> = const { RefCell::new(None) };
    static SCREENS: RefCell<HashMap<u64, ClientRef>> = RefCell::new(HashMap::new());
}

static NEXT_SCREEN: AtomicU64 = AtomicU64::new(1);

/// The running core, from any thread. Cloning it is cheap; the core stops only with the process.
#[derive(Clone)]
pub struct PhoneHandle {
    core: CoreHandle,
}

/// An answer to a request: its status and whole body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Starts the core on a thread of its own: the records from `dir`, then the machines.
pub fn start(o: Options) -> PhoneHandle {
    let (core, _thread) = core_thread(move || {
        if let Some(log) = o.log {
            console::set_sink(Some(Rc::new(move |error, line| log(error, line))));
        }
        let store: Rc<dyn Store> = match FsStore::new(&o.dir) {
            Ok(s) => Rc::new(s),
            Err(e) => {
                // nothing saved survives, but the phone still works until it closes
                console::error(&format!("Can't keep the phone's records in {}: {e}", o.dir.display()));
                Rc::new(MemoryStore::default())
            }
        };
        let (dialer, fetcher): (Rc<dyn Dialer>, Option<Rc<dyn Fetcher>>) = if o.offline { (Rc::new(Offline), None) } else { (Rc::new(NativeDialer), Some(Rc::new(NativeFetcher))) };
        let reach = Reach { dialer, resolve: pkarr_resolve(), plain_ws: true };
        let phone = phone::start(phone::Options { store, fetcher, ssh: (o.ssh)(), reach, name: o.name, rotate_push: Rc::new(|| {}) });
        PHONE.with(|p| *p.borrow_mut() = Some(Rc::new(phone)));
    });
    PhoneHandle { core }
}

/// The core's phone; only on the core's thread, once it started.
fn phone() -> Option<Rc<Phone>> {
    PHONE.with(|p| p.borrow().clone())
}

/// What the app asks for as a request to the hub would be: the path with its query.
pub fn request_of(method: &str, path: &str, body: Option<Vec<u8>>) -> Req {
    let method: Method = method.parse().unwrap_or(Method::GET);
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut headers = HeaderMap::new();
    if body.is_some() {
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    Req {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        peer: ([127, 0, 0, 1], 0).into(),
        https: false,
        body: body.map(Body::from),
        params: vec![],
        extensions: http::Extensions::new(),
    }
}

impl PhoneHandle {
    /// Answers an /api request the way the hub would.
    pub fn request(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> impl Future<Output = Result<Answer, CoreGone>> + Send + 'static {
        let req = request_of(method, path, body);
        self.core.call(move || async move {
            let Some(phone) = phone() else { return Answer { status: 503, body: br#"{"error":"The phone is still starting"}"#.to_vec() } };
            let (parts, body) = phone.answer(req).await.into_parts();
            Answer { status: parts.status.as_u16(), body: body.bytes().await.map(|b| b.to_vec()).unwrap_or_default() }
        })
    }

    /// A screen connecting on protocol v2: `out` hears each frame, on the core's thread.
    pub fn connect(&self, out: impl Fn(Frame) + Send + 'static) -> Screen {
        let id = NEXT_SCREEN.fetch_add(1, Ordering::Relaxed);
        let _ = self.core.send(move || async move {
            let Some(phone) = phone() else { return };
            let c = phone.connect2(out);
            SCREENS.with(|s| s.borrow_mut().insert(id, c));
        });
        Screen { id, core: self.core.clone() }
    }

    /// The app went to the background (true) or came back (false).
    pub fn background(&self, hidden: bool) {
        let _ = self.core.send(move || async move {
            if let Some(p) = phone() {
                p.background(hidden);
                if !hidden {
                    p.resume();
                }
            }
        });
    }

    /// Runs `job` on the core's thread with the phone, once it started.
    /// None when the core isn't running.
    pub fn with_phone<R, Fut>(&self, job: impl FnOnce(Rc<Phone>) -> Fut + Send + 'static) -> impl Future<Output = Option<R>> + Send + 'static
    where
        R: Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let call = self.core.call(move || async move {
            match phone() {
                Some(p) => Some(job(p).await),
                None => None,
            }
        });
        async move { call.await.ok().flatten() }
    }

    /// Starts `job` on the core's thread without waiting for it.
    pub fn spawn<Fut: Future<Output = ()> + 'static>(&self, job: impl FnOnce() -> Fut + Send + 'static) {
        let _ = self.core.send(job);
    }
}

/// One connected screen. What it sends reaches the core in order; dropping it disconnects.
pub struct Screen {
    id: u64,
    core: CoreHandle,
}

impl Screen {
    /// A message from the screen, as the text it would send the hub's `/ws2`.
    pub fn send(&self, text: String) {
        let id = self.id;
        let _ = self.core.send(move || async move {
            if let Some(c) = SCREENS.with(|s| s.borrow().get(&id).cloned()) {
                client_message2(&c, &text);
            }
        });
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let id = self.id;
        let _ = self.core.send(move || async move {
            if let Some(c) = SCREENS.with(|s| s.borrow_mut().remove(&id)) {
                clients::close_client(&c);
            }
        });
    }
}

// ---------- WebSockets, for the link ----------

/// The certificates every phone trusts: Mozilla's, as the browser's would be.
fn tls() -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS 1.2 and 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(config)
}

enum Out {
    Data(Vec<u8>),
    Close(u16, String),
}

/// The socket as the link sees it; what's sent before it opens waits for it, as a browser's does.
struct NativeWire(mpsc::UnboundedSender<Out>);

impl Wire for NativeWire {
    fn send(&self, data: &[u8]) {
        let _ = self.0.send(Out::Data(data.to_vec()));
    }
    fn close(&self, code: u16, reason: &str) {
        let _ = self.0.send(Out::Close(code, reason.to_owned()));
    }
}

/// No network at all (`Options::offline`).
struct Offline;

impl Dialer for Offline {
    fn dial(&self, _url: &str, _on: OnWire) -> Result<Rc<dyn Wire>, String> {
        Err("offline".into())
    }
}

/// Opens WebSockets with tokio-tungstenite: `ws://` on the local network, `wss://` to relays.
pub struct NativeDialer;

impl Dialer for NativeDialer {
    fn dial(&self, url: &str, on: OnWire) -> Result<Rc<dyn Wire>, String> {
        let req = url.into_client_request().map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::unbounded_channel();
        spawn(run_socket(req, rx, on));
        Ok(Rc::new(NativeWire(tx)))
    }
}

/// How long a socket may take to open, the TLS handshake included.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

async fn run_socket(req: tokio_tungstenite::tungstenite::handshake::client::Request, mut rx: mpsc::UnboundedReceiver<Out>, on: OnWire) {
    let config = WebSocketConfig::default().max_message_size(Some(64 << 20)).max_frame_size(Some(64 << 20));
    // no Nagle: keys typed into a terminal go out at once instead of waiting for more to send
    let open = tokio_tungstenite::connect_async_tls_with_config(req, Some(config), true, Some(Connector::Rustls(tls())));
    let ws = match tokio::time::timeout(OPEN_TIMEOUT, open).await {
        Ok(Ok((ws, _))) => ws,
        // a socket that never opened, as a browser reports it
        Ok(Err(e)) => return on(WireEvent::Close(1006, e.to_string())),
        Err(_) => return on(WireEvent::Close(1006, "timed out".into())),
    };
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(Out::Data(d)) => {
                    if let Err(e) = sink.send(Message::Binary(d.into())).await {
                        return on(WireEvent::Close(1006, e.to_string()));
                    }
                }
                Some(Out::Close(code, reason)) => {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.clone().into() }))).await;
                    return on(WireEvent::Close(code, reason));
                }
                // nothing holds the socket any more
                None => {
                    let _ = sink.send(Message::Close(None)).await;
                    return;
                }
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(b))) => on(WireEvent::Message(b.to_vec())),
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = frame.map_or((1005, String::new()), |f| (u16::from(f.code), f.reason.to_string()));
                    return on(WireEvent::Close(code, reason));
                }
                // text frames aren't the link's; pings are answered by tungstenite
                Some(Ok(_)) => {}
                Some(Err(e)) => return on(WireEvent::Close(1006, e.to_string())),
                None => return on(WireEvent::Close(1006, String::new())),
            },
        }
    }
}

// ---------- fetch ----------

/// The most a fetched body may hold: a release list, a registry answer, never a download.
const MAX_BODY: u64 = 16 * 1024 * 1024;

/// The core's few requests to the web (pkarr relays, the newest release, npm), with ureq on a
/// blocking thread.
pub struct NativeFetcher;

impl Fetcher for NativeFetcher {
    fn fetch(&self, req: FetchRequest) -> LocalBoxFuture<'static, Result<FetchResponse, String>> {
        Box::pin(async move { tokio::task::spawn_blocking(move || fetch_now(req)).await.map_err(|e| e.to_string())? })
    }
}

fn fetch_now(req: FetchRequest) -> Result<FetchResponse, String> {
    let timeout = (req.timeout_ms > 0).then(|| Duration::from_millis(req.timeout_ms));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(format!("inyon-phone/{}", env!("CARGO_PKG_VERSION")))
        .timeout_global(timeout)
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::WebPki).build())
        .build()
        .into();
    let mut b = http::Request::builder().method(req.method.as_str()).uri(&req.url);
    for (k, v) in &req.headers {
        b = b.header(k, v);
    }
    let request = b.body(req.body.unwrap_or_default()).map_err(|e| e.to_string())?;
    let mut res = agent.run(request).map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let body = res.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| e.to_string())?;
    Ok(FetchResponse { status, body })
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
