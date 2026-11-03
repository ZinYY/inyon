//! Port of push/routes.ts and push/hub-push.ts. A hub is always on, so it sends the browsers' push
//! notifications itself: every notice the open app would show, a machine going offline included,
//! reaches a closed tab or installed app too. The registrations are inyon-api's (the phone app
//! keeps them the same way); the encryption and signing are inyon-engine's (RFC 8291 and VAPID),
//! shared with the engine's own pushes.

use axum::http::Method;
use futures_util::future::LocalBoxFuture;
pub use inyon_api::push::{add_registration, registrations, remove_registration, valid_registration, Registration, MAX_REGISTRATIONS};
use inyon_core::hosts::http_error::{http, HubResult};
use inyon_core::platform::console;
use inyon_engine::webpush::{self, send_push, Sent};
use inyon_protocol::ServerMsg;
use serde_json::{json, Value};

use crate::http::{ok, ok_json, Req};
use crate::server::{guarded, h, Routes};

/// The same tag and link the open app uses (apps/web/src/lib/store.ts), so one replaces the other.
pub fn payload_for(host: &str, pane: &str, kind: &str, title: &str, body: &str) -> Value {
    let machine = pane.is_empty();
    json!({
        "title": title,
        "body": body,
        "kind": kind,
        "tag": if machine { format!("machine:{host}") } else { format!("{host}:{pane}") },
        "href": if machine {
            format!("/machines/{}", inyon_core::js::text::encode_uri_component(host))
        } else {
            format!("/m/{}/p/{}", inyon_core::js::text::encode_uri_component(host), pane.strip_prefix('%').unwrap_or(pane))
        },
    })
}

/// A page's board moving on: one notice per page, the next replacing it, opening the page.
pub fn page_payload(host: &str, slug: &str, title: &str, body: &str) -> Value {
    json!({
        "title": title,
        "body": body,
        "kind": "page",
        "tag": format!("page:{host}/{slug}"),
        "href": format!("/pages/{}/{slug}", inyon_core::js::text::encode_uri_component(host)),
    })
}

pub fn test_payload() -> Value {
    json!({ "title": "Inyon", "body": "Notifications work, even with Inyon closed.", "tag": "test", "href": "/settings#notifications", "kind": "test" })
}

pub type Sender = std::rc::Rc<dyn Fn(Registration, Value) -> LocalBoxFuture<'static, Result<Sent, String>>>;

fn web_push() -> Sender {
    std::rc::Rc::new(|r: Registration, p: Value| {
        Box::pin(async move {
            let s = &r.subscription;
            let subscription = webpush::Subscription { endpoint: s.endpoint.clone(), keys: webpush::Keys { p256dh: s.keys.p256dh.clone(), auth: s.keys.auth.clone() } };
            let j = &r.vapid.private_jwk;
            let vapid = webpush::Vapid {
                private_jwk: webpush::EcJwk { kty: j.kty.clone(), crv: j.crv.clone(), d: j.d.clone(), x: j.x.clone(), y: j.y.clone() },
                subject: r.vapid.subject.clone(),
            };
            send_push(&subscription, &p, &vapid).await
        })
    })
}

/// Sends one notice to every browser that wants it; drops the ones their push service says are gone.
pub async fn push_all(p: Value, send: Option<Sender>, only: Option<&str>) -> usize {
    let send = send.unwrap_or_else(web_push);
    let kind = p.get("kind").and_then(Value::as_str);
    let wants = |r: &Registration| match kind {
        Some("done") => r.done,
        Some("page") => r.pages,
        _ => true,
    };
    let to: Vec<Registration> = registrations().into_iter().filter(|r| wants(r) && only.is_none_or(|o| r.subscription.endpoint == o)).collect();
    let sends = to.into_iter().map(|r| {
        let (send, p) = (send.clone(), p.clone());
        async move {
            let endpoint = r.subscription.endpoint.clone();
            match send(r, p).await {
                Ok(res) if res.ok => 1,
                Ok(res) if res.gone => {
                    remove_registration(&endpoint);
                    0
                }
                Ok(res) => {
                    let host = url::Url::parse(&endpoint).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
                    console::error(&format!("push: {host} answered {}", res.status));
                    0
                }
                Err(e) => {
                    console::error(&format!("push: {e}"));
                    0
                }
            }
        }
    });
    futures_util::future::join_all(sends).await.into_iter().sum()
}

