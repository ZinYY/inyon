//! What a route handler gets and gives back, and the helpers core.ts has for them: JSON answers,
//! request bodies counted against a limit, query values, and the TS's loose reading of a JSON body
//! (`String(b.name ?? '')`, `Number(b.port)`, `!!b.on`).

use std::net::SocketAddr;

use http::{header, Extensions, HeaderMap, HeaderValue, Method, Response, StatusCode};
use inyon_core::hosts::http_error::{http, HubError, HubResult};
use inyon_core::platform::console;
use serde_json::{Map, Value};

pub use crate::body::Body;

pub type Resp = Response<Body>;

/// One request as a handler sees it: Bun's `Request` plus what `server.requestIP` and the route's
/// `params` add.
pub struct Req {
    pub method: Method,
    /// the URL's path, still percent-encoded, with dot segments resolved (`new URL(req.url).pathname`)
    pub path: String,
    /// the query string without its `?`
    pub query: String,
    pub headers: HeaderMap,
    pub peer: SocketAddr,
    /// whether the hub serves HTTPS itself (`new URL(req.url).protocol === 'https:'`)
    pub https: bool,
    pub body: Option<Body>,
    pub params: Vec<(String, String)>,
    /// what the server carrying the request adds (the hub's connection, for a WebSocket upgrade)
    pub extensions: Extensions,
}

impl Req {
    /// `req.headers.get(name)`: several of one header are joined as Bun joins them.
    pub fn header(&self, name: &str) -> Option<String> {
        let all: Vec<String> = self.headers.get_all(name).iter().map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).collect();
        if all.is_empty() {
            return None;
        }
        Some(all.join(if name.eq_ignore_ascii_case("cookie") { "; " } else { ", " }))
    }

    /// A route parameter (`req.params.id`).
    pub fn param(&self, name: &str) -> String {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    /// `new URL(req.url).searchParams.get(name)`
    pub fn query_get(&self, name: &str) -> Option<String> {
        url::form_urlencoded::parse(self.query.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
    }

    /// `q(req, name)`: the query value or ''.
    pub fn q(&self, name: &str) -> String {
        self.query_get(name).unwrap_or_default()
    }

    /// The agent catalog the client knows (see `inyon_protocol::compat`): its `x-inyon-catalog`
    /// header or, where it can't set one (a WebSocket, a page on another origin), `?catalog=`.
    pub fn catalog(&self) -> u32 {
        inyon_protocol::compat::catalog_of(self.header("x-inyon-catalog").or_else(|| self.query_get("catalog")).as_deref())
    }

    /// `searchParams.has(name)`
    pub fn query_has(&self, name: &str) -> bool {
        url::form_urlencoded::parse(self.query.as_bytes()).any(|(k, _)| k == name)
    }

    /// The peer's address as `server.requestIP(req).address` gives it.
    pub fn ip(&self) -> String {
        self.peer.ip().to_string()
    }

    /// `req.url` as Bun builds it: the scheme, the Host header and the path.
    pub fn url(&self) -> String {
        let host = self.header("host").unwrap_or_default();
        let q = if self.query.is_empty() { String::new() } else { format!("?{}", self.query) };
        format!("{}://{host}{}{q}", if self.https { "https" } else { "http" }, self.path)
    }

    /// `new URL(req.url).origin`
    pub fn origin(&self) -> String {
        url::Url::parse(&self.url()).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".into())
    }

    /// The body, read whole but refused with 413 once it passes `limit` bytes (`limitedBody`).
    /// Content-Length is checked first; a chunked body has none, so the bytes are counted as well.
    pub async fn read_limited(&mut self, limit: usize, message: &str) -> HubResult<Vec<u8>> {
        let declared: f64 = self.header("content-length").map(|v| inyon_core::js::to_number(&v)).unwrap_or(0.0);
        let mut body = self.body.take().unwrap_or_else(Body::empty);
        if declared > limit as f64 {
            drain(body).await;
            return Err(http(413, message));
        }
        let mut out = Vec::new();
        while let Some(data) = body.next_chunk().await {
            let data = data.map_err(|e| HubError::plain(e.to_string()))?;
            out.extend_from_slice(&data);
            if out.len() > limit {
                drain(body).await;
                return Err(http(413, message));
            }
        }
        Ok(out)
    }

    /// `await body(req, limit)`: the JSON body, 413 past the limit, 400 for anything else wrong.
    pub async fn json_body(&mut self, limit: usize) -> HubResult<Value> {
        let bytes = match self.read_limited(limit, "Request too large").await {
            Ok(b) => b,
            Err(e) if e.status == Some(413) => return Err(e),
            Err(_) => return Err(http(400, "Invalid JSON")),
        };
        // Response.text() decodes as UTF-8 and drops a byte order mark
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        inyon_core::js::json::parse(text).ok_or_else(|| http(400, "Invalid JSON"))
    }

    /// `await body(req)` with the default 1 MB limit.
    pub async fn body(&mut self) -> HubResult<Value> {
        self.json_body(1024 * 1024).await
    }
}

/// What a client sent past a limit is read and dropped, up to a point, so the answer reaches it
/// instead of a reset connection while it's still sending.
async fn drain(mut body: Body) {
    let mut seen = 0usize;
    let read = async {
        while let Some(Ok(d)) = body.next_chunk().await {
            seen += d.len();
            if seen > 16 << 20 {
                break;
            }
        }
    };
    futures_util::future::select(Box::pin(read), inyon_core::platform::rt::sleep(5_000)).await;
}

// ---------- answers ----------

/// `Response.json(data, { status })`
pub fn json(status: u16, data: &Value) -> Resp {
    let mut r = Response::new(Body::from(inyon_core::js::stringify(data)));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json;charset=utf-8"));
    r
}

/// `json(data)` with status 200
pub fn ok_json(data: &Value) -> Resp {
    json(200, data)
}

/// `ok()`: `{ ok: true }`
pub fn ok() -> Resp {
    json(200, &serde_json::json!({ "ok": true }))
}

/// A serializable value as JSON.
pub fn json_of<T: serde::Serialize>(data: &T) -> Resp {
    match serde_json::to_value(data) {
        Ok(v) => json(200, &v),
        Err(e) => error_json(&HubError::plain(e.to_string())),
    }
}

/// `new Response(text, { status })`: Bun's default text type.
pub fn text(status: u16, body: impl Into<String>) -> Resp {
    let mut r = Response::new(Body::from(body.into()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain;charset=utf-8"));
    r
}

/// An error as the app gets it: `{ error }` with its status, 500 (and a log line) for one without.
pub fn error_json(e: &HubError) -> Resp {
    if e.status.is_none() {
        console::error(&format!("error: {}", e.message));
    }
    json(e.status.unwrap_or(500), &serde_json::json!({ "error": e.message }))
}

/// `api(fn)`: errors become JSON answers with the right status.
pub fn answer(r: HubResult<Resp>) -> Resp {
    r.unwrap_or_else(|e| error_json(&e))
}

pub fn set_header(r: &mut Resp, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        r.headers_mut().insert(name, v);
    }
}

pub fn append_header(r: &mut Resp, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        r.headers_mut().append(name, v);
    }
}

// ---------- reading a JSON body the way the TS does ----------

/// `b.<key>` on a parsed body. A body that isn't an object has no fields (the TS reads
/// `undefined`); `null` throws there, as reading a field of null does.
pub fn field<'a>(b: &'a Value, key: &str) -> HubResult<Option<&'a Value>> {
    match b {
        Value::Null => Err(HubError::plain(format!("null is not an object (evaluating 'b.{key}')"))),
        Value::Object(m) => Ok(m.get(key)),
        _ => Ok(None),
    }
}

