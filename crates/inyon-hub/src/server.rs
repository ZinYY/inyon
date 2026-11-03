//! The server half of index.ts: the route table Bun.serve had, the checks every API route goes
//! through (the hub's own names, same origin, sign-in, CORS for the phone app), the fallback for
//! everything else (the WebSocket upgrades, the web app's files), and the listener itself.
//!
//! Threading. Everything the hub keeps (its records, the registry and chat watchers of
//! inyon-core, sign-ins, phones, open connections) lives on one thread, the core's, as the TS
//! hub lived on Bun's one event loop. The listener runs on that same thread's tokio runtime. axum
//! hands each request to one handler that sends it, whole, to the core as a job of its own
//! (`CoreHandle::call`) and waits for the answer. Two things follow: no route races another on
//! shared state, since a synchronous stretch on the core thread is never interleaved, and a
//! request whose client goes away still runs to its end, since dropping the handler only drops
//! the wait for the answer, not the job. WebSockets are the same: the socket's own task only
//! moves frames, and every frame is handled on the core thread in the order it came.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::Method;
use inyon_backends::CoreHandle;
use inyon_core::platform::console;

use crate::auth::is_authed;
use crate::http::{answer, json, set_header, text, Body, Req, Resp};
use crate::phones::has_bearer;

type KnownHost = Box<dyn Fn(Option<&str>) -> bool>;

pub use inyon_api::routes::{guarded, guarded_raw, h, raw, ApiHandler, Handler, Route, Routes};

/// The hub's route table: every `guarded` route behind `api_for`, with what a companion may do there.
pub fn hub_routes() -> Routes {
    Routes::with_guard(Rc::new(|pattern, m, f| api_for(f, Opts::default(), crate::companions::route_for(pattern, m))))
}

// ---------- the hub's settings and state on the core thread ----------

/// What index.ts worked out as it started, for every route.
pub struct HubCtx {
    pub data_dir: PathBuf,
    pub bind: String,
    pub port: u16,
    pub tls: bool,
    /// INYON_ALLOWED_ORIGINS
    pub allowed_origins: HashSet<String>,
    known_host: KnownHost,
    /// this machine's names on its private networks, found as the hub starts
    pub network_names: Rc<RefCell<HashSet<String>>>,
    refused_hosts: RefCell<HashSet<String>>,
    /// the first-run code, while the hub has no password
    pub setup_code: RefCell<Option<String>>,
    pub setup_attempts: Cell<(u32, u64)>,
    /// wrong setup codes per address this minute (`setup_attempts` covers all of them)
    pub setup_tries: RefCell<HashMap<String, u32>>,
    pub local_token: String,
    pub hub_commit: String,
    pub core: CoreHandle,
}

thread_local! {
    static CTX: RefCell<Option<Rc<HubCtx>>> = const { RefCell::new(None) };
    static ROUTES: RefCell<Option<Rc<Routes>>> = const { RefCell::new(None) };
}

impl HubCtx {
    /// Whether a Host header names this hub (see hostnames), with no cookie looked at.
    pub fn answers_to(&self, host: &str) -> bool {
        (self.known_host)(Some(host))
    }
}

/// The hub's context; only on the core thread, once the hub has started.
pub fn ctx() -> Rc<HubCtx> {
    CTX.with(|c| c.borrow().clone()).expect("the hub has started on this thread")
}

pub struct CtxInit {
    pub data_dir: PathBuf,
    pub bind: String,
    pub port: u16,
    pub tls: bool,
    pub allowed_hosts: Option<String>,
    pub allowed_origins: Option<String>,
    pub hostname: String,
    pub setup_code: Option<String>,
    pub local_token: String,
    pub hub_commit: String,
    pub core: CoreHandle,
}

pub fn install_ctx(i: CtxInit, routes: Routes) {
    let network_names = Rc::new(RefCell::new(HashSet::new()));
    let allowed_origins = i.allowed_origins.as_deref().unwrap_or("").split(',').map(|o| o.trim().trim_end_matches('/').to_owned()).filter(|o| !o.is_empty()).collect();
    let known = crate::hostnames::allowed_hosts(i.allowed_hosts.as_deref(), i.allowed_origins.as_deref(), &i.hostname, &i.bind, network_names.clone());
    let c = HubCtx {
        data_dir: i.data_dir,
        bind: i.bind,
        port: i.port,
        tls: i.tls,
        allowed_origins,
        known_host: Box::new(known),
        network_names,
        refused_hosts: RefCell::new(HashSet::new()),
        setup_code: RefCell::new(i.setup_code),
        setup_attempts: Cell::new((0, 0)),
        setup_tries: RefCell::new(HashMap::new()),
        local_token: i.local_token,
        hub_commit: i.hub_commit,
        core: i.core,
    };
    CTX.with(|x| *x.borrow_mut() = Some(Rc::new(c)));
    ROUTES.with(|r| *r.borrow_mut() = Some(Rc::new(routes)));
}