/// Every notice the open app gets goes to the browsers too.
pub fn start_hub_push() {
    crate::clients::on_notice(|n| {
        let p = match n {
            ServerMsg::Notify { host, pane, kind, title, body, .. } => {
                let kind = serde_json::to_value(kind).ok().and_then(|k| k.as_str().map(str::to_owned)).unwrap_or_default();
                payload_for(host, pane, &kind, title, body)
            }
            ServerMsg::PageNotify { host, slug, title, body } => page_payload(host, slug, title, body),
            _ => return,
        };
        inyon_core::platform::rt::spawn(async move {
            push_all(p, None, None).await;
        });
    });
}

fn endpoint_of(b: &Value) -> HubResult<String> {
    crate::http::field(b, "endpoint")?.and_then(Value::as_str).map(str::to_owned).ok_or_else(|| http(400, "Which endpoint?"))
}

/// The browser's side: POST its registration when notifications are allowed (and again when the
/// "finished" setting changes), DELETE it when they're turned off, and ask for a test that goes the
/// whole way through its push service.
pub fn add(routes: &mut Routes) {
    guarded(
        routes,
        "/api/push",
        vec![
            (
                Method::POST,
                h(|mut r: Req| async move {
                    let reg = valid_registration(&r.json_body(16 * 1024).await?)?;
                    add_registration(&reg);
                    Ok(ok())
                }),
            ),
            (
                Method::DELETE,
                h(|mut r: Req| async move {
                    let endpoint = endpoint_of(&r.json_body(16 * 1024).await?)?;
                    remove_registration(&endpoint);
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/push/test",
        vec![(
            Method::POST,
            h(|mut r: Req| async move {
                let endpoint = endpoint_of(&r.json_body(16 * 1024).await?)?;
                let sent = push_all(test_payload(), None, Some(&endpoint)).await;
                Ok(ok_json(&json!({ "sent": sent })))
            }),
        )],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn browser(n: u32, done: bool) -> Value {
        json!({
            "subscription": { "endpoint": format!("https://push.example.net/{n}"), "keys": { "p256dh": "p", "auth": "a" } },
            "vapid": { "privateJwk": { "kty": "EC", "crv": "P-256", "d": "d", "x": "x", "y": "y" }, "subject": "https://example.com/" },
            "done": done,
        })
    }

    #[test]
    fn a_registration_keeps_only_what_it_needs_and_refuses_the_wrong_shape() {
        crate::test_util::core();
        let mut b = browser(1, true);
        b["vapid"]["privateJwk"]["extra"] = json!(1);
        b["vapid"]["subject"] = json!("mailto:me@example.com");
        b["extra"] = json!(true);
        let r = valid_registration(&b).unwrap();
        assert_eq!(serde_json::to_value(&r.vapid.private_jwk).unwrap(), json!({ "kty": "EC", "crv": "P-256", "d": "d", "x": "x", "y": "y" }));
        assert!(r.done);
        let mut b = browser(1, true);
        b["subscription"]["endpoint"] = json!("http://push.example.net/1");
        assert!(valid_registration(&b).unwrap_err().message.contains("endpoint"));
        let mut b = browser(1, true);
        b["vapid"]["privateJwk"]["crv"] = json!("P-384");
        assert!(valid_registration(&b).unwrap_err().message.contains("VAPID key"));
        let mut b = browser(1, true);
        b["vapid"]["subject"] = json!("example.com");
        assert!(valid_registration(&b).unwrap_err().message.contains("subject"));
    }

    #[test]
    fn a_sessions_notice_and_a_machines_share_the_open_apps_tags_and_links() {
        crate::test_util::core();
        assert_eq!(
            payload_for("laptop", "%12", "waiting", "Fix login", "Allow Bash?"),
            json!({ "title": "Fix login", "body": "Allow Bash?", "kind": "waiting", "tag": "laptop:%12", "href": "/m/laptop/p/12" })
        );
        let m = payload_for("my box", "", "offline", "my box is offline", "");
        assert_eq!((m["tag"].as_str(), m["href"].as_str()), (Some("machine:my box"), Some("/machines/my%20box")));
    }

    #[test]
    fn a_hub_sends_to_every_browser_skips_finished_notices_for_those_that_dont_want_them_drops_gone_ones() {
        crate::test_util::core();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            inyon_core::platform::rt::install(Rc::new(inyon_backends::runtime::TokioRuntime));
            for (n, done) in [(1, true), (2, false), (3, true)] {
                add_registration(&valid_registration(&browser(n, done)).unwrap());
            }
            let to = Rc::new(RefCell::new(Vec::<String>::new()));
            let seen = to.clone();
            let send: Sender = Rc::new(move |r: Registration, _p: Value| {
                let e = r.subscription.endpoint.clone();
                seen.borrow_mut().push(e[e.len() - 1..].to_owned());
                let gone = e.ends_with("/3");
                Box::pin(async move { Ok(Sent { ok: !gone, status: if gone { 410 } else { 201 }, gone }) })
            });
            assert_eq!(push_all(payload_for("h", "%1", "done", "t", "b"), Some(send.clone()), None).await, 1);
            let mut got = to.borrow().clone();
            got.sort();
            assert_eq!(got, ["1", "3"]);
            let mut left: Vec<String> = registrations().iter().map(|r| r.subscription.endpoint[r.subscription.endpoint.len() - 1..].to_owned()).collect();
            left.sort();
            assert_eq!(left, ["1", "2"]);
            to.borrow_mut().clear();
            assert_eq!(push_all(payload_for("h", "%1", "waiting", "t", "b"), Some(send), Some("https://push.example.net/2")).await, 1);
            assert_eq!(*to.borrow(), ["2"]);
        });
    }

    // a page's board moving on goes to the browsers that want it, which one registered before
    // 0.22.4 does
    #[test]
    fn a_pages_notice_opens_the_page_and_skips_browsers_that_turned_page_notices_off() {
        crate::test_util::core();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            inyon_core::platform::rt::install(Rc::new(inyon_backends::runtime::TokioRuntime));
            let p = page_payload("my box", "robot", "Robot plan", "3 of 12 done");
            assert_eq!((p["kind"].as_str(), p["tag"].as_str(), p["href"].as_str()), (Some("page"), Some("page:my box/robot"), Some("/pages/my%20box/robot")));
            let mut off = browser(1, true);
            off["pages"] = json!(false);
            add_registration(&valid_registration(&off).unwrap());
            let mut old = serde_json::to_value(valid_registration(&browser(2, true)).unwrap()).unwrap();
            old.as_object_mut().unwrap().remove("pages");
            let old: Registration = serde_json::from_value(old).unwrap();
            assert!(old.pages);
            add_registration(&old);
            let to = Rc::new(RefCell::new(Vec::<String>::new()));
            let seen = to.clone();
            let send: Sender = Rc::new(move |r: Registration, _p: Value| {
                seen.borrow_mut().push(r.subscription.endpoint.clone());
                Box::pin(async move { Ok(Sent { ok: true, status: 201, gone: false }) })
            });
            assert_eq!(push_all(p, Some(send), None).await, 1);
            assert_eq!(*to.borrow(), ["https://push.example.net/2"]);
        });
    }

    #[test]
    fn a_hub_sends_to_at_most_twenty_browsers_dropping_the_one_that_registered_longest_ago() {
        crate::test_util::core();
        let add = |n: u32, at: u64| add_registration(&Registration { at, ..valid_registration(&browser(n, true)).unwrap() });
        for n in 1..=(MAX_REGISTRATIONS as u32 + 2) {
            add(n, 1000 + u64::from(n));
        }
        let ends = || {
            let mut e: Vec<u32> = registrations().iter().map(|r| r.subscription.endpoint.rsplit('/').next().unwrap().parse().unwrap()).collect();
            e.sort_unstable();
            e
        };
        assert_eq!(MAX_REGISTRATIONS, 20);
        assert_eq!(ends(), (3..=22).collect::<Vec<_>>());
        // a browser registering again moves to the end of the queue instead of taking a second place
        add(3, 5000);
        add(99, 5001);
        assert!(ends().contains(&3) && !ends().contains(&4));
    }

    #[test]
    fn a_hub_keeps_every_browsers_registration() {
        crate::test_util::core();
        add_registration(&valid_registration(&browser(1, true)).unwrap());
        add_registration(&valid_registration(&browser(3, true)).unwrap());
        assert_eq!(registrations().len(), 2);
    }
}