/// The body as an object, for handlers that read several fields (`null` throws, as above).
pub fn fields(b: &Value) -> HubResult<Map<String, Value>> {
    match b {
        Value::Null => Err(HubError::plain("null is not an object (evaluating 'b')")),
        Value::Object(m) => Ok(m.clone()),
        _ => Ok(Map::new()),
    }
}

/// `String(v)` for a value that is there (`undefined` is the caller's to default).
pub fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => inyon_core::js::fmt_number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|x| if x.is_null() { String::new() } else { js_string(x) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `String(b.x ?? '')`
pub fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => js_string(v),
    }
}

/// `String(b.x)`: undefined becomes "undefined", as the TS gets it.
pub fn str_raw(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(v) => js_string(v),
    }
}

/// `Number(v)`
pub fn js_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => inyon_core::js::to_number(s),
        Some(Value::Array(a)) if a.is_empty() => 0.0,
        Some(Value::Array(a)) if a.len() == 1 => js_number(Some(&Value::String(js_string(&a[0])))),
        Some(_) => f64::NAN,
    }
}

/// `x | 0`
pub fn to_int32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    let t = x.trunc() % 4_294_967_296.0;
    let t = if t < 0.0 { t + 4_294_967_296.0 } else { t };
    (t as u64 as u32) as i32
}

/// `!!v`
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// `typeof v === 'string' ? v : undefined`
pub fn as_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

/// URL-safe pane ids: "%12" travels as "12" (`paneParam`).
pub fn pane_param(p: &str) -> String {
    if p.starts_with('%') {
        p.to_owned()
    } else {
        format!("%{p}")
    }
}

/// `decodeURIComponent`, or the text as it was when it isn't valid.
pub fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let h = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Random bytes from the system.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("random bytes from the system");
    b
}

/// `crypto.randomBytes(n).toString('base64url')`
pub fn random_base64url(n: usize) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(n))
}

