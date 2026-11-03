//! Pages on the computers the phone is paired with, with no hub: the list, adding and removing a
//! folder, and every file of a page. The app's viewer asks for a file as `/v/<token>/<path>`, and
//! it is answered here as a hub answers it for a joined machine: the machine's own `inyon pages
//! get` says what is there (over the encrypted link, as every other command), and pages_core's
//! `decide_view` decides, so the rules (dot names, symlinks out of the site, folders and their
//! slash, a path that climbs out) are the hub's, byte for byte. Nothing here leaves the app: the
//! tokens are signed with a key made when the core starts, and the app's viewer renders the page
//! from these answers (apps/web/src/pageview).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use http::{HeaderName, HeaderValue, Method, StatusCode};
use inyon_api::body::Body;
use inyon_api::http::{fields, ok, ok_json, random_bytes, Req, Resp};
use inyon_api::routes::{guarded, h, Routes};
use inyon_core::features::notes;
use inyon_core::features::pages_core::{
    decide_view, gone_after_open, is_absolute, is_slug, mint_token, split_view_target, too_big, verify_token, view_segments, FileStat, VerifyContext, ViewContext, ViewDecision,
    ViewFs, ViewRequest, BAD_SLUG, NOT_ABSOLUTE, TOKEN_TTL_MS,
};
use inyon_core::features::pages_remote::{self, slashed, Listing, RemoteLists, RemotePage, NOT_CONNECTED, UNKNOWN_PAGE};
use inyon_core::hosts::http_error::{http, HubResult};
use inyon_core::js::text::JsStr;
use inyon_core::platform::rt::Deferred;
use serde_json::{json, Value};

/// The biggest file the phone fetches from a computer. The computer closes its link to the phone
/// (4413) once more than 8 MB waits to go out to it, so a file has to stay well under that as base64.
pub const PHONE_FILE_MAX: u64 = 4 * 1024 * 1024;
/// The most bytes of the computers' files the phone keeps while their sites stay the same.
pub const PHONE_CACHE_MAX: usize = 16 * 1024 * 1024;
/// The origin decide_view writes into the CSP of its answers. The viewer doesn't use those headers
/// (its own frame sets the policy), but decide_view refuses a request without one.
const ORIGIN: &str = "http://phone.invalid";

/// What the phone keeps for Pages: the key its tokens are signed with, each page's epoch (Revoke
/// moves it on), and the computers' lists and files.
pub struct PhonePages {
    key: Vec<u8>,
    epochs: RefCell<HashMap<String, u64>>,
    pub remote: RemoteLists,
    now: Rc<dyn Fn() -> u64>,
    turns: Turns,
}

impl PhonePages {
    pub fn new(now: Rc<dyn Fn() -> u64>) -> PhonePages {
        PhonePages { key: random_bytes(32), epochs: RefCell::default(), remote: RemoteLists::with_cache(PHONE_CACHE_MAX), now, turns: Turns::default() }
    }

    fn epoch(&self, host: &str, slug: &str) -> u64 {
        self.epochs.borrow().get(&format!("{host}/{slug}")).copied().unwrap_or(0)
    }

    fn bump(&self, host: &str, slug: &str) {
        *self.epochs.borrow_mut().entry(format!("{host}/{slug}")).or_insert(0) += 1;
    }

    async fn page(&self, host: &str, slug: &str) -> HubResult<RemotePage> {
        if !is_slug(slug) {
            return Err(http(404, UNKNOWN_PAGE));
        }
        self.remote.find(host, slug, (self.now)(), &|_, _| {}).await.ok_or_else(|| http(404, UNKNOWN_PAGE))
    }

    /// Every computer's pages, and the computers whose inyon is too old to list them.
    pub async fn list(&self) -> Value {
        let mut all = vec![];
        for (host, list) in self.remote.all((self.now)(), &|_, _| {}).await {
            all.extend(list.iter().map(|p| entry(&host, p)));
        }
        json!({ "pages": all, "outdated": self.remote.outdated() })
    }

