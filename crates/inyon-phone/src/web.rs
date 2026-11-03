//! The JavaScript side: what `startPhone` takes and gives back (apps/web/src/lib/phone-types.ts), the
//! browser's event loop as the core's runtime, the app's store, the native SSH plugin, WebSockets
//! for the link, `fetch`, and Request and Response in and out. Only globals every engine has
//! (setTimeout, fetch, WebSocket, Request, Response), so the same module runs in the Android
//! WebView, a browser tab and Bun's tests.

use std::cell::Cell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use inyon_api::body::Body;
use inyon_api::clients::{self, ClientRef};
use inyon_api::http::{Req, Resp};
use inyon_core::platform::fetch::{FetchRequest, FetchResponse, Fetcher};
use inyon_core::platform::rt::{install, Runtime};
use inyon_core::platform::store::Store;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array, JSON};
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, JsFuture};

use crate::link::{pkarr_resolve, Dialer, OnWire, Reach, Wire, WireEvent};
use crate::phone::{self, Options, Phone};
use crate::ssh::{OnEvent, PluginError, PluginResult, SshPlugin};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(f: &Function, ms: f64) -> JsValue;
    #[wasm_bindgen(js_name = fetch)]
    fn global_fetch(url: &str, init: &Object) -> Promise;
    #[wasm_bindgen(js_namespace = AbortSignal, js_name = timeout)]
    fn abort_after(ms: f64) -> JsValue;
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn console_log(line: &str);
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(line: &str);

    #[wasm_bindgen(js_name = Response)]
    type JsResponse;
    #[wasm_bindgen(constructor, js_class = "Response")]
    fn new(body: &JsValue, init: &Object) -> JsResponse;

    #[wasm_bindgen(js_name = WebSocket)]
    type JsWebSocket;
    #[wasm_bindgen(constructor, js_class = "WebSocket", catch)]
    fn new(url: &str) -> Result<JsWebSocket, JsValue>;
    #[wasm_bindgen(method, setter = binaryType)]
    fn set_binary_type(this: &JsWebSocket, t: &str);
    #[wasm_bindgen(method, getter = readyState)]
    fn ready_state(this: &JsWebSocket) -> u16;
    #[wasm_bindgen(method, catch, js_name = send)]
    fn send_bytes(this: &JsWebSocket, data: &Uint8Array) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = close)]
    fn close_with(this: &JsWebSocket, code: u16, reason: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = close)]
    fn close_plain(this: &JsWebSocket) -> Result<(), JsValue>;
    #[wasm_bindgen(method, setter = onopen)]
    fn set_onopen(this: &JsWebSocket, f: &JsValue);
    #[wasm_bindgen(method, setter = onmessage)]
    fn set_onmessage(this: &JsWebSocket, f: &JsValue);
    #[wasm_bindgen(method, setter = onclose)]
    fn set_onclose(this: &JsWebSocket, f: &JsValue);
}

// ---------- the runtime ----------

/// The browser's event loop: tasks are promises' continuations, timers are setTimeout.
struct BrowserRuntime;

impl Runtime for BrowserRuntime {
    fn spawn(&self, fut: LocalBoxFuture<'static, ()>) {
        wasm_bindgen_futures::spawn_local(fut);
    }
    fn sleep(&self, ms: u64) -> LocalBoxFuture<'static, ()> {
        let p = Promise::new(&mut |resolve, _| {
            set_timeout(&resolve, ms as f64);
        });
        Box::pin(async move {
            let _ = JsFuture::from(p).await;
        })
    }
    fn now_ms(&self) -> u64 {
        js_sys::Date::now() as u64
    }
}

// ---------- the store ----------

/// The app's `PhoneStore` (IndexedDB behind a map it loaded first): values cross as JSON.
struct JsStore(JsValue);

impl JsStore {
    fn call(&self, name: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = Reflect::get(&self.0, &name.into())?.dyn_into()?;
        f.apply(&self.0, &args.iter().copied().collect::<Array>())
    }
}

impl Store for JsStore {
    fn get(&self, key: &str) -> Option<Value> {
        let v = self.call("get", &[&key.into()]).ok()?;
        if v.is_undefined() {
            return None;
        }
        inyon_core::js::json::parse(&JSON::stringify(&v).ok()?.as_string()?)
    }
    fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        let v = JSON::parse(&inyon_core::js::stringify(value)).map_err(|e| describe(&e))?;
        self.call("set", &[&key.into(), &v]).map(|_| ()).map_err(|e| describe(&e))
    }
    fn del(&self, key: &str) {
        let _ = self.call("del", &[&key.into()]);
    }
    fn keys(&self, prefix: &str) -> Vec<String> {
        self.call("keys", &[&prefix.into()]).ok().map(|v| Array::from(&v).iter().filter_map(|k| k.as_string()).collect()).unwrap_or_default()
    }
}

