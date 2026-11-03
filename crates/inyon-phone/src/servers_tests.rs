use std::rc::Rc;
use std::time::Duration;

use base64::Engine;
use inyon_api::http::test_req;
use inyon_core::platform::store::MemoryStore;
use serde_json::Value;

use inyon_core::platform::rt::Deferred;

use super::*;
use crate::phone::{start, Options, Phone};
use crate::ssh::B64;
use crate::testing::{net, offline, on_core, Answer, FakeMachine, FakeSsh, FINGERPRINT};

fn on_phone<F: std::future::Future<Output = ()>>(test: impl FnOnce(Phone, Rc<FakeSsh>) -> F) {
    on_core(|| {
        let fake = FakeSsh::new();
        test(
            start(Options { store: Rc::new(MemoryStore::new()), fetcher: None, ssh: fake.clone(), reach: offline(), name: "Test phone".into(), rotate_push: Rc::new(|| {}) }),
            fake,
        )
    });
}

async fn call(p: &Phone, method: &str, path: &str, body: Value) -> (u16, Value) {
    let text = if body.is_null() { String::new() } else { body.to_string() };
    let res = p.answer(test_req(method, path, &[("content-type", "application/json")], &text)).await;
    let status = res.status().as_u16();
    let bytes = res.into_body().bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Lets the registry's polls run until `check` holds (tokio's clock is paused, so this is quick).
async fn until(check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened");
}

fn connects(fake: &FakeSsh, id: &str) -> Vec<Value> {
    fake.calls_of("connect").into_iter().filter(|c| c["id"] == id).collect()
}

#[test]
fn the_phones_key_is_one_openssh_ed25519_line_made_once() {
    on_phone(|p, fake| async move {
        let (status, a) = call(&p, "GET", "/api/phone/key", Value::Null).await;
        assert_eq!(status, 200);
        let line = a["publicKey"].as_str().unwrap().to_owned();
        assert!(inyon_core::re!(r"^ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI[\w+/]+=* inyon-phone$").is_match(&line), "{line}");
        assert_eq!(call(&p, "GET", "/api/phone/key", Value::Null).await.1["publicKey"], line);
        // kept as the TS keeps it, so a phone updated from the TS core keeps its key
        let saved: Value = serde_json::from_str(&fake.secrets.borrow()["phone-key"]).unwrap();
        let public = B64.decode(saved["pub"].as_str().unwrap()).unwrap();
        assert_eq!(crate::keys::public_line(&public.try_into().unwrap()), line);
        assert_eq!(fake.calls_of("setSecret").len(), 1);
    });
}

#[test]
fn a_key_saved_by_the_typescript_core_is_the_one_used() {
    // any 32 bytes are a key
    let seed = [7u8; 32];
    let me = inyon_link::Identity::from_seed(seed);
    on_core(|| async move {
        // saved before the core starts, as on a phone that ran the TypeScript core
        let fake = FakeSsh::new();
        fake.secrets.borrow_mut().insert("phone-key".into(), json!({ "seed": B64.encode(seed), "pub": B64.encode(me.public_key) }).to_string());
        let p = start(Options { store: Rc::new(MemoryStore::new()), fetcher: None, ssh: fake.clone(), reach: offline(), name: "Test phone".into(), rotate_push: Rc::new(|| {}) });
        assert_eq!(call(&p, "GET", "/api/phone/key", Value::Null).await.1["publicKey"], crate::keys::public_line(&me.public_key));
        assert!(fake.calls_of("setSecret").is_empty());
    });
}

#[test]
fn probing_a_server_reads_its_fingerprint() {
    on_phone(|p, fake| async move {
        let (status, got) = call(&p, "POST", "/api/phone/probe", json!({ "target": "me@10.0.0.5", "port": 2222 })).await;
        assert_eq!((status, got["fingerprint"].as_str()), (200, Some(FINGERPRINT)));
        assert_eq!(fake.calls_of("probe"), [json!({ "host": "10.0.0.5", "port": 2222 })]);
        assert_eq!(call(&p, "POST", "/api/phone/probe", json!({ "target": " " })).await, (400, json!({ "error": "Enter the server address" })));
        fake.fail.borrow_mut().insert("probe", crate::ssh::PluginError::plain("timed out"));
        assert_eq!(call(&p, "POST", "/api/phone/probe", json!({ "target": "10.0.0.6" })).await, (502, json!({ "error": "Can't reach 10.0.0.6: timed out" })));
        assert_eq!(fake.calls_of("probe")[1]["port"], 22);
    });
}