    /// A link to a page for the app's viewer, good for TOKEN_TTL_MS.
    pub async fn open(&self, host: &str, slug: &str) -> HubResult<Value> {
        let p = self.page(host, slug).await?;
        let (token, expires) = mint_token(&self.key, host, &p.record.slug, self.epoch(host, slug), None, (self.now)());
        Ok(json!({ "url": format!("/v/{token}/"), "expires": expires, "ttl": TOKEN_TTL_MS }))
    }

    pub async fn revoke(&self, host: &str, slug: &str) -> HubResult<()> {
        self.page(host, slug).await?;
        self.bump(host, slug);
        Ok(())
    }

    /// Adds a folder on a computer as a page, with its own `inyon pages add` (which checks it by the
    /// hub's rules); the entry as the list will have it.
    pub async fn add(&self, host: &str, b: &serde_json::Map<String, Value>) -> HubResult<Value> {
        if pages_remote::machines().iter().all(|m| m != host) {
            return Err(http(404, NOT_CONNECTED));
        }
        let folder = b.get("path").and_then(Value::as_str).map(|s| s.jtrim().to_owned()).unwrap_or_default();
        if !is_absolute(&folder) {
            return Err(http(400, NOT_ABSOLUTE));
        }
        let slug = match b.get("slug") {
            None => None,
            Some(Value::String(s)) if is_slug(s) => Some(s.as_str()),
            Some(_) => return Err(http(400, BAD_SLUG)),
        };
        let name = match b.get("name") {
            None => None,
            Some(Value::String(n)) => Some(n.jtrim().to16(200).to_owned()),
            Some(_) => return Err(http(400, "The name must be text")),
        };
        let added = pages_remote::add(host, &folder, name.as_deref(), slug).await?;
        self.remote.forget(host);
        Ok(entry(host, &added))
    }

    pub async fn remove(&self, host: &str, slug: &str) -> HubResult<()> {
        self.page(host, slug).await?;
        pages_remote::remove(host, slug).await?;
        self.remote.forget(host);
        // its links end now, and stay dead if the slug comes back
        self.bump(host, slug);
        Ok(())
    }

    /// One `/v/` request, answered as the hub answers one for a page on a joined machine.
    pub async fn view(&self, req: &Req) -> Resp {
        let target = if req.query.is_empty() { req.path.clone() } else { format!("{}?{}", req.path, req.query) };
        let method = req.method.as_str().to_owned();
        let epoch_of = |h: &str, s: &str| self.epoch(h, s);
        let paired = |_: &str| true;
        let verify = VerifyContext { key: &self.key, now: (self.now)(), epoch_of: &epoch_of, phone_paired: &paired };
        let asked = self.listing(&method, &target, &verify).await;
        let site_root = |h: &str, s: &str| asked.as_ref().filter(|a| a.host == h && a.slug == s).map(|a| a.root.clone());
        let fs: &dyn ViewFs = match &asked {
            Some(a) => a.listing.as_ref(),
            None => &Nothing,
        };
        let ctx = ViewContext { verify, site_root: &site_root, fs };
        let inm = req.header("if-none-match");
        let vr = ViewRequest { method: &method, target: &target, origin: Some(ORIGIN), if_none_match: inm.as_deref() };
        let d = decide_view(&vr, &ctx).await;
        let head = req.method == Method::HEAD;
        let (Some(a), Some((path, _))) = (&asked, &d.file) else { return respond(d, head, None) };
        match a.listing.bytes_for(path) {
            Some(md) if md.len() <= notes::NOTE_MAX && notes::wants_note(&req.query, path) => note(d, head, md),
            Some(bytes) => {
                let bytes = bytes.to_vec();
                respond(d, head, Some(bytes))
            }
            None if a.listing.big => respond(too_big(&vr), head, None),
            None => respond(gone_after_open(&vr), head, None),
        }
    }

