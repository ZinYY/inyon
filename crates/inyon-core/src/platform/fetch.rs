//! `fetch`: the few requests the core makes to the web (the npm registry for an agent's newest
//! version). The core does no I/O of its own, so the platform plugs in how a request goes out: the
//! hub through its HTTP client, the phone app through the browser's `fetch`. Without one, every
//! request fails as an unreachable network would, which the callers already handle.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;

/// What came back: the status and the whole body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl FetchResponse {
    /// `res.ok`
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// One request: method, URL, headers and body, given up after `timeout_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: u64,
}

impl FetchRequest {
    pub fn get(url: impl Into<String>, timeout_ms: u64) -> Self {
        FetchRequest { method: "GET".into(), url: url.into(), timeout_ms, ..Default::default() }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

pub trait Fetcher {
    /// Fails (with a message) where the request couldn't be made or timed out; any status is an answer.
    fn fetch(&self, req: FetchRequest) -> LocalBoxFuture<'static, Result<FetchResponse, String>>;
}

thread_local! {
    static FETCHER: RefCell<Option<Rc<dyn Fetcher>>> = const { RefCell::new(None) };
}

/// Plugs in how this thread's requests go out (`None`: none go out).
pub fn use_fetcher(f: Option<Rc<dyn Fetcher>>) {
    FETCHER.with(|c| *c.borrow_mut() = f);
}

/// `await fetch(...)`
pub async fn fetch(req: FetchRequest) -> Result<FetchResponse, String> {
    let f = FETCHER.with(|c| c.borrow().clone());
    match f {
        Some(f) => f.fetch(req).await,
        None => Err("fetch failed: no network on this platform".into()),
    }
}