#[test]
fn a_server_needs_a_user_and_a_confirmed_fingerprint() {
    on_phone(|p, _| async move {
        let refused = |b: Value| {
            let p = &p;
            async move { call(p, "POST", "/api/hosts", b).await }
        };
        assert_eq!(refused(json!({ "name": "x", "target": "10.0.0.5", "hostKey": FINGERPRINT })).await.1["error"], "Use user@host, like me@100.64.0.5");
        assert_eq!(refused(json!({ "name": "x", "target": "me@10.0.0.5" })).await.1["error"], "Check the server's fingerprint first");
        assert_eq!(refused(json!({ "name": "x", "target": "me@10.0.0.5", "hostKey": FINGERPRINT, "auth": "password" })).await, (400, json!({ "error": "Enter the password" })));
        assert_eq!(refused(json!({ "name": "x", "target": "me@-oProxy", "hostKey": FINGERPRINT })).await.1["error"], "The address has unexpected characters");
        assert_eq!(refused(json!({ "name": "x", "target": "me@h", "hostKey": FINGERPRINT, "auth": "telepathy" })).await.1["error"], "Unknown sign-in method");
        assert_eq!(refused(json!({ "name": "x", "target": "me@h", "hostKey": FINGERPRINT, "port": 70000 })).await.1["error"], "Port must be 1-65535");
        assert_eq!(refused(json!({ "name": " ", "target": "me@h" })).await.1["error"], "Give the server a name");
    });
}

