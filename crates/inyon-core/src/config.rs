//! Port of config.ts: the machine list and the hub's settings, as records in the store.

use std::cell::RefCell;

use indexmap::IndexMap;
use inyon_protocol::HostConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::platform::store::kv;

// ---------- hosts ----------

thread_local! {
    /// the machine the core runs on, when it can run commands there (the hub can, the phone app can't)
    static LOCAL: RefCell<Option<HostConfig>> = const { RefCell::new(None) };
}

pub fn set_local_host(host: Option<HostConfig>) {
    LOCAL.with(|l| *l.borrow_mut() = host);
}

thread_local! {
    /// hosts.json as last read or written, so what this build can't read goes back out unchanged
    static RAW_HOSTS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

/// Every member of [`HostConfig`], as it's written. Members of an entry not named here (from a
/// newer Inyon, say) are kept as they were.
const HOST_FIELDS: &[&str] =
    &["id", "name", "kind", "target", "port", "identityFile", "hostKey", "auth", "via", "tokenHash", "pairedAt", "tokenIssuedAt", "machineId", "urls", "relays", "keepIntegration"];

/// The machine list. An entry this build can't read (one from another version, or a port of
/// 2222.5 that the TS's checks let through) is left out here, but kept in hosts.json: saving the
/// list never drops what it didn't understand.
pub fn load_hosts() -> Vec<HostConfig> {
    let raw: Vec<Value> = kv::get_value("hosts").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut hosts: Vec<HostConfig> = vec![];
    for h in &raw {
        match serde_json::from_value::<HostConfig>(h.clone()) {
            Ok(c) => hosts.push(c),
            Err(e) => crate::platform::console::error(&format!("hosts: kept an entry this version can't read ({e}): {}", crate::js::json::stringify(h))),
        }
    }
    RAW_HOSTS.with(|r| *r.borrow_mut() = raw);
    if let Some(local) = LOCAL.with(|l| l.borrow().clone()) {
        if !hosts.iter().any(|h| h.id == "local") {
            hosts.insert(0, local);
        }
    }
    hosts
}

/// Saves the machine list. Each machine is written over its entry as read, so members this build
/// doesn't know stay, and entries it couldn't read stay where they were.
pub fn save_hosts(hosts: &[HostConfig]) -> Result<(), String> {
    let raw = RAW_HOSTS.with(|r| r.borrow().clone());
    let readable = |v: &Value| serde_json::from_value::<HostConfig>(v.clone()).is_ok();
    let mut out: Vec<Value> = vec![];
    for h in hosts {
        let typed = serde_json::to_value(h).map_err(|e| e.to_string())?;
        let was = raw.iter().find(|v| readable(v) && v.get("id").and_then(Value::as_str) == Some(h.id.as_str()));
        // in the order it was written, as the TS's Object.assign keeps it
        let mut entry: Map<String, Value> = was.and_then(|v| v.as_object().cloned()).unwrap_or_default();
        let Value::Object(typed) = typed else { return Err("a machine that isn't an object".into()) };
        for k in HOST_FIELDS {
            if !typed.contains_key(*k) {
                entry.shift_remove(*k);
            }
        }
        entry.extend(typed);
        out.push(Value::Object(entry));
    }
    for (i, v) in raw.iter().enumerate() {
        if !readable(v) {
            out.insert(i.min(out.len()), v.clone());
        }
    }
    kv::set_value("hosts", &Value::Array(out.clone()))?;
    RAW_HOSTS.with(|r| *r.borrow_mut() = out);
    Ok(())
}

// ---------- settings ----------

/// maxTotalMB: the most a machine's uploads folder keeps before the oldest go (0: no limit)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSettings {
    pub retention_days: f64,
    #[serde(rename = "maxMB")]
    pub max_mb: f64,
    #[serde(rename = "maxTotalMB")]
    pub max_total_mb: f64,
    pub long_paste_lines: f64,
}

/// The settings as a JSON object, defaults filled in: the TS spreads whatever the file holds over
/// them, so keys this port doesn't know are kept too.
#[derive(Debug, Clone, PartialEq)]
pub struct HubSettings(pub Map<String, Value>);

impl HubSettings {
    pub fn uploads(&self) -> Option<UploadSettings> {
        self.0.get("uploads").and_then(|u| serde_json::from_value(u.clone()).ok())
    }

