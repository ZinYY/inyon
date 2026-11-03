//! Port of platform/store.ts: where the core keeps its small JSON records (machine list, settings,
//! drafts, last-seen snapshots, chat caches). The hub keeps them as files under ~/.inyon
//! (inyon-backends' `FsStore`); the phone app keeps them in its own storage. Keys look like file
//! paths without the extension: "hosts", "chats/abc".

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

pub trait Store {
    fn get(&self, key: &str) -> Option<Value>;
    /// Fails where the platform couldn't write it (a full disk); the TS throws then.
    fn set(&self, key: &str, value: &Value) -> Result<(), String>;
    fn del(&self, key: &str);
    /// keys under a prefix such as "chats/"
    fn keys(&self, prefix: &str) -> Vec<String>;
}

/// Until a real store is plugged in (and in tests), records live in memory, as JSON text so what
/// comes back is a copy.
#[derive(Default)]
pub struct MemoryStore {
    data: RefCell<BTreeMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.data.borrow().get(key).and_then(|raw| serde_json::from_str(raw).ok())
    }
    fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        self.data.borrow_mut().insert(key.to_owned(), crate::js::json::stringify(value));
        Ok(())
    }
    fn del(&self, key: &str) {
        self.data.borrow_mut().remove(key);
    }
    fn keys(&self, prefix: &str) -> Vec<String> {
        self.data.borrow().keys().filter(|k| k.starts_with(prefix)).cloned().collect()
    }
}

thread_local! {
    static CURRENT: RefCell<Rc<dyn Store>> = RefCell::new(Rc::new(MemoryStore::new()));
}

/// Plugs in the platform's store. Call it before anything reads a record.
pub fn use_store(store: Rc<dyn Store>) {
    CURRENT.with(|c| *c.borrow_mut() = store);
}

#[track_caller]
fn current() -> Rc<dyn Store> {
    crate::platform::rt::assert_core_thread("the store");
    CURRENT.with(|c| c.borrow().clone())
}

/// The records, through whichever store is plugged in (`kv` in the TS).
pub mod kv {
    use super::*;

    pub fn get_value(key: &str) -> Option<Value> {
        current().get(key)
    }

    /// The record as `T`. One that doesn't read as `T` is treated as missing; the TS would hand
    /// on whatever shape it found.
    pub fn get<T: DeserializeOwned>(key: &str) -> Option<T> {
        get_value(key).and_then(|v| serde_json::from_value(v).ok())
    }

    pub fn set_value(key: &str, value: &Value) -> Result<(), String> {
        current().set(key, value)
    }

    pub fn set<T: Serialize + ?Sized>(key: &str, value: &T) -> Result<(), String> {
        let v = serde_json::to_value(value).map_err(|e| e.to_string())?;
        set_value(key, &v)
    }

    pub fn del(key: &str) {
        current().del(key)
    }

    pub fn keys(prefix: &str) -> Vec<String> {
        current().keys(prefix)
    }
}

/// A file-name-safe form of an id, for keys like "chats/<id>": `s.replace(/[^\w.-]/g, '_')`.
pub fn key_part(s: &str) -> String {
    // per UTF-16 code unit, as the TS replaces them: an astral character becomes two
    s.encode_utf16()
        .map(|u| if (u < 0x80 && (u as u8 as char).is_ascii_alphanumeric()) || u == b'_' as u16 || u == b'.' as u16 || u == b'-' as u16 { u as u8 as char } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_part_replaces_what_a_file_name_cannot_hold() {
        assert_eq!(key_part("host-1 %3.2/ü😀"), "host-1__3.2____");
    }

    #[test]
    fn the_memory_store_hands_back_copies() {
        let s = MemoryStore::new();
        s.set("chats/a", &serde_json::json!({ "x": 1 })).unwrap();
        s.set("hosts", &serde_json::json!([])).unwrap();
        assert_eq!(s.keys("chats/"), vec!["chats/a"]);
        assert_eq!(s.get("chats/a").unwrap()["x"], 1);
        s.del("chats/a");
        assert_eq!(s.get("chats/a"), None);
    }
}