#[test]
fn adding_editing_and_removing_a_server_keeps_its_secret_in_the_keystore() {
    on_phone(|p, fake| async move {
        let (status, add) =
            call(&p, "POST", "/api/hosts", json!({ "name": "Laptop", "target": "me@10.0.0.5", "port": 2222, "hostKey": FINGERPRINT, "auth": "password", "password": "hunter22" }))
                .await;
        assert_eq!(status, 200);
        let id = add["id"].as_str().unwrap().to_owned();
        assert_eq!(fake.secrets.borrow()[&format!("server:{id}")], r#"{"password":"hunter22"}"#);

        // the first poll connects with the saved password, and commands run through sh
        until(|| !connects(&fake, &id).is_empty()).await;
        assert_eq!(connects(&fake, &id)[0], json!({ "id": id, "host": "10.0.0.5", "port": 2222, "username": "me", "hostKey": FINGERPRINT, "password": "hunter22" }));
        until(|| fake.calls_of("exec").iter().any(|c| c["id"] == id)).await;
        assert!(fake.calls_of("exec").iter().all(|c| c["command"].as_str().unwrap().starts_with("sh -c '")));

        let (_, list) = call(&p, "GET", "/api/hosts", Value::Null).await;
        let saved = list.as_array().unwrap().iter().find(|h| h["id"] == id).unwrap().clone();
        assert_eq!((saved["name"].as_str(), saved["auth"].as_str(), saved["hostKey"].as_str()), (Some("Laptop"), Some("password"), Some(FINGERPRINT)));
        assert!(!list.to_string().contains("hunter22"));

        // switching to the phone's key drops the password and reconnects with the key
        assert_eq!(call(&p, "PATCH", &format!("/api/hosts/{id}"), json!({ "auth": "phone-key" })).await.0, 200);
        assert!(!fake.secrets.borrow().contains_key(&format!("server:{id}")));
        assert!(fake.calls_of("disconnect").iter().any(|c| c["id"] == id));
        until(|| connects(&fake, &id).iter().any(|c| c["ed25519Seed"].is_string())).await;
        let keyed = connects(&fake, &id).into_iter().find(|c| c["ed25519Seed"].is_string()).unwrap();
        assert!(keyed.get("password").is_none());

        // and back to a password: it has to come with one
        assert_eq!(call(&p, "PATCH", &format!("/api/hosts/{id}"), json!({ "auth": "password" })).await, (400, json!({ "error": "Enter the password" })));

        assert_eq!(call(&p, "DELETE", &format!("/api/hosts/{id}"), Value::Null).await, (200, json!({ "ok": true })));
        assert!(!call(&p, "GET", "/api/hosts", Value::Null).await.1.to_string().contains(&id));
        assert_eq!(call(&p, "PATCH", &format!("/api/hosts/{id}"), json!({ "name": "x" })).await, (404, json!({ "error": "Unknown server" })));
    });
}

#[test]
fn a_server_that_needs_no_key_or_password_signs_in_with_neither() {
    on_phone(|p, fake| async move {
        let (_, add) = call(&p, "POST", "/api/hosts", json!({ "name": "Open", "target": "me@10.0.0.7", "hostKey": FINGERPRINT, "auth": "none" })).await;
        let id = add["id"].as_str().unwrap().to_owned();
        assert!(!fake.secrets.borrow().contains_key(&format!("server:{id}")));
        until(|| !connects(&fake, &id).is_empty()).await;
        let c = connects(&fake, &id)[0].clone();
        assert_eq!((c["username"].as_str(), c["password"].as_str()), (Some("me"), Some("")));
        assert!(c.get("ed25519Seed").is_none());
    });
}

#[test]
fn a_key_pasted_for_a_server_is_kept_with_its_passphrase() {
    on_phone(|p, fake| async move {
        let (_, add) =
            call(&p, "POST", "/api/hosts", json!({ "name": "K", "target": "me@h", "hostKey": FINGERPRINT, "auth": "key", "privateKey": "-----BEGIN-----", "passphrase": "pw" }))
                .await;
        let id = add["id"].as_str().unwrap().to_owned();
        until(|| !connects(&fake, &id).is_empty()).await;
        let c = connects(&fake, &id)[0].clone();
        assert_eq!((c["privateKey"].as_str(), c["passphrase"].as_str()), (Some("-----BEGIN-----"), Some("pw")));
    });
}

#[test]
fn a_server_can_be_reached_through_a_paired_computer_instead_of_the_phone() {
    on_phone(|p, fake| async move {
        // only a paired computer can open it
        assert_eq!(
            call(&p, "POST", "/api/hosts", json!({ "name": "DB", "target": "work-db", "via": "nowhere" })).await,
            (400, json!({ "error": "Pick a paired computer to connect from" }))
        );
        let jump = HostConfig {
            id: "jump".into(),
            name: "Jump box".into(),
            kind: HostKind::Link,
            machine_id: Some("test-machine".into()),
            urls: Some(vec![]),
            relays: Some(vec![]),
            ..empty_config()
        };
        registry().add(jump, true).unwrap();
        // an alias from the computer's ssh config, no fingerprint and no sign-in of its own
        let (status, add) = call(&p, "POST", "/api/hosts", json!({ "name": "DB", "target": "work-db", "via": "jump" })).await;
        assert_eq!(status, 200);
        let id = add["id"].as_str().unwrap().to_owned();
        let (_, list) = call(&p, "GET", "/api/hosts", Value::Null).await;
        let saved = list.as_array().unwrap().iter().find(|h| h["id"] == id).unwrap().clone();
        assert_eq!((saved["kind"].as_str(), saved["target"].as_str(), saved["via"].as_str()), (Some("ssh"), Some("work-db"), Some("jump")));
        assert!(saved.get("hostKey").is_none() && saved.get("auth").is_none());
        // the phone never dials it itself
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(connects(&fake, &id).is_empty());

        // back to the phone's own connection: then it needs a fingerprint and a sign-in
        assert_eq!(call(&p, "PATCH", &format!("/api/hosts/{id}"), json!({ "via": "", "target": "me@10.0.0.9", "auth": "password" })).await.0, 400);
        assert_eq!(
            call(&p, "PATCH", &format!("/api/hosts/{id}"), json!({ "via": "", "target": "me@10.0.0.9", "hostKey": FINGERPRINT, "auth": "password", "password": "pw" })).await.0,
            200
        );
        let (_, list) = call(&p, "GET", "/api/hosts", Value::Null).await;
        assert!(list.as_array().unwrap().iter().find(|h| h["id"] == id).unwrap().get("via").is_none());

        // a computer is renamed, nothing else
        assert_eq!(call(&p, "PATCH", "/api/hosts/jump", json!({ "name": " " })).await, (400, json!({ "error": "Give the computer a name" })));
        assert_eq!(call(&p, "PATCH", "/api/hosts/jump", json!({ "name": "Desk" })).await.0, 200);
        assert_eq!(registry().get("jump").unwrap().borrow().host.config().name, "Desk");
    });
}

/// The phone, connected to a paired laptop ("laptop") at a LAN address.
async fn with_laptop(seed: [u8; 32]) -> (Phone, Rc<FakeSsh>, Rc<FakeMachine>) {
    let me = inyon_link::Identity::from_seed(seed);
    let net = net();
    let m = FakeMachine::new(9);
    m.paired.borrow_mut().push(me.noise.public);
    net.put("ws://10.0.0.5:4890", Answer::Machine(m.clone()));
    let fake = FakeSsh::new();
    fake.secrets.borrow_mut().insert("phone-key".into(), json!({ "seed": B64.encode(seed), "pub": B64.encode(me.public_key) }).to_string());
    let p = start(Options { store: Rc::new(MemoryStore::new()), fetcher: None, ssh: fake.clone(), reach: net.reach(), name: "Test phone".into(), rotate_push: Rc::new(|| {}) });
    let laptop = HostConfig {
        id: "laptop".into(),
        name: "Laptop".into(),
        kind: HostKind::Link,
        machine_id: Some(m.id.id.clone()),
        urls: Some(vec!["ws://10.0.0.5:4890".into()]),
        relays: Some(vec![]),
        ..empty_config()
    };
    registry().add(laptop, true).unwrap();
    until(|| node_link::is_up("laptop")).await;
    (p, fake, m)
}

fn pings(m: &FakeMachine) -> usize {
    m.heard.borrow().iter().filter(|h| h["op"] == "ping").count()
}

#[test]
fn removing_a_connected_computer_ends_what_waits_on_it_and_it_stays_removed() {
    on_core(|| async move {
        let (p, _, _) = with_laptop([7; 32]).await;
        // a stream the computer is still sending
        let ended = Rc::new(std::cell::RefCell::new(None));
        let e = ended.clone();
        let _tail = node_link::start("laptop", json!({ "op": "stream", "cmd": "tail -f log" }), move |m| *e.borrow_mut() = Some(m.clone())).unwrap();

        assert_eq!(call(&p, "DELETE", "/api/hosts/laptop", Value::Null).await, (200, json!({ "ok": true })));
        assert_eq!(ended.borrow().clone(), Some(json!({ "type": "error", "error": "The machine disconnected" })));
        assert!(!node_link::is_up("laptop"));
        // nothing connects it again
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!node_link::is_up("laptop") && registry().get("laptop").is_none());
    });
}

#[test]
fn a_change_while_a_computer_is_being_removed_doesnt_connect_it_again() {
    on_core(|| async move {
        let (p, fake, m) = with_laptop([7; 32]).await;
        // one from before 0.17 would take the phone back
        m.ignores_unpair.set(true);
        // the keystore is slow to answer, and something else changes meanwhile
        let slow = Deferred::new();
        fake.held.borrow_mut().insert("deleteSecret", slow.clone());
        let meanwhile = async {
            until(|| !fake.calls_of("deleteSecret").is_empty()).await;
            registry().emit_for_test(inyon_core::hosts::registry::RegistryEvent::Overview);
            tokio::time::sleep(Duration::from_secs(5)).await;
            slow.resolve(());
        };
        let (removed, _) = futures_util::future::join(call(&p, "DELETE", "/api/hosts/laptop", Value::Null), meanwhile).await;
        assert_eq!(removed.0, 200);
        // no connection is left to ping it
        let before = pings(&m);
        tokio::time::sleep(Duration::from_secs(90)).await;
        assert_eq!(pings(&m), before);
    });
}
