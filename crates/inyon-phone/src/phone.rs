//! Port of the TypeScript phone core (removed in 0.23): the phone app runs Inyon's core itself, with no hub and no
//! password. The web app hands it a store and calls it in-process: `answer` for what would be an
//! /api request, `connect` for what would be the hub's WebSocket.
//!
//! Servers are reached over SSH through the native plugin (`ssh`), computers paired with
//! `inyon pair` over the encrypted link, and servers can be reached through one of those. The
//! computers send the phone's notifications, and update from the website's newest release.

use std::rc::Rc;

use inyon_api::clients::{self, ClientRef};
use inyon_api::http::{json, ok, ok_json, Req, Resp};
use inyon_api::routes::{raw, Route, Routes};
use inyon_core::claude::{drafts, watcher::watchers};
use inyon_core::hosts::host::{set_host_backends, HostBackend};
use inyon_core::hosts::node_backend::node_backend;
use inyon_core::hosts::node_update::{latest_release_from, node_updates};
use inyon_core::hosts::registry::{registry, RegistryEvent};
use inyon_core::hosts::via_backend::via_backend;
use inyon_core::platform::fetch::{fetch, use_fetcher, FetchRequest, Fetcher};
use inyon_core::platform::store::{use_store, Store};
use serde_json::json;

use inyon_protocol::{HostKind, LATEST_URL};

use crate::keys::PhoneKeys;
use crate::link::{Links, Reach};
use crate::pages::PhonePages;
use crate::ssh::{CredentialsFor, SshBackend, SshPlugin};
use crate::tables::{find, Lookup};

/// What the platform gives the core; its runtime has to be installed on this thread first.
pub struct Options {
    pub store: Rc<dyn Store>,
    pub fetcher: Option<Rc<dyn Fetcher>>,
    pub ssh: Rc<dyn SshPlugin>,
    /// how the link reaches paired computers
    pub reach: Reach,
    /// what paired computers list this phone as
    pub name: String,
    /// a computer that had this browser's push keys was removed: the page makes new ones
    pub rotate_push: Rc<dyn Fn()>,
}

pub struct Phone {
    tables: Rc<[Routes; 2]>,
    links: Links,
    pages: Rc<PhonePages>,
}

/// Starts the core: the records come from `store`, then the machines are watched and the
/// housekeeping timers run, as `startPhone` does.
pub fn start(o: Options) -> Phone {
    // records some modules read as they load, so the store goes in before anything else
    use_store(o.store);
    // the phone is no machine of its own: every machine on its list is reached from it
    inyon_core::config::set_local_host(None);
    use_fetcher(o.fetcher);
    let keys = PhoneKeys::new(o.ssh.clone());
    let k = keys.clone();
    let credentials: CredentialsFor = Rc::new(move |config| {
        let (keys, config) = (k.clone(), config.clone());
        Box::pin(async move { keys.credentials(&config).await })
    });
    let conn = SshBackend::new(o.ssh.clone(), credentials);
    // laptops paired with `inyon pair` take the same commands a hub sends to inyon, and a server
    // reached through one of them goes as `ssh` commands run there
    let (node, via, ssh): (Rc<dyn HostBackend>, Rc<dyn HostBackend>, Rc<dyn HostBackend>) =
        (node_backend(), via_backend(|id| registry().get(id).map(|rt| rt.borrow().host.config())), Rc::new(conn.clone()));
    set_host_backends(move |c| match c.kind {
        HostKind::Link => node.clone(),
        _ if c.via.as_deref().is_some_and(|v| !v.is_empty()) => via.clone(),
        _ => ssh.clone(),
    });
    let k = keys.clone();
    let noise: crate::link::Noise = Rc::new(move || {
        let keys = k.clone();
        Box::pin(async move { Ok(keys.own().await?.noise) })
    });
    let save = Rc::new(|id: &str, urls: Vec<String>, relays: Vec<String>| {
        let _ = registry().note(id, |c| {
            c.urls = Some(urls);
            c.relays = Some(relays);
        });
    });
    let links = Links::new(o.reach.clone(), noise, o.name, save);
    let ensure = {
        let links = links.clone();
        move || {
            for c in registry().configs().into_iter().filter(|c| c.kind == HostKind::Link) {
                links.ensure(&c);
            }
        }
    };
    let on_change = ensure.clone();
    let _ = registry().on(move |e| {
        if matches!(e, RegistryEvent::Overview) {
            on_change();
        }
    });
    let mut core = Routes::default();
    inyon_api::core_routes::add(&mut core);
    let mut own = phone_routes();
    crate::servers::add(&mut own, o.ssh, keys.clone(), conn, links.clone(), o.rotate_push);
    crate::computers::add(&mut own, o.reach, links.clone());
    // notifications while the page is closed come from the computers
    crate::push::add(&mut own);
    // pages on the computers, which the app's viewer opens through `/v/` here
    let pages = Rc::new(PhonePages::new(Rc::new(inyon_core::clock::now_ms)));
    crate::pages::add(&mut own, pages.clone());
    inyon_api::core_routes::start_core();
    ensure();
    crate::push::start_node_push(Rc::new(move || {
        let keys = keys.clone();
        Box::pin(async move { Ok(keys.own().await?.noise.public) })
    }));
    // laptops paired with the phone update from the website's copy of the newest release; its
    // latest.json answers the WebView (CORS)
    node_updates().watch_releases(
        || {
            Box::pin(async {
                let r = fetch(FetchRequest::get(LATEST_URL, 15_000)).await?;
                latest_release_from(r.status, &r.text())
            })
        },
        6 * 3_600_000,
    );
    Phone { tables: Rc::new([own, core]), links, pages }
}

