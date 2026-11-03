//! Port of the Bun hub's node-push.ts (removed in 0.23) and the phone's push routes. The web app has no server to
//! send its notifications, so each paired computer sends them: the page hands every computer this
//! browser's push registration (`inyon push set`), and the computer sends when an agent's hook says
//! it needs you or finished. The computer keys them by this phone's link key, the one it paired
//! with, so `inyon forget` takes them away too.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use futures_util::future::{join_all, LocalBoxFuture};
use http::Method;
use inyon_api::http::{hex, ok, ok_json, sha256};
use inyon_api::push::{add_registration, on_registrations, registrations, remove_registration, valid_registration, Registration};
use inyon_api::routes::{guarded, h, Routes};
use inyon_core::clock::now_ms;
use inyon_core::hosts::host::ExecOptions;
use inyon_core::hosts::http_error::{http, HubResult};
use inyon_core::hosts::registry::{registry, RegistryEvent};
use inyon_core::platform::console;
use inyon_core::platform::rt::spawn;
use inyon_protocol::{newer_release, HostKind, HostState, HostStatus};
use serde_json::{json, Value};

/// `inyon push set`, with what to keep on stdin.
pub const PUSH_SET: &str = r#""$INYON_EXE" ${INYON_SCRIPT:+"$INYON_SCRIPT"} push set"#;
/// `inyon push test`: sends a test to the endpoint on stdin, prints how many went out.
pub const PUSH_TEST: &str = r#""$INYON_EXE" ${INYON_SCRIPT:+"$INYON_SCRIPT"} push test"#;
/// The first inyon with them; an older one is left alone until it's updated.
pub const FIRST_PUSH: &str = "0.10.0";

/// What a computer keeps for this phone: its id there for each registration, and the registrations.
pub fn push_set_input(phone: &str, host: &str, regs: &[Registration]) -> String {
    let regs: Vec<Value> = regs.iter().map(|r| json!({ "subscription": r.subscription, "vapid": r.vapid, "done": r.done })).collect();
    json!({ "phone": phone, "host": host, "registrations": regs }).to_string()
}

fn can_push(h: &HostState) -> bool {
    h.config.kind == HostKind::Link && h.status == HostStatus::Online && h.node_version.as_deref().is_some_and(|v| !v.is_empty() && !newer_release(FIRST_PUSH, v))
}

pub type PhoneKey = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<[u8; 32], String>>>;

#[derive(Default)]
struct Sync {
    // what each computer was last given, so a reconnect doesn't send it again
    given: RefCell<HashMap<String, String>>,
    // computers it failed on, by when: try again a while later
    failed_at: RefCell<HashMap<String, u64>>,
    running: Cell<bool>,
    again: Cell<bool>,
}

/// Hands the browser's registrations to every computer that can send, whenever either changes.
pub fn start_node_push(phone_key: PhoneKey) {
    let s = Rc::new(Sync::default());
    let (s1, k1) = (s.clone(), phone_key.clone());
    let _ = registry().on(move |e| {
        if matches!(e, RegistryEvent::Overview) {
            forget_gone(&s1);
            sync(s1.clone(), k1.clone());
        }
    });
    let (s2, k2) = (s.clone(), phone_key.clone());
    on_registrations(move || sync(s2.clone(), k2.clone()));
    sync(s, phone_key);
}

/// A removed computer's entries go with it (the registry says so just after it removes one): one paired
/// again under the same id starts from nothing, so it's given the registrations again.
fn forget_gone(s: &Sync) {
    let ids: std::collections::HashSet<String> = registry().overview().into_iter().map(|h| h.config.id).collect();
    s.given.borrow_mut().retain(|id, _| ids.contains(id));
    s.failed_at.borrow_mut().retain(|id, _| ids.contains(id));
}

fn sync(s: Rc<Sync>, phone_key: PhoneKey) {
    if s.running.replace(true) {
        return s.again.set(true);
    }
    spawn(async move {
        if let Ok(key) = phone_key().await {
            give(&s, &hex(&key)).await;
        }
        s.running.set(false);
        if s.again.replace(false) {
            sync(s, phone_key);
        }
    });
}

async fn give(s: &Sync, phone: &str) {
    let regs = registrations();
    for h in registry().overview() {
        let id = h.config.id.clone();
        if !can_push(&h) {
            continue;
        }
        if now_ms().saturating_sub(s.failed_at.borrow().get(&id).copied().unwrap_or(0)) < 5 * 60_000 {
            continue;
        }
        let input = push_set_input(phone, &id, &regs);
        let sig = sha256(&input);
        if s.given.borrow().get(&id) == Some(&sig) {
            continue;
        }
        // a computer that was never given any has nothing to forget
        if regs.is_empty() && !s.given.borrow().contains_key(&id) {
            continue;
        }
        let Some(host) = registry().get(&id).map(|rt| rt.borrow().host.clone()) else { continue };
        let r = host.exec(PUSH_SET, ExecOptions::input(input).with_timeout(15_000)).await;
        match r {
            Ok(r) if r.code == 0 => {
                s.given.borrow_mut().insert(id.clone(), sig);
                s.failed_at.borrow_mut().remove(&id);
            }
            r => {
                s.failed_at.borrow_mut().insert(id.clone(), now_ms());
                let said = match r {
                    Ok(r) => {
                        if r.stderr.is_empty() {
                            r.stdout
                        } else {
                            r.stderr
                        }
                    }
                    Err(e) => e.message,
                };
                console::error(&format!("push: {id}: {}", said.trim().chars().take(200).collect::<String>()));
            }
        }
    }
}

/// A test from every computer that can send one; they share a tag, so it shows once.
pub async fn push_test(endpoint: &str) -> u64 {
    let input = json!({ "endpoint": endpoint }).to_string();
    let sends = registry().overview().into_iter().filter(can_push).filter_map(|h| registry().get(&h.config.id).map(|rt| rt.borrow().host.clone())).map(|host| {
        let input = input.clone();
        async move {
            match host.exec(PUSH_TEST, ExecOptions::input(input).with_timeout(20_000)).await {
                Ok(r) if r.code == 0 => r.stdout.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).map_or(0, |n| n as u64),
                _ => 0,
            }
        }
    });
    join_all(sends).await.into_iter().sum()
}

fn endpoint_of(b: &Value) -> HubResult<String> {
    inyon_api::http::field(b, "endpoint")?.and_then(Value::as_str).map(str::to_owned).ok_or_else(|| http(400, "Which endpoint?"))
}

/// The browser's side: POST its registration when notifications are allowed (and again when a
/// setting changes), DELETE it when they're turned off, and ask the computers for a test. Every
/// registration here is this one browser's, so a new one replaces the last.
pub fn add(routes: &mut Routes) {
    guarded(
        routes,
        "/api/push",
        vec![
            (
                Method::POST,
                h(|mut r| async move {
                    let reg = valid_registration(&r.json_body(16 * 1024).await?)?;
                    for old in registrations().into_iter().filter(|o| o.subscription.endpoint != reg.subscription.endpoint) {
                        remove_registration(&old.subscription.endpoint);
                    }
                    add_registration(&reg);
                    Ok(ok())
                }),
            ),
            (
                Method::DELETE,
                h(|mut r| async move {
                    remove_registration(&endpoint_of(&r.json_body(16 * 1024).await?)?);
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
            h(|mut r| async move {
                let endpoint = endpoint_of(&r.json_body(16 * 1024).await?)?;
                Ok(ok_json(&json!({ "sent": push_test(&endpoint).await })))
            }),
        )],
    );
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