/// `crypto.randomBytes(n).toString('hex')`
pub fn random_hex(n: usize) -> String {
    hex(&random_bytes(n))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `crypto.timingSafeEqual` on two strings of the same length (false when they differ in length).
pub fn same(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// sha256 hex of a text
pub fn sha256(s: &str) -> String {
    inyon_core::sha::sha256_hex(s)
}

/// A request for tests: method, path with query, headers, body.
pub fn test_req(method: &str, path_q: &str, headers: &[(&str, &str)], body: &str) -> Req {
    let (path, query) = path_q.split_once('?').unwrap_or((path_q, ""));
    let mut h = HeaderMap::new();
    for (k, v) in headers {
        h.append(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
    }
    Req {
        method: method.parse().unwrap(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers: h,
        peer: "127.0.0.1:50000".parse().unwrap(),
        https: false,
        body: Some(Body::from(body.to_owned())),
        params: vec![],
        extensions: Extensions::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // core.test.ts

    /// A body sent in chunks, so the request has no Content-Length to check.
    fn chunked(chunks: usize, size: usize) -> (Req, Arc<AtomicUsize>) {
        let pulled = Arc::new(AtomicUsize::new(0));
        let n = pulled.clone();
        let stream = futures_util::stream::unfold((), move |()| {
            let sent = n.fetch_add(1, Ordering::SeqCst) + 1;
            let chunk = if sent > chunks {
                None
            } else if sent == 1 {
                Some(b"{\"x\":\"".to_vec())
            } else {
                Some(vec![b'a'; size])
            };
            async move { chunk.map(|c| (Ok::<_, std::io::Error>(bytes::Bytes::from(c)), ())) }
        });
        let mut r = test_req("POST", "/api/login", &[], "");
        r.body = Some(Body::from_stream(stream));
        (r, pulled)
    }

    #[test]
    fn a_chunked_body_is_cut_off_once_it_passes_the_limit() {
        let (mut r, pulled) = chunked(10_000, 1024);
        assert!(r.header("content-length").is_none());
        let e = crate::test_util::block(r.json_body(8 * 1024)).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(413), "Request too large"));
        // it kept nothing past the limit; the rest is read and dropped so the 413 reaches the
        // client (the Bun hub stopped reading at once instead): under 16 MB, all of it, to the end
        assert_eq!(pulled.load(Ordering::SeqCst), 10_001);
    }

    #[test]
    fn a_chunked_body_past_16_mb_stops_being_read() {
        let (mut r, pulled) = chunked(20_000, 1024);
        let e = crate::test_util::block(r.json_body(8 * 1024)).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(413), "Request too large"));
        let n = pulled.load(Ordering::SeqCst);
        assert!((16_384..16_500).contains(&n), "{n}");
    }

    #[test]
    fn small_bodies_still_parse_and_bad_ones_say_so() {
        crate::test_util::block(async {
            let mut ok = test_req("POST", "/", &[], r#"{"password":"pw"}"#);
            assert_eq!(ok.json_body(8 * 1024).await.unwrap(), serde_json::json!({ "password": "pw" }));
            let mut bad = test_req("POST", "/", &[], "{nope");
            assert_eq!(bad.body().await.unwrap_err().message, "Invalid JSON");
            let big = "x".repeat(9000);
            let mut long = test_req("POST", "/", &[("content-length", "9000")], &big);
            let e = long.json_body(8 * 1024).await.unwrap_err();
            assert_eq!((e.status, e.message.as_str()), (Some(413), "Request too large"));
        });
    }

    #[test]
    fn js_coercions_read_a_body_as_the_ts_does() {
        assert_eq!(str_or_empty(None), "");
        assert_eq!(str_or_empty(Some(&Value::Null)), "");
        assert_eq!(str_or_empty(Some(&serde_json::json!(12.5))), "12.5");
        assert_eq!(str_raw(None), "undefined");
        assert_eq!(js_string(&serde_json::json!([1, null, "a"])), "1,,a");
        assert_eq!(js_number(Some(&serde_json::json!("42"))), 42.0);
        assert!(js_number(None).is_nan());
        assert_eq!(to_int32(9999.9), 9999);
        assert_eq!(to_int32(-5.5), -5);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert!(truthy(Some(&serde_json::json!("x"))) && !truthy(Some(&serde_json::json!(0))));
    }

    #[test]
    fn pane_ids_travel_without_their_percent_sign() {
        assert_eq!(pane_param("12"), "%12");
        assert_eq!(pane_param("%3"), "%3");
    }

    #[test]
    fn uri_components_decode_or_fail_like_decode_uri_component() {
        assert_eq!(decode_uri_component("a%20b").as_deref(), Some("a b"));
        assert_eq!(decode_uri_component("%E2%82%AC").as_deref(), Some("€"));
        assert_eq!(decode_uri_component("%zz"), None);
        assert_eq!(decode_uri_component("%C3"), None);
    }
}
