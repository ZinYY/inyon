//! The phone's own routes for the servers it SSHes into: its key,
//! reading a server's fingerprint, and adding, editing and removing servers. The password or
//! pasted key goes to the keystore, the rest to the machine list.

use std::rc::Rc;

use http::Method;
use inyon_api::core_routes::{empty_config, new_host_id};
use inyon_api::http::{fields, js_number, js_string, ok, ok_json, truthy};
use inyon_api::routes::{guarded, h, Routes};
use inyon_core::hosts::http_error::{http, HubError, HubResult};
use inyon_core::hosts::node_link;
use inyon_core::hosts::registry::registry;
use inyon_core::hosts::tidy::{drop_authorized_key, tidy_machine, TidyState};
use inyon_protocol::{HostAuth, HostConfig, HostKind};
use serde_json::{json, Map, Value};

use crate::keys::PhoneKeys;
use crate::link::Links;
use crate::ssh::{split_target, Credentials, SshBackend, SshPlugin};

/// `String(b.key ?? current ?? '')`
fn pick(b: &Map<String, Value>, key: &str, current: Option<&str>) -> String {
    match b.get(key).filter(|v| !v.is_null()) {
        Some(v) => js_string(v),
        None => current.unwrap_or_default().to_owned(),
    }
}

/// Server fields from the add/edit form, checked.
fn server_form(b: &Map<String, Value>, cur: Option<&HostConfig>) -> HubResult<HostConfig> {
    let name = pick(b, "name", cur.map(|c| c.name.as_str())).trim().to_owned();
    let target = pick(b, "target", cur.and_then(|c| c.target.as_deref())).trim().to_owned();
    if name.is_empty() {
        return Err(http(400, "Give the server a name"));
    }
    let port = if truthy(b.get("port")) { Some(js_number(b.get("port"))) } else { cur.and_then(|c| c.port).map(f64::from) };
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if port.is_some_and(|p| !(p > 0.0 && p < 65536.0)) {
        return Err(http(400, "Port must be 1-65535"));
    }
    let port = port.map(|p| p as u16);
    // through a paired computer: its ssh signs in and checks the server's key, and an alias from
    // its ~/.ssh/config is a fine address
    let via = pick(b, "via", cur.and_then(|c| c.via.as_deref()));
    if !via.is_empty() {
        let computer = registry().get(&via).map(|rt| rt.borrow().host.config());
        if computer.map(|c| c.kind) != Some(HostKind::Link) {
            return Err(http(400, "Pick a paired computer to connect from"));
        }
        if !inyon_core::re!(r"^[\w.@:-]+$").is_match(&target) || target.starts_with('-') {
            return Err(http(400, "Use user@host, or a name from that computer's ~/.ssh/config"));
        }
        return Ok(HostConfig { name, kind: HostKind::Ssh, target: Some(target), port, via: Some(via), ..empty_config() });
    }
    let (username, host) = split_target(&target);
    if !inyon_core::re!(r"^[\w.-]+$").is_match(&username) {
        return Err(http(400, "Use user@host, like me@100.64.0.5"));
    }
    if !inyon_core::re!(r"^[\w.:-]+$").is_match(&host) || host.starts_with('-') {
        return Err(http(400, "The address has unexpected characters"));
    }
    let host_key = pick(b, "hostKey", cur.and_then(|c| c.host_key.as_deref()));
    if !host_key.starts_with("SHA256:") {
        return Err(http(400, "Check the server's fingerprint first"));
    }
    let auth = match b.get("auth").filter(|v| !v.is_null()) {
        Some(v) => serde_json::from_value::<HostAuth>(v.clone()).map_err(|_| http(400, "Unknown sign-in method"))?,
        None => cur.and_then(|c| c.auth).unwrap_or(HostAuth::PhoneKey),
    };
    Ok(HostConfig { name, kind: HostKind::Ssh, target: Some(target), port, host_key: Some(host_key), auth: Some(auth), ..empty_config() })
}

fn needs_secret(auth: Option<HostAuth>) -> bool {
    matches!(auth, Some(HostAuth::Password | HostAuth::Key))
}

/// The password or pasted key, when the form brought one (an edit may keep the old one).
fn secret_from(b: &Map<String, Value>, auth: Option<HostAuth>) -> Option<Credentials> {
    let text = |k: &str| truthy(b.get(k)).then(|| b.get(k).map(js_string)).flatten();
    match auth {
        Some(HostAuth::Password) => text("password").map(|password| Credentials { password: Some(password), ..Default::default() }),
        Some(HostAuth::Key) => text("privateKey").map(|key| Credentials { private_key: Some(key), passphrase: text("passphrase"), ..Default::default() }),
        _ => None,
    }
}

fn missing_secret(auth: Option<HostAuth>) -> HubError {
    http(400, if auth == Some(HostAuth::Password) { "Enter the password" } else { "Paste the private key" })
}

