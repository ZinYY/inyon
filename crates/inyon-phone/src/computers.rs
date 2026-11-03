//! The phone's route for pairing with a computer (`/api/phone/pair`):
//! from the code `inyon pair` shows, or the short ID and code typed in, to a computer on the list.

use http::Method;
use inyon_api::core_routes::{empty_config, new_host_id};
use inyon_api::http::{fields, js_string, ok_json};
use inyon_api::routes::{guarded, h, Routes};
use inyon_core::hosts::http_error::http;
use inyon_core::hosts::registry::registry;
use inyon_link::{parse_pair_link, parse_short_code};
use inyon_protocol::{HostConfig, HostKind};
use serde_json::{json, Value};

use crate::link::{Links, Reach};

fn strings(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
}

pub fn add(r: &mut Routes, reach: Reach, links: Links) {
    guarded(
        r,
        "/api/phone/pair",
        vec![(
            Method::POST,
            h(move |mut req| {
                let (reach, links) = (reach.clone(), links.clone());
                async move {
                    let b = fields(&req.body().await?)?;
                    let text = b.get("link").filter(|v| !v.is_null()).map(js_string).unwrap_or_default();
                    let short = parse_short_code(&text);
                    // a typed ID and code: the relays say which machine the two together stand for
                    let info = match &short {
                        Some(c) => reach.short_code(c).await,
                        None => parse_pair_link(&text),
                    };
                    let Some(info) = info else {
                        return Err(if short.is_some() {
                            http(404, "That ID and code don't match a computer right now. Check both against the terminal, or run `inyon pair` again for new ones (each lasts 10 minutes).")
                        } else {
                            http(400, "That isn't an Inyon pairing code. Run `inyon pair` on the computer and scan the code it shows.")
                        });
                    };
                    let welcome = links.pair(&info).await.map_err(|e| http(502, e.message()))?;
                    let urls = strings(welcome.get("urls")).unwrap_or_else(|| info.urls.clone());
                    let relays = strings(welcome.get("relays")).unwrap_or_else(|| info.relays.clone());
                    let same = registry().configs().into_iter().find(|c| c.kind == HostKind::Link && c.machine_id.as_deref() == Some(info.id.as_str()));
                    if let Some(same) = same {
                        registry().note(&same.id, |c| {
                            c.urls = Some(urls);
                            c.relays = Some(relays);
                        })?;
                        links.resume();
                        return Ok(ok_json(&json!({ "id": same.id, "machineId": info.id })));
                    }
                    let given = b.get("name").filter(|v| !v.is_null()).map(js_string).unwrap_or_default().trim().to_owned();
                    let hostname = welcome.get("hostname").and_then(Value::as_str).filter(|h| !h.is_empty());
                    let name = [Some(given.as_str()).filter(|n| !n.is_empty()), hostname, Some(info.name.as_str()).filter(|n| !n.is_empty())]
                        .into_iter()
                        .flatten()
                        .next()
                        .unwrap_or("Laptop")
                        .to_owned();
                    let id = new_host_id(&name, "host");
                    registry().add(
                        HostConfig { id: id.clone(), name, kind: HostKind::Link, machine_id: Some(info.id.clone()), urls: Some(urls), relays: Some(relays), ..empty_config() },
                        true,
                    )?;
                    // the machine's id, for the app to show after a typed code: anyone who saw the code could
                    // have published their own computer under it
                    Ok(ok_json(&json!({ "id": id, "machineId": info.id })))
                }
            }),
        )],
    );
}