/// The routes the phone answers itself, ahead of the core's.
fn phone_routes() -> Routes {
    let mut r = Routes::default();
    // nothing to sign in to: the phone is the hub
    r.add("/api/session", Route::Any(raw(|_| async { ok_json(&json!({ "authed": true, "phone": true })) })));
    r.add("/api/logout", Route::Any(raw(|_| async { ok() })));
    r
}

impl Phone {
    /// Answers an /api request the way the hub would.
    pub async fn answer(&self, mut req: Req) -> Resp {
        // a file of a page on a computer, which the app's page viewer asks for as a hub's would be
        if req.path.starts_with("/v/") {
            return self.pages.view(&req).await;
        }
        match find(&self.tables[..], &req.path, &req.method) {
            Lookup::Handler(h, params) => {
                req.params = params;
                h(req).await
            }
            Lookup::NotAllowed => json(405, &json!({ "error": "Method not allowed" })),
            Lookup::NotFound => json(404, &json!({ "error": "Not found" })),
        }
    }

    /// A screen of the app connecting: it hears what it subscribes to through `send`, as JSON text.
    pub fn connect(&self, send: impl Fn(String) + 'static) -> ClientRef {
        let c = clients::new_client(move |msg| send(clients::wire(msg)));
        clients::add(&c);
        c
    }

    /// A screen on protocol v2 (the hub's `/ws2`): it follows streams, and hears them through
    /// `out`, as text or (once it asked for that) compressed.
    pub fn connect2(&self, out: impl Fn(inyon_api::streams::Wire) + 'static) -> ClientRef {
        let c = clients::new_client2(out);
        clients::add(&c);
        c
    }

    /// The app went to the background (true) or came back (false): poll less while hidden.
    pub fn background(&self, hidden: bool) {
        registry().quiet(hidden);
        watchers().quiet(hidden);
        registry().hold_alerts(hidden);
        // Android may close the app from the background: keep what each open chat has read,
        // drafts and how each machine looked
        drafts::save_drafts_at_once(hidden);
        if hidden {
            inyon_api::core_routes::save_everything();
        }
    }

    /// The app is back in the foreground: check connections now, don't wait out a backoff.
    pub fn resume(&self) {
        registry().quiet(false);
        watchers().quiet(false);
        registry().hold_alerts(false);
        self.links.resume();
        // connections that died in the background fail their next command; probe now, not at the
        // next poll
        for c in registry().configs() {
            registry().refresh(&c.id, 0);
        }
    }
}

#[cfg(test)]
impl Phone {
    /// Reads a computer's list of pages again next time.
    pub fn pages_forget(&self, id: &str) {
        self.pages.remote.forget(id);
    }
}

#[cfg(test)]
#[path = "phone_tests.rs"]
pub(crate) mod tests;
