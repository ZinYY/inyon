use std::cell::Cell;
use std::rc::Rc;

use inyon_api::http::test_req;
use inyon_core::platform::store::MemoryStore;
use inyon_protocol::HostConfig;

use super::*;
use crate::phone::{start, Options, Phone};
use crate::testing::{offline, on_core, FakeSsh, FINGERPRINT};

fn browser(n: u32) -> Value {
    json!({
        "subscription": { "endpoint": format!("https://push.example.net/{n}"), "keys": { "p256dh": "p", "auth": "a" } },
        "vapid": { "privateJwk": { "kty": "EC", "crv": "P-256", "d": "d", "x": "x", "y": "y" }, "subject": "https://inyon.tech" },
        "done": true,
    })
}

fn on_phone<F: std::future::Future<Output = ()>>(test: impl FnOnce(Phone, Rc<Cell<u32>>) -> F) {
    on_core(|| {
        let rotations = Rc::new(Cell::new(0));
        let r = rotations.clone();
        let o = Options {
            store: Rc::new(MemoryStore::new()),
            fetcher: None,
            ssh: FakeSsh::new(),
            reach: offline(),
            name: "Test phone".into(),
            rotate_push: Rc::new(move || r.set(r.get() + 1)),
        };
        test(start(o), rotations)
    });
}

async fn call(p: &Phone, method: &str, path: &str, body: Value) -> (u16, Value) {
    let text = if body.is_null() { String::new() } else { body.to_string() };
    let res = p.answer(test_req(method, path, &[("content-type", "application/json")], &text)).await;
    let status = res.status().as_u16();
    let bytes = res.into_body().bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn endpoints() -> Vec<String> {
    registrations().into_iter().map(|r| r.subscription.endpoint).collect()
}

#[test]
fn the_page_keeps_one_registration_and_can_take_it_back() {
    on_phone(|p, _| async move {
        assert_eq!(call(&p, "POST", "/api/push", browser(1)).await.0, 200);
        assert_eq!(endpoints(), ["https://push.example.net/1"]);
        // the same browser with a new subscription: the old one goes
        assert_eq!(call(&p, "POST", "/api/push", browser(2)).await.0, 200);
        assert_eq!(endpoints(), ["https://push.example.net/2"]);
        let mut bad = browser(3);
        bad["subscription"]["endpoint"] = json!("http://push.example.net/3");
        assert_eq!(call(&p, "POST", "/api/push", bad).await, (400, json!({ "error": "Bad push registration: endpoint" })));
        assert_eq!(call(&p, "POST", "/api/push/test", json!({})).await, (400, json!({ "error": "Which endpoint?" })));
        // no computer can send one yet
        assert_eq!(call(&p, "POST", "/api/push/test", json!({ "endpoint": "https://push.example.net/2" })).await, (200, json!({ "sent": 0 })));
        assert_eq!(call(&p, "DELETE", "/api/push", json!({ "endpoint": "https://push.example.net/2" })).await.0, 200);
        assert!(endpoints().is_empty());
    });
}

#[test]
fn removing_a_machine_has_the_page_make_new_push_keys_once_there_are_any() {
    on_phone(|p, rotations| async move {
        let add = |name: &'static str| {
            let p = &p;
            async move {
                call(p, "POST", "/api/hosts", json!({ "name": name, "target": "me@10.0.0.9", "hostKey": FINGERPRINT, "auth": "none" })).await.1["id"].as_str().unwrap().to_owned()
            }
        };
        // no notifications yet: nothing to make again
        let before = add("Before").await;
        assert_eq!(call(&p, "DELETE", &format!("/api/hosts/{before}"), Value::Null).await.0, 200);
        assert_eq!(rotations.get(), 0);
        call(&p, "POST", "/api/push", browser(1)).await;
        let after = add("After").await;
        assert_eq!(call(&p, "DELETE", &format!("/api/hosts/{after}"), Value::Null).await.0, 200);
        assert_eq!(rotations.get(), 1);
    });
}

#[test]
fn a_computer_paired_again_under_the_same_id_is_given_the_registrations_again() {
    on_phone(|p, _| async move {
        let id =
            call(&p, "POST", "/api/hosts", json!({ "name": "Laptop", "target": "me@10.0.0.9", "hostKey": FINGERPRINT, "auth": "none" })).await.1["id"].as_str().unwrap().to_owned();
        let s = Sync::default();
        s.given.borrow_mut().insert(id.clone(), "what it had".into());
        s.failed_at.borrow_mut().insert(id.clone(), 1);
        forget_gone(&s);
        assert!(s.given.borrow().contains_key(&id) && s.failed_at.borrow().contains_key(&id));
        // removed: what it was given goes too, as the registry says it's gone
        let s = Rc::new(s);
        let s2 = s.clone();
        let _ = registry().on(move |e| {
            if matches!(e, RegistryEvent::Overview) {
                forget_gone(&s2);
            }
        });
        assert_eq!(call(&p, "DELETE", &format!("/api/hosts/{id}"), Value::Null).await.0, 200);
        // the registry gathers its changes for 100 ms before saying so
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(s.given.borrow().is_empty() && s.failed_at.borrow().is_empty());
    });
}

#[test]
fn a_computer_is_given_what_it_needs_to_send_and_only_from_the_first_inyon_that_can() {
    let r = valid_registration(&browser(1)).unwrap();
    let input: Value = serde_json::from_str(&push_set_input("ab", "laptop", &[r])).unwrap();
    assert_eq!(input, json!({ "phone": "ab", "host": "laptop", "registrations": [{ "subscription": browser(1)["subscription"], "vapid": browser(1)["vapid"], "done": true }] }));
    let state = |kind: HostKind, status: HostStatus, version: Option<&str>| {
        let mut h: HostState = serde_json::from_value(json!({ "config": { "id": "x", "name": "x", "kind": "link" }, "status": "online", "integration": "unknown", "tmux": "unknown", "agents": [], "sessions": [], "updatedAt": 0 })).unwrap();
        h.config = HostConfig { kind, ..h.config };
        h.status = status;
        h.node_version = version.map(str::to_owned);
        h
    };
    assert!(can_push(&state(HostKind::Link, HostStatus::Online, Some("0.23.0"))));
    assert!(can_push(&state(HostKind::Link, HostStatus::Online, Some("0.10.0"))));
    assert!(!can_push(&state(HostKind::Link, HostStatus::Online, Some("0.9.9"))));
    assert!(!can_push(&state(HostKind::Link, HostStatus::Online, None)));
    assert!(!can_push(&state(HostKind::Link, HostStatus::Error, Some("0.23.0"))));
    assert!(!can_push(&state(HostKind::Ssh, HostStatus::Online, Some("0.23.0"))));
}