// ---------- the checks ----------

/// A browser request made by this app's own pages. Other sites can share the hub's cookie when they
/// run on the same host (a dev server on another port, a sibling subdomain), so anything that
/// changes state, and the WebSocket, must come from the hub's own origin. Requests without an
/// Origin header don't come from a web page (curl, the node agent) and carry no ambient cookie risk.
pub fn same_origin(req: &Req) -> bool {
    let Some(origin) = req.header("origin") else { return true };
    if ctx().allowed_origins.contains(&origin) {
        return true;
    }
    let host = req.header("x-forwarded-host").or_else(|| req.header("host"));
    match (host, url::Url::parse(&origin)) {
        (Some(host), Ok(u)) => !host.is_empty() && url_host(&u) == host,
        _ => false,
    }
}

/// `new URL(origin).host`: the hostname with the port when it isn't the scheme's default.
fn url_host(u: &url::Url) -> String {
    let h = match u.host() {
        Some(url::Host::Ipv6(a)) => format!("[{a}]"),
        Some(h) => h.to_string(),
        None => String::new(),
    };
    match u.port() {
        Some(p) => format!("{h}:{p}"),
        None => h,
    }
}

pub const WRONG_HOST: &str = "This hub doesn't answer to that address. If it's yours, add it to INYON_ALLOWED_HOSTS on the hub.";

/// Sent to a name the hub answers to (see hostnames), including the one a local proxy forwards.
/// A signed-in browser passes under any name: its cookie can only have been set by the hub itself.
pub fn right_host(req: &Req) -> bool {
    if is_authed(req) || crate::companions::companion_for(req).is_some() {
        return true;
    }
    let c = ctx();
    let mut names: Vec<String> = req.header("host").into_iter().collect();
    if let Some(f) = req.header("x-forwarded-host") {
        names.extend(f.split(',').map(|h| h.trim().to_owned()));
    }
    let wrong = names.iter().find(|h| !(c.known_host)(Some(h))).cloned().or_else(|| names.is_empty().then(|| "(none)".to_owned()));
    let Some(wrong) = wrong else { return true };
    let mut refused = c.refused_hosts.borrow_mut();
    if !refused.contains(&wrong) && refused.len() < 50 {
        refused.insert(wrong.clone());
        let shown: String = wrong.chars().take(200).collect();
        console::log(&format!("Refused a request for host {}. If that is this hub's address, add it to INYON_ALLOWED_HOSTS.", serde_json::Value::String(shown)));
    }
    false
}

/// The phone app runs on an origin of its own and signs in with a bearer token, so its requests
/// are cross-origin and need CORS answers. That is safe where no cookie is read: a request with a
/// bearer is judged by the bearer alone (see is_authed), and these two routes never look at one.
const CORS_PATHS: [&str; 2] = ["/api/phones/claim", "/api/phones/ticket"];

fn phone_request(req: &Req) -> bool {
    has_bearer(req) || CORS_PATHS.contains(&req.path.as_str())
}

/// A call from a page with no origin of its own (a sandboxed frame, such as a page served under
/// /v/) or from another site. The browser may still attach the hub's cookie, so these are refused
/// for every method, reads included. A bearer request carries no ambient credential and passes.
fn framed_caller(req: &Req) -> bool {
    let origin = req.header("origin");
    if origin.as_deref() == Some("null") {
        return true;
    }
    req.header("sec-fetch-site").as_deref() == Some("cross-site") && !origin.is_some_and(|o| ctx().allowed_origins.contains(&o))
}

fn allow_origin(mut res: Resp, origin: &str) -> Resp {
    set_header(&mut res, "access-control-allow-origin", origin);
    set_header(&mut res, "vary", "Origin");
    set_header(&mut res, "access-control-allow-headers", "authorization, content-type");
    set_header(&mut res, "access-control-allow-methods", "GET, POST, PUT, PATCH, DELETE");
    res
}

/// An API route's answer to a name the hub doesn't answer to. A page on another origin (the phone
/// app's) may read it, so its "Can't connect?" can tell a wrong name from a hub that isn't there.
/// It carries only the origin it may be read from: the answer says nothing else, and no method or
/// header is allowed with it.
fn wrong_host(req: &Req) -> Resp {
    let mut res = json(421, &serde_json::json!({ "error": WRONG_HOST }));
    if let Some(o) = req.header("origin") {
        set_header(&mut res, "access-control-allow-origin", &o);
        set_header(&mut res, "vary", "Origin");
    }
    res
}