// ---------- the SSH plugin ----------

/// The native InyonSsh plugin (Capacitor's bridge): arguments and answers cross as JSON.
struct JsSsh(JsValue);

impl SshPlugin for JsSsh {
    fn call(&self, method: &'static str, args: Value) -> PluginResult {
        let started = (|| -> Result<Promise, JsValue> {
            let f: Function = Reflect::get(&self.0, &method.into())?.dyn_into()?;
            Ok(Promise::resolve(&f.call1(&self.0, &JSON::parse(&args.to_string())?)?))
        })();
        Box::pin(async move {
            let v = JsFuture::from(started.map_err(plugin_error)?).await.map_err(plugin_error)?;
            Ok(to_json(&v))
        })
    }

    fn listen(&self, on: OnEvent) {
        for event in ["streamLines", "streamExit", "ptyData", "ptyExit", "closed"] {
            let on = on.clone();
            // the plugin keeps it for the life of the page
            let f = Closure::<dyn Fn(JsValue)>::new(move |e: JsValue| on(event, &to_json(&e))).into_js_value();
            let _ = Reflect::get(&self.0, &"addListener".into()).and_then(|a| a.dyn_into::<Function>()).and_then(|a| a.call2(&self.0, &event.into(), &f));
        }
    }
}

fn plugin_error(e: JsValue) -> PluginError {
    PluginError { code: Reflect::get(&e, &"code".into()).ok().and_then(|c| c.as_string()), message: describe(&e) }
}