    /// machines where Inyon keeps a tmux session running
    pub fn keep_session(&self) -> IndexMap<String, bool> {
        self.0.get("keepSession").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    /// resume idle Claude sessions that stopped reporting live numbers (off unless turned on)
    pub fn auto_resume(&self) -> Option<bool> {
        self.0.get("autoResume").and_then(Value::as_bool)
    }
}

fn default_uploads() -> Map<String, Value> {
    let Value::Object(m) = serde_json::json!({ "retentionDays": 7, "maxMB": 100, "maxTotalMB": 1024, "longPasteLines": 40 }) else { unreachable!() };
    m
}

pub fn load_settings() -> HubSettings {
    let s = kv::get_value("settings").and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut uploads = default_uploads();
    if let Some(Value::Object(u)) = s.get("uploads") {
        for (k, v) in u {
            uploads.insert(k.clone(), v.clone());
        }
    }
    let mut out = Map::new();
    out.insert("uploads".into(), Value::Object(default_uploads()));
    for (k, v) in s {
        out.insert(k, v);
    }
    out.insert("uploads".into(), Value::Object(uploads));
    HubSettings(out)
}

pub fn save_settings(s: &HubSettings) -> Result<(), String> {
    kv::set_value("settings", &Value::Object(s.0.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_machine_comes_first_unless_the_list_has_one() {
        crate::test_support::rt::run(|| async {
            set_local_host(Some(serde_json::from_value(serde_json::json!({ "id": "local", "name": "box", "kind": "local" })).unwrap()));
            kv::set_value("hosts", &serde_json::json!([{ "id": "a", "name": "A", "kind": "ssh", "target": "a" }])).unwrap();
            let ids: Vec<String> = load_hosts().into_iter().map(|h| h.id).collect();
            assert_eq!(ids, vec!["local", "a"]);
        });
    }

    #[test]
    fn saving_the_machine_list_keeps_what_this_version_cant_read() {
        crate::test_support::rt::run(|| async {
            kv::set_value(
                "hosts",
                &serde_json::json!([
                    { "id": "a", "name": "A", "kind": "ssh", "target": "a", "fromLater": { "x": 1 } },
                    { "id": "odd", "name": "Odd", "kind": "ssh", "target": "o", "port": 2222.5 },
                    { "id": "b", "name": "B", "kind": "ssh", "target": "b", "port": 22 },
                ]),
            )
            .unwrap();
            let mut hosts = load_hosts();
            assert_eq!(hosts.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
            // a renamed and its port cleared, b removed
            hosts[0].name = "Alpha".into();
            hosts.pop();
            save_hosts(&hosts).unwrap();
            assert_eq!(
                crate::js::stringify(&kv::get_value("hosts").unwrap()),
                r#"[{"id":"a","name":"Alpha","kind":"ssh","target":"a","fromLater":{"x":1}},{"id":"odd","name":"Odd","kind":"ssh","target":"o","port":2222.5}]"#
            );
            // and a member this version knows, once cleared, stays cleared
            let mut hosts = load_hosts();
            hosts[0].target = None;
            save_hosts(&hosts).unwrap();
            assert!(!crate::js::stringify(&kv::get_value("hosts").unwrap()).contains(r#""target":"a""#));
        });
    }

    #[test]
    fn every_member_of_a_machine_is_one_the_list_knows() {
        let all: HostConfig = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "A", "kind": "ssh", "target": "a", "port": 22, "identityFile": "k", "hostKey": "SHA256:x", "auth": "key", "via": "v",
            "tokenHash": "h", "pairedAt": 1, "tokenIssuedAt": 1, "machineId": "m", "urls": [], "relays": [], "keepIntegration": true,
        }))
        .unwrap();
        let written = serde_json::to_value(&all).unwrap();
        let mut keys: Vec<&str> = written.as_object().unwrap().keys().map(String::as_str).collect();
        let mut known = HOST_FIELDS.to_vec();
        keys.sort();
        known.sort();
        assert_eq!(keys, known);
    }

    #[test]
    fn settings_fill_in_the_defaults_and_keep_what_they_dont_know() {
        crate::test_support::rt::run(|| async {
            kv::set_value("settings", &serde_json::json!({ "uploads": { "maxMB": 5 }, "autoResume": true, "other": 1 })).unwrap();
            let s = load_settings();
            assert_eq!(
                crate::js::stringify(&Value::Object(s.0.clone())),
                r#"{"uploads":{"retentionDays":7,"maxMB":5,"maxTotalMB":1024,"longPasteLines":40},"autoResume":true,"other":1}"#
            );
            assert_eq!(s.auto_resume(), Some(true));
        });
    }
}