    /// For a request whose token is good and names a page a computer still lists: what that
    /// computer says about it, asked once. A file it already sent is answered from the phone's copy
    /// while its site's stamp stays the same. One computer sends one file at a time.
    async fn listing(&self, method: &str, target: &str, verify: &VerifyContext<'_>) -> Option<Asked> {
        if method != "GET" && method != "HEAD" {
            return None;
        }
        let t = split_view_target(target)?;
        let claims = verify_token(&t.token, verify).ok()?;
        if t.rest.is_empty() {
            return None;
        }
        let (segments, slash) = view_segments(&t.rest)?;
        let page = self.remote.find(&claims.h, &claims.s, (self.now)(), &|_, _| {}).await?;
        let root = slashed(&page.record.path);
        let key = pages_remote::file_key(&claims.h, &claims.s, &segments, slash);
        let stamp = self.remote.stamp(&claims.h, &claims.s);
        if let Some(kept) = stamp.as_deref().and_then(|st| self.remote.kept(&key, st)) {
            return Some(Asked { host: claims.h, slug: claims.s, root, listing: kept });
        }
        let _turn = self.turns.take(&claims.h).await;
        let listing = if self.remote.watching(&claims.h) {
            let l = Rc::new(pages_remote::get_lines(&claims.h, &claims.s, &root, &segments, slash, PHONE_FILE_MAX).await.unwrap_or_default());
            if let Some(st) = stamp.filter(|_| l.answered() && !l.short) {
                self.remote.keep(key, st, l.clone());
            }
            l
        } else {
            Rc::new(pages_remote::get(&claims.h, &claims.s, &root, &segments, slash, None, PHONE_FILE_MAX).await.unwrap_or_default())
        };
        Some(Asked { host: claims.h, slug: claims.s, root, listing })
    }
}

/// What a computer said about one request, for the page it was asked about.
struct Asked {
    host: String,
    slug: String,
    root: PathBuf,
    listing: Rc<Listing>,
}

/// The disk decide_view sees for a request no computer was asked about: nothing at all.
struct Nothing;

impl ViewFs for Nothing {
    fn stat<'a>(&'a self, _: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<FileStat>> {
        Box::pin(async { None })
    }
    fn realpath<'a>(&'a self, _: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<PathBuf>> {
        Box::pin(async { None })
    }
}

/// One file at a time from each computer: a page asks for all its files at once, and two big ones
/// sent together could fill the computer's queue to the phone and close the link.
#[derive(Default)]
struct Turns {
    waiting: Queues,
}

/// For each computer: whether a request has its turn now, and those waiting for one, in order.
type Queues = Rc<RefCell<HashMap<String, (bool, VecDeque<Rc<Deferred<()>>>)>>>;

/// A computer's turn, given to the next request when dropped.
struct Turn {
    host: String,
    waiting: Queues,
}

impl Turns {
    async fn take(&self, host: &str) -> Turn {
        let wait = {
            let mut w = self.waiting.borrow_mut();
            let (busy, queue) = w.entry(host.to_owned()).or_default();
            if *busy {
                let d = Rc::new(Deferred::new());
                queue.push_back(d.clone());
                Some(d)
            } else {
                *busy = true;
                None
            }
        };
        if let Some(d) = wait {
            d.wait().await;
        }
        Turn { host: host.to_owned(), waiting: self.waiting.clone() }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let next = {
            let mut w = self.waiting.borrow_mut();
            let Some((busy, queue)) = w.get_mut(&self.host) else { return };
            // a request dropped while it waited holds no copy of its own any more: skip it
            let next = std::iter::from_fn(|| queue.pop_front()).find(|d| Rc::strong_count(d) > 1);
            if next.is_none() {
                *busy = false;
                w.remove(&self.host);
            }
            next
        };
        if let Some(d) = next {
            d.resolve(());
        }
    }
}