fn to_json(v: &JsValue) -> Value {
    if v.is_undefined() {
        return Value::Null;
    }
    JSON::stringify(v).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

// ---------- sockets, for the link ----------

/// The page's WebSocket. The handshake starts before it has opened: what's sent waits until it does.
struct WebWire {
    ws: JsWebSocket,
    early: Rc<std::cell::RefCell<Option<Vec<Vec<u8>>>>>,
}

impl Wire for WebWire {
    fn send(&self, data: &[u8]) {
        if let Some(early) = self.early.borrow_mut().as_mut() {
            return early.push(data.to_vec());
        }
        if self.ws.ready_state() == 1 {
            let _ = self.ws.send_bytes(&Uint8Array::from(data));
        }
    }
    fn close(&self, code: u16, reason: &str) {
        if self.ws.close_with(code, reason).is_err() {
            let _ = self.ws.close_plain();
        }
    }
}

struct WebDialer;

impl Dialer for WebDialer {
    fn dial(&self, url: &str, on: OnWire) -> Result<Rc<dyn Wire>, String> {
        let ws = JsWebSocket::new(url).map_err(|e| describe(&e))?;
        ws.set_binary_type("arraybuffer");
        let early: Rc<std::cell::RefCell<Option<Vec<Vec<u8>>>>> = Rc::new(std::cell::RefCell::new((ws.ready_state() == 0).then(Vec::new)));
        let e = early.clone();
        // the handlers take the socket from their event: one they held themselves would keep it,
        // and them, alive after it closed
        ws.set_onopen(
            &Closure::<dyn Fn(JsValue)>::new(move |ev: JsValue| {
                let Ok(w) = Reflect::get(&ev, &"target".into()).map(|t| t.unchecked_into::<JsWebSocket>()) else { return };
                for d in e.borrow_mut().take().unwrap_or_default() {
                    let _ = w.send_bytes(&Uint8Array::from(&d[..]));
                }
            })
            .into_js_value(),
        );
        let o = on.clone();
        ws.set_onmessage(
            &Closure::<dyn Fn(JsValue)>::new(move |m: JsValue| {
                // text frames aren't the link's
                if let Ok(buf) = Reflect::get(&m, &"data".into()).and_then(|d| d.dyn_into::<js_sys::ArrayBuffer>()) {
                    o(WireEvent::Message(Uint8Array::new(&buf).to_vec()));
                }
            })
            .into_js_value(),
        );
        ws.set_onclose(
            &Closure::<dyn Fn(JsValue)>::new(move |c: JsValue| {
                let code = Reflect::get(&c, &"code".into()).ok().and_then(|v| v.as_f64()).unwrap_or(1006.0) as u16;
                let reason = Reflect::get(&c, &"reason".into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
                // closed for good: the handlers let go of the link, so both can be collected
                if let Ok(w) = Reflect::get(&c, &"target".into()) {
                    for h in ["onopen", "onmessage", "onclose"] {
                        let _ = Reflect::set(&w, &h.into(), &JsValue::NULL);
                    }
                }
                on(WireEvent::Close(code, reason));
            })
            .into_js_value(),
        );
        Ok(Rc::new(WebWire { ws, early }))
    }
}

/// A page served over https may open only wss://, unless it's the Android app.
fn plain_ws_allowed() -> bool {
    let g = js_sys::global();
    let protocol = Reflect::get(&g, &"location".into()).and_then(|l| Reflect::get(&l, &"protocol".into())).ok().and_then(|p| p.as_string());
    let native = Reflect::get(&g, &"Capacitor".into())
        .and_then(|c| Reflect::get(&c, &"isNativePlatform".into()).and_then(|f| f.dyn_into::<Function>()).and_then(|f| f.call0(&c)))
        .is_ok_and(|v| v.is_truthy());
    protocol.as_deref() != Some("https:") || native
}

/// What a laptop lists this phone as when the app didn't name it: its model when the WebView says
/// it, without the build ("A142", not "A142 Build/BP2A.250605.031").
fn phone_name() -> String {
    let ua = Reflect::get(&js_sys::global(), &"navigator".into()).and_then(|n| Reflect::get(&n, &"userAgent".into())).ok().and_then(|u| u.as_string()).unwrap_or_default();
    let model =
        inyon_core::re!(r"Android [\d.]+; ([^;)]+)").captures(&ua).and_then(|c| c.get(1)).map(|m| inyon_core::re!(r"\s+Build/.*$").replace(m.as_str(), "").trim().to_owned());
    match model {
        Some(m) if !m.is_empty() && m != "K" && m != "wv" => m,
        _ => "Phone".into(),
    }
}

// ---------- fetch ----------

/// The few requests the core makes to the web (an agent's newest version on npm).
struct WebFetcher;

impl Fetcher for WebFetcher {
    fn fetch(&self, req: FetchRequest) -> LocalBoxFuture<'static, Result<FetchResponse, String>> {
        Box::pin(async move {
            let init = Object::new();
            let headers = Object::new();
            for (k, v) in &req.headers {
                set(&headers, k, &v.into());
            }
            set(&init, "method", &req.method.as_str().into());
            set(&init, "headers", &headers);
            if let Some(b) = &req.body {
                set(&init, "body", &Uint8Array::from(b.as_slice()));
            }
            if req.timeout_ms > 0 {
                set(&init, "signal", &abort_after(req.timeout_ms as f64));
            }
            let res = JsFuture::from(global_fetch(&req.url, &init)).await.map_err(|e| describe(&e))?;
            let status = Reflect::get(&res, &"status".into()).ok().and_then(|s| s.as_f64()).unwrap_or(0.0) as u16;
            let body = JsFuture::from(call0(&res, "arrayBuffer").map_err(|e| describe(&e))?.dyn_into::<Promise>().map_err(|e| describe(&e))?).await.map_err(|e| describe(&e))?;
            Ok(FetchResponse { status, body: Uint8Array::new(&body).to_vec() })
        })
    }
}

// ---------- requests and answers ----------

/// A JavaScript Request as a route handler takes it.
async fn request_in(req: &JsValue) -> Result<Req, JsValue> {
    let method: Method = Reflect::get(req, &"method".into())?.as_string().unwrap_or_default().parse().unwrap_or(Method::GET);
    let href = Reflect::get(req, &"url".into())?.as_string().unwrap_or_default();
    // the app asks for "/api/…" against its own page, so a path alone is fine too
    let url = url::Url::parse(&href).or_else(|_| url::Url::parse("http://phone.invalid")?.join(&href)).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let mut headers = HeaderMap::new();
    if let Some(iter) = js_sys::try_iter(&Reflect::get(req, &"headers".into())?)? {
        for pair in iter {
            let pair = Array::from(&pair?);
            let (Some(k), Some(v)) = (pair.get(0).as_string(), pair.get(1).as_string()) else { continue };
            if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(&v)) {
                headers.append(k, v);
            }
        }
    }
    let body = if method == Method::GET || method == Method::HEAD {
        None
    } else {
        let buf = JsFuture::from(call0(req, "arrayBuffer")?.dyn_into::<Promise>()?).await?;
        Some(Body::from(Uint8Array::new(&buf).to_vec()))
    };
    Ok(Req {
        method,
        path: url.path().to_owned(),
        query: url.query().unwrap_or_default().to_owned(),
        headers,
        peer: ([127, 0, 0, 1], 0).into(),
        https: url.scheme() == "https",
        body,
        params: vec![],
        extensions: http::Extensions::new(),
    })
}