pub fn add(r: &mut Routes, plugin: Rc<dyn SshPlugin>, keys: Rc<PhoneKeys>, conn: SshBackend, links: Links, rotate_push: Rc<dyn Fn()>) {
    use Method as M;
    let k = keys.clone();
    guarded(
        r,
        "/api/phone/key",
        vec![(
            M::GET,
            h(move |_| {
                let keys = k.clone();
                async move { Ok(ok_json(&json!({ "publicKey": keys.public_line().await.map_err(HubError::plain)? }))) }
            }),
        )],
    );

    guarded(
        r,
        "/api/phone/probe",
        vec![(
            M::POST,
            h(move |mut req| {
                let plugin = plugin.clone();
                async move {
                    let b = fields(&req.body().await?)?;
                    let (_, host) = split_target(pick(&b, "target", None).trim());
                    if host.is_empty() {
                        return Err(http(400, "Enter the server address"));
                    }
                    let port = Some(js_number(b.get("port"))).filter(|p| *p != 0.0 && !p.is_nan()).unwrap_or(22.0);
                    // a whole number goes as one ("22", not "22.0"), as JSON.stringify writes it
                    let port = if port.fract() == 0.0 && port.abs() < 1e15 { json!(port as i64) } else { json!(port) };
                    match plugin.call("probe", json!({ "host": host, "port": port })).await {
                        Ok(v) => Ok(ok_json(&v)),
                        Err(e) => Err(http(502, format!("Can't reach {host}: {}", e.message))),
                    }
                }
            }),
        )],
    );

    let k = keys.clone();
    guarded(
        r,
        "/api/hosts",
        vec![(
            M::POST,
            h(move |mut req| {
                let keys = k.clone();
                async move {
                    let b = fields(&req.body().await?)?;
                    let mut cfg = server_form(&b, None)?;
                    let secret = secret_from(&b, cfg.auth);
                    if needs_secret(cfg.auth) && secret.is_none() {
                        return Err(missing_secret(cfg.auth));
                    }
                    let id = new_host_id(&cfg.name, "host");
                    if let Some(s) = &secret {
                        keys.save(&id, s).await.map_err(HubError::plain)?;
                    }
                    cfg.id = id.clone();
                    registry().add(cfg, true)?;
                    Ok(ok_json(&json!({ "id": id })))
                }
            }),
        )],
    );

    let (k1, c1, k2, c2) = (keys.clone(), conn.clone(), keys, conn);
    guarded(
        r,
        "/api/hosts/:id",
        vec![
            (
                M::PATCH,
                h(move |mut req| {
                    let (keys, conn) = (k1.clone(), c1.clone());
                    async move {
                        let id = req.param("id");
                        let cur = registry().get(&id).map(|rt| rt.borrow().host.config()).ok_or_else(|| http(404, "Unknown server"))?;
                        let b = fields(&req.body().await?)?;
                        if cur.kind == HostKind::Link {
                            let name = pick(&b, "name", None).trim().to_owned();
                            if name.is_empty() {
                                return Err(http(400, "Give the computer a name"));
                            }
                            registry().note(&id, |c| c.name = name)?;
                            registry().refresh(&id, 0);
                            return Ok(ok());
                        }
                        let next = server_form(&b, Some(&cur))?;
                        let secret = secret_from(&b, next.auth);
                        if needs_secret(next.auth) && (next.auth != cur.auth || cur.via.is_some()) && secret.is_none() {
                            return Err(missing_secret(next.auth));
                        }
                        if let Some(s) = &secret {
                            keys.save(&id, s).await.map_err(HubError::plain)?;
                        }
                        if !needs_secret(next.auth) {
                            keys.drop_secret(&id).await;
                        }
                        conn.forget(&id).await;
                        registry().update(&id, |c| {
                            c.name = next.name;
                            c.kind = HostKind::Ssh;
                            c.target = next.target;
                            c.port = next.port;
                            c.host_key = next.host_key;
                            c.auth = next.auth;
                            c.via = next.via;
                        })?;
                        Ok(ok())
                    }
                }),
            ),
            (
                M::DELETE,
                h(move |req| {
                    let (keys, conn, links, rotate_push) = (k2.clone(), c2.clone(), links.clone(), rotate_push.clone());
                    async move {
                        let id = req.param("id");
                        let cur = registry().get(&id).map(|rt| rt.borrow().host.config());
                        // while it still answers: Inyon's hooks and uploads come off it, and so does the phone's key
                        if let Some(rt) = registry().get(&id) {
                            let (host, state) = {
                                let b = rt.borrow();
                                (b.host.clone(), TidyState::from(&b.state))
                            };
                            let also = if host.config().auth == Some(HostAuth::PhoneKey) {
                                drop_authorized_key(&keys.public_line().await.map_err(HubError::plain)?)
                            } else {
                                String::new()
                            };
                            tidy_machine(&host, Some(state), &also).await;
                        }
                        conn.forget(&id).await;
                        keys.drop_secret(&id).await;
                        // with nothing awaited between them: while it's still listed, the
                        // registry's next change would have the phone connect to it again
                        let told = match cur.as_ref().filter(|c| c.kind == HostKind::Link) {
                            Some(c) => links.forget(&id, Some(c)),
                            None => {
                                links.forget(&id, None);
                                true
                            }
                        };
                        registry().remove(&id, true)?;
                        // only now: dropped any earlier, the registry would re-probe it and the
                        // phone connect again; what still waits on its link ends here
                        node_link::removed(&id);
                        // it was handed this browser's push signing key and subscription, and may
                        // keep them: the page makes new ones and hands them to the computers left
                        // (apps/web/src/lib/push.ts)
                        if !inyon_api::push::registrations().is_empty() {
                            rotate_push();
                        }
                        // a computer out of reach still trusts the phone until it hears; the app says so
                        Ok(ok_json(&match cur.filter(|_| !told) {
                            Some(c) => json!({ "ok": true, "untold": c.name }),
                            None => json!({ "ok": true }),
                        }))
                    }
                }),
            ),
        ],
    );
}

#[cfg(test)]
#[path = "servers_tests.rs"]
mod tests;