fn cors(req_origin: Option<&str>, phone: bool, res: Resp) -> Resp {
    match req_origin {
        Some(o) if phone => allow_origin(res, o),
        _ => res,
    }
}

/// A preflight carries no token yet; the one for a bearer request names the header it will send.
pub fn preflight(req: &Req) -> Option<Resp> {
    if req.method != Method::OPTIONS {
        return None;
    }
    let asked = req.header("access-control-request-headers").unwrap_or_default();
    let for_bearer = inyon_core::re!(r"(?i)\bauthorization\b").is_match(&asked);
    if !req.path.starts_with("/api/") || !(for_bearer || CORS_PATHS.contains(&req.path.as_str())) {
        return None;
    }
    if !right_host(req) {
        return Some(json(421, &serde_json::json!({ "error": WRONG_HOST })));
    }
    let mut res = text(204, "");
    res.headers_mut().remove("content-type");
    set_header(&mut res, "access-control-max-age", "600");
    Some(match req.header("origin") {
        Some(o) => allow_origin(res, &o),
        None => res,
    })
}

/// Who may call an API route: `open` needs no sign-in; `owner` refuses a paired phone (the hub's
/// own account is the browser's to manage).
#[derive(Clone, Copy, Default)]
pub struct Opts {
    pub open: bool,
    pub owner: bool,
}

pub const OPEN: Opts = Opts { open: true, owner: false };
pub const OWNER: Opts = Opts { open: false, owner: true };
pub const OPEN_OWNER: Opts = Opts { open: true, owner: true };

/// `api(fn, opts)`: an API handler behind the hub's checks, its errors as JSON.
pub fn api(f: ApiHandler, o: Opts) -> Handler {
    api_for(f, o, None)
}

/// `api(fn, { ...opts, companion })`: `companion` is what a companion app may do on this route,
/// with which scope; without it a companion is refused.
pub fn api_for(f: ApiHandler, o: Opts, companion: Option<crate::companions::CompanionRoute>) -> Handler {
    Rc::new(move |req: Req| {
        let f = f.clone();
        let companion = companion.clone();
        Box::pin(async move {
            if let Some(pre) = preflight(&req) {
                return pre;
            }
            if !right_host(&req) {
                return wrong_host(&req);
            }
            // a companion's token is judged on its own: its contract, then its scope for this route
            if let Some(c) = crate::companions::companion_for(&req) {
                return crate::companions::as_companion(req, c, companion).await;
            }
            // the inyon command, with a joined machine's token or the hub's local one
            if let Some(who) = crate::machines::caller(&req) {
                return crate::machines::as_caller(req, who).await;
            }
            let origin = req.header("origin");
            let phone = phone_request(&req);
            if o.owner && has_bearer(&req) {
                return cors(origin.as_deref(), phone, json(403, &serde_json::json!({ "error": "Not from a phone. Do this in the browser." })));
            }
            if !phone && framed_caller(&req) {
                return json(403, &serde_json::json!({ "error": "Cross-site request refused" }));
            }
            // a bearer request carries no ambient credential, so CSRF doesn't apply to it
            if req.method != Method::GET && req.method != Method::HEAD && !phone && !same_origin(&req) {
                return json(403, &serde_json::json!({ "error": "Cross-site request refused" }));
            }
            // a bearer this hub doesn't know was forgotten here (or never was this hub's): the phone may
            // forget the hub on this answer, and only on this one, not on any 401 a proxy gives
            if !o.open && !is_authed(&req) {
                let body = if has_bearer(&req) { serde_json::json!({ "error": "Not signed in", "reason": "revoked" }) } else { serde_json::json!({ "error": "Not signed in" }) };
                return cors(origin.as_deref(), phone, json(401, &body));
            }
            let res = answer(f(req).await);
            cors(origin.as_deref(), phone, res)
        })
    })
}

// ---------- requests ----------

/// Answers one request on the core thread: its route if one matches, else the fallback.
pub async fn dispatch(mut req: Req) -> Resp {
    let routes = ROUTES.with(|r| r.borrow().clone()).expect("routes");
    if let Some((h, params)) = routes.handler(&req.path, &req.method) {
        req.params = params;
        return h(req).await;
    }
    fallback(req).await
}