/// An answer as a JavaScript Response. A streamed body (a file being sent) is read whole first.
async fn response_out(res: Resp) -> Result<JsValue, JsValue> {
    let (parts, body) = res.into_parts();
    let headers = Array::new();
    for (k, v) in &parts.headers {
        headers.push(&Array::of2(&k.as_str().into(), &String::from_utf8_lossy(v.as_bytes()).into_owned().into()));
    }
    let init = Object::new();
    set(&init, "status", &parts.status.as_u16().into());
    set(&init, "headers", &headers);
    let status = parts.status.as_u16();
    // these may carry no body at all
    let body = if matches!(status, 101 | 204 | 205 | 304) {
        JsValue::NULL
    } else {
        let bytes = body.bytes().await.map_err(|e| JsValue::from_str(&e.to_string()))?;
        Uint8Array::from(&bytes[..]).into()
    };
    Ok(JsResponse::new(&body, &init).into())
}

// ---------- startPhone ----------

thread_local! {
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

/// `startPhone({ store, ssh, name })`: the core, started once per page. `name` is what paired
/// computers list the phone as.
#[wasm_bindgen(js_name = startPhone)]
pub async fn start_phone(o: JsValue) -> Result<PhoneRuntime, JsValue> {
    // a call that can't start it leaves the page free to try again
    let store = Reflect::get(&o, &"store".into())?;
    if store.is_undefined() {
        return Err(JsValue::from_str("startPhone needs a store"));
    }
    if STARTED.with(|s| s.replace(true)) {
        return Err(JsValue::from_str("The phone core is already running in this page"));
    }
    install(Rc::new(BrowserRuntime));
    inyon_core::platform::console::set_sink(Some(Rc::new(|error, line| if error { console_error(line) } else { console_log(line) })));
    let ssh = Reflect::get(&o, &"ssh".into())?;
    let name = Reflect::get(&o, &"name".into())?.as_string().filter(|n| !n.is_empty()).unwrap_or_else(phone_name);
    let reach = Reach { dialer: Rc::new(WebDialer), resolve: pkarr_resolve(), plain_ws: plain_ws_allowed() };
    // apps/web/src/lib/push.ts listens for it
    let rotate_push = Rc::new(|| {
        let g = js_sys::global();
        let event = Reflect::get(&g, &"Event".into()).and_then(|e| e.dyn_into::<Function>()).and_then(|e| Reflect::construct(&e, &Array::of1(&"inyon:push-rotate".into())));
        let dispatch = Reflect::get(&g, &"dispatchEvent".into()).and_then(|d| d.dyn_into::<Function>());
        if let (Ok(event), Ok(dispatch)) = (event, dispatch) {
            let _ = dispatch.call1(&g, &event);
        }
    });
    let phone = phone::start(Options { store: Rc::new(JsStore(store)), fetcher: Some(Rc::new(WebFetcher)), ssh: Rc::new(JsSsh(ssh)), reach, name, rotate_push });
    Ok(PhoneRuntime(Rc::new(phone)))
}

/// What `startPhone` gives back (PhoneRuntime in types.ts).
#[wasm_bindgen]
pub struct PhoneRuntime(Rc<Phone>);

#[wasm_bindgen]
impl PhoneRuntime {
    /// Answers an /api request the way the hub would: a Request in, a promise of a Response.
    pub fn fetch(&self, req: JsValue) -> Promise {
        let phone = self.0.clone();
        future_to_promise(async move { response_out(phone.answer(request_in(&req).await?).await).await })
    }

    /// A screen of the app connecting: `send` gets each message, already parsed.
    pub fn connect(&self, send: Function) -> PhoneConnection {
        let c = self.0.connect(move |text| {
            if let Ok(msg) = JSON::parse(&text) {
                let _ = send.call1(&JsValue::NULL, &msg);
            }
        });
        PhoneConnection(c)
    }

    /// The app is back in the foreground: check connections now.
    pub fn resume(&self) {
        self.0.resume();
    }

    /// The app went to the background (true) or came back (false).
    pub fn background(&self, hidden: bool) {
        self.0.background(hidden);
    }
}

/// One connected screen (PhoneConnection in types.ts).
#[wasm_bindgen]
pub struct PhoneConnection(ClientRef);

#[wasm_bindgen]
impl PhoneConnection {
    /// What the screen sent, as the text it would have sent the hub's WebSocket.
    pub fn message(&self, raw: &str) {
        clients::client_message(&self.0, raw);
    }

    pub fn close(&self) {
        clients::close_client(&self.0);
    }
}

// ---------- helpers ----------

fn set(o: &Object, k: &str, v: &JsValue) {
    let _ = Reflect::set(o, &k.into(), v);
}

fn call0(o: &JsValue, name: &str) -> Result<JsValue, JsValue> {
    Reflect::get(o, &name.into())?.dyn_into::<Function>()?.call0(o)
}

/// An exception's message, for an error string.
fn describe(e: &JsValue) -> String {
    Reflect::get(e, &"message".into()).ok().and_then(|m| m.as_string()).or_else(|| e.as_string()).unwrap_or_else(|| "failed".into())
}