/// A page on a computer as the app lists it, as the hub lists one on a joined machine.
fn entry(host: &str, p: &RemotePage) -> Value {
    let r = &p.record;
    let mut v = json!({ "host": host, "slug": r.slug, "name": r.name, "project": r.project, "by": r.by, "added": Value::Number(r.added.clone()), "folder": r.path });
    if let Some(progress) = &p.progress {
        v["progress"] = progress.clone();
    }
    if !p.notes.is_empty() {
        v["notes"] = json!(p.notes);
    }
    if let Some(s) = &r.session {
        if let Some(name) = inyon_core::hosts::names::last_name(host, s) {
            v["sessionName"] = json!(name);
        }
        v["session"] = json!(s);
    }
    v
}

fn respond(d: ViewDecision, head: bool, bytes: Option<Vec<u8>>) -> Resp {
    let body = match bytes {
        Some(b) => b,
        None => d.body.into_bytes(),
    };
    let mut res = Resp::new(Body::empty());
    *res.status_mut() = StatusCode::from_u16(d.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (k, v) in &d.headers {
        if let Ok(v) = HeaderValue::from_str(v) {
            res.headers_mut().append(HeaderName::from_static(k), v);
        }
    }
    res.headers_mut().insert(http::header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    if !head && !body.is_empty() {
        *res.body_mut() = Body::from(body);
    }
    res
}

/// A note (`?as=note`) as a page to read, with its own type and tag, as the hub sends it.
fn note(mut d: ViewDecision, head: bool, md: &[u8]) -> Resp {
    let name = d.file.as_ref().and_then(|(p, _)| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for (k, v) in d.headers.iter_mut() {
        match *k {
            "content-type" => *v = "text/html; charset=utf-8".into(),
            "etag" => *v = format!("{}-note\"", v.trim_end_matches('"')),
            _ => {}
        }
    }
    respond(d, head, Some(notes::page(&name, &String::from_utf8_lossy(md)).into_bytes()))
}

/// The phone's Pages routes. `/v/` itself has no route: Phone::answer hands it to `view`.
pub fn add(r: &mut Routes, pages: Rc<PhonePages>) {
    let p = pages.clone();
    guarded(
        r,
        "/api/pages",
        vec![(
            Method::GET,
            h(move |_| {
                let p = p.clone();
                async move { Ok(ok_json(&p.list().await)) }
            }),
        )],
    );
    let p = pages.clone();
    guarded(
        r,
        "/api/pages/:host",
        vec![(
            Method::POST,
            h(move |mut req| {
                let p = p.clone();
                async move {
                    let b = fields(&req.body().await?)?;
                    Ok(ok_json(&p.add(&req.param("host"), &b).await?))
                }
            }),
        )],
    );
    let p = pages.clone();
    guarded(
        r,
        "/api/pages/:host/:slug",
        vec![(
            Method::DELETE,
            h(move |req| {
                let p = p.clone();
                async move {
                    p.remove(&req.param("host"), &req.param("slug")).await?;
                    Ok(ok())
                }
            }),
        )],
    );
    let p = pages.clone();
    guarded(
        r,
        "/api/pages/:host/:slug/open",
        vec![(
            Method::POST,
            h(move |req| {
                let p = p.clone();
                async move { Ok(ok_json(&p.open(&req.param("host"), &req.param("slug")).await?)) }
            }),
        )],
    );
    let p = pages;
    guarded(
        r,
        "/api/pages/:host/:slug/revoke",
        vec![(
            Method::POST,
            h(move |req| {
                let p = p.clone();
                async move {
                    p.revoke(&req.param("host"), &req.param("slug")).await?;
                    Ok(ok())
                }
            }),
        )],
    );
}

#[cfg(test)]
#[path = "pages_tests.rs"]
mod tests;