/// Bun.serve's `fetch`: what no route took. Routes are matched on the path as it came, as Bun's
/// router matches them; `fetch` reads `new URL(req.url).pathname`, with its dot segments resolved,
/// so this does too.
async fn fallback(mut req: Req) -> Resp {
    req.path = inyon_core::features::pages_core::normalize_target(&req.path).0;
    if req.path == "/node" {
        return crate::ws::node_upgrade(req).await;
    }
    if req.path == "/ws" {
        return crate::ws::app_upgrade(req);
    }
    if req.path == "/ws2" {
        return crate::ws::app2_upgrade(req);
    }
    // a page's own files: the token in the path is the only credential, never the cookie, and the
    // Host is checked there without one (pages::view)
    if req.path.starts_with("/v/") {
        return crate::pages::view(req).await;
    }
    // a route that only lists some methods leaves OPTIONS to land here
    if let Some(pre) = preflight(&req) {
        return pre;
    }
    if req.path.starts_with("/api/") {
        return json(404, &serde_json::json!({ "error": "Not found" }));
    }
    if !right_host(&req) {
        let mut r = text(421, format!("{WRONG_HOST}\n"));
        crate::static_files::page_headers(&mut r);
        return r;
    }
    let gzip = crate::static_files::takes_gzip(req.header("accept-encoding").as_deref());
    // the new app (apps/app) while it catches up with the web app at /
    if req.path == "/next" || req.path.starts_with("/next/") {
        return crate::static_files::serve_next(&req.path, gzip);
    }
    crate::static_files::serve(&req.path, gzip)
}

// ---------- the listener ----------

#[derive(Clone)]
struct App {
    core: CoreHandle,
    https: bool,
}

/// The one axum handler: the request goes to the core thread whole, as a job of its own.
async fn handle(State(app): State<App>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, req: Request) -> axum::response::Response {
    use futures_util::StreamExt;
    let (parts, body) = req.into_parts();
    let r = Req {
        method: parts.method,
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().unwrap_or("").to_owned(),
        headers: parts.headers,
        peer,
        https: app.https,
        body: Some(Body::from_stream(body.into_data_stream().map(|c| c.map_err(std::io::Error::other)))),
        params: vec![],
        // with hyper's OnUpgrade, for the WebSocket routes
        extensions: parts.extensions,
    };
    let res = match app.core.call(move || dispatch(r)).await {
        // a page's file is read here, off the core's job (pages::view only named it)
        Ok(mut res) => match res.extensions_mut().remove::<crate::pages::ViewFile>().and_then(|f| f.file.lock().ok()?.take().map(|file| (file, f.size))) {
            Some((file, size)) => {
                *res.body_mut() = crate::pages::file_body(file, size);
                res
            }
            None => res,
        },
        // the job panicked (and only it ended), or the hub is stopping
        Err(e) => json(500, &serde_json::json!({ "error": e.to_string() })),
    };
    let (parts, body) = res.into_parts();
    axum::response::Response::from_parts(parts, axum_body(body))
}

/// An answer's body as axum sends it.
fn axum_body(b: Body) -> axum::body::Body {
    match b {
        Body::Empty => axum::body::Body::empty(),
        Body::Full(b) => axum::body::Body::from(b),
        Body::Stream(s) => axum::body::Body::from_stream(s),
    }
}

/// The client's address, from a plain or a TLS connection.
#[derive(Clone, Copy, Debug)]
pub struct Peer(pub SocketAddr);

type PlainListener = crate::idle::IdleListener<tokio::net::TcpListener>;
type SecureListener = crate::idle::IdleListener<crate::tls::TlsListener>;

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, PlainListener>> for Peer {
    fn connect_info(s: axum::serve::IncomingStream<'_, PlainListener>) -> Self {
        Peer(*s.remote_addr())
    }
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, SecureListener>> for Peer {
    fn connect_info(s: axum::serve::IncomingStream<'_, SecureListener>) -> Self {
        Peer(*s.remote_addr())
    }
}

pub fn router(core: CoreHandle, https: bool) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(App { core, https })
}

/// Starts serving on `listener` (TLS when `tls` holds a config), on this thread's runtime.
pub fn serve(listener: tokio::net::TcpListener, tls: Option<std::sync::Arc<rustls::ServerConfig>>, core: CoreHandle) {
    let app = router(core, tls.is_some()).into_make_service_with_connect_info::<Peer>();
    match tls {
        None => {
            let l = crate::idle::IdleListener::new(listener, crate::idle::IDLE);
            tokio::spawn(async move {
                if let Err(e) = axum::serve(l, app).await {
                    console::error(&format!("the hub's server stopped: {e}"));
                }
            });
        }
        Some(cfg) => {
            let l = crate::idle::IdleListener::new(crate::tls::TlsListener::new(listener, cfg), crate::idle::IDLE);
            tokio::spawn(async move {
                if let Err(e) = axum::serve(l, app).await {
                    console::error(&format!("the hub's server stopped: {e}"));
                }
            });
        }
    }
}

pub use inyon_api::http::test_req;
